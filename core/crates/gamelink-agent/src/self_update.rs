//! Linux agent updater. The staged directory is untrusted even when the
//! request came from the local agent. Only the root helpers install updates.
mod apt;
use crate::{Result, VERSION};
use gamelink_core::{
    storage,
    update::{self, AgentRelease},
};
use serde::{Deserialize, Serialize};
use std::{
    ffi::CString,
    fs::{self, File, Metadata, OpenOptions},
    io::{self, Read, Write},
    os::unix::{
        fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt},
        process::CommandExt,
    },
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

type ReleaseResult<T> = std::result::Result<T, &'static str>;

const UPDATE: &str = "/var/lib/gamelink-agent/update";
const INSTALLED: &str = "/usr/local/bin/gamelink-agent";
const UNIT: &str = "gamelink-agent.service";
const REQUEST: &str = "request";
const STAGE_STATUS: &str = "status.json";
const PENDING: &str = "pending.json";
const CLAIMED_REQUEST: &str = "claimed-request";
const QUARANTINE: &str = "quarantine";
const INSTALL_STATUS: &str = "install-status.json";
const SWAP_LOCK: &str = "swap.lock";
const HEALTH_WINDOW: Duration = Duration::from_secs(10);
const LOCK_WAIT: Duration = Duration::from_secs(5);
const COOLDOWN: Duration = Duration::from_secs(5 * 60);

#[derive(Clone)]
struct Paths {
    update: PathBuf,
    installed: PathBuf,
    installer_uid: u32,
}

impl Paths {
    fn system() -> Self {
        if apt::installed() {
            return apt::paths();
        }
        Self::manual()
    }

    fn manual() -> Self {
        Self {
            update: UPDATE.into(),
            installed: INSTALLED.into(),
            installer_uid: 0,
        }
    }

    fn staged(&self) -> PathBuf {
        self.update.join("staged")
    }

    fn marker(&self) -> PathBuf {
        self.staged().join(REQUEST)
    }

    fn backup(&self) -> PathBuf {
        self.installed.with_file_name(".gamelink-agent.previous")
    }

    fn helper(&self) -> PathBuf {
        self.installed
            .with_file_name(".gamelink-agent-update-helper")
    }

    fn candidate(&self) -> PathBuf {
        self.installed.with_file_name(".gamelink-agent.next")
    }

    fn bin_dir(&self) -> &Path {
        self.installed.parent().expect("agent binary has a parent")
    }

    // The sticky state home lets gamelink keep writing its existing files
    // while protecting the root-owned update directory from replacement.
    fn update_dir(&self) -> Result<File> {
        let open = || {
            let home = open_dir(self.update.parent().expect("update has a parent"))?;
            if self.installed == Path::new(apt::BINARY) {
                let meta = home.metadata()?;
                if meta.uid() != self.installer_uid || meta.mode() & 0o022 != 0 {
                    return Err(io::Error::other("unsafe apt updater parent directory"));
                }
            } else {
                check_metadata(&home, self.installer_uid, 0o1770)?;
            }
            let update = open_dir(&self.update)?;
            check_metadata(&update, self.installer_uid, 0o750)?;
            Ok::<_, io::Error>(update)
        };
        open().map_err(|_| "agent update directory ownership or permissions are unsafe".into())
    }

    fn staging_dir(&self, owner: u32) -> Result<File> {
        open_dir(&self.staged())
            .and_then(|dir| {
                check_metadata(&dir, owner, 0o700)?;
                Ok(dir)
            })
            .map_err(|_| "agent staging directory requires gamelink ownership and mode 0700".into())
    }

    fn swap_lock(&self) -> io::Result<File> {
        let lock = open_read(&self.update.join(SWAP_LOCK))?;
        check_metadata(&lock, self.installer_uid, 0o644)?;
        if !lock.metadata()?.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "unsafe agent CLI lock",
            ));
        }
        Ok(lock)
    }

    fn staging_lock(&self, owner: u32) -> Result<File> {
        let lock = self.update_dir()?;
        self.swap_lock()
            .map_err(|_| "agent update requires a valid swap lock")?;
        self.staging_dir(owner)?;
        Ok(lock)
    }

    fn staging_status(&self, owner: u32) -> Option<Status> {
        self.staging_dir(owner).ok()?;
        let bytes = read_file(&self.staged().join(STAGE_STATUS), 4096, owner).ok()?;
        let status: Status = serde_json::from_slice(&bytes).ok()?;
        if status
            .attempt_id
            .as_deref()
            .is_some_and(|id| !valid_attempt_id(id))
        {
            return None;
        }
        matches!(
            status.state,
            Phase::Checking | Phase::Downloading | Phase::Staged | Phase::StagingFailed
        )
        .then_some(status)
    }

    fn transaction(&self, name: &str) -> Result<Option<Transaction>> {
        let bytes = match read_file(&self.update.join(name), 4096, self.installer_uid) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(format!("could not read installer record: {error}")),
        };
        serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|error| format!("invalid installer record: {error}"))
    }

    fn status(&self, owner: u32) -> Status {
        if self.update_dir().is_err() {
            return Status::default();
        }
        if let Ok(Some(active)) = self.transaction(PENDING) {
            return active.status();
        }
        let staged = self.staging_status(owner);
        let installed = self.transaction(INSTALL_STATUS).ok().flatten();
        match (staged, installed) {
            (Some(staged), Some(installed)) if staged.attempt <= installed.attempt => {
                installed.status()
            }
            (Some(staged), _) => staged,
            (None, Some(installed)) => installed.status(),
            (None, None) => Status::default(),
        }
    }
}

fn valid_attempt_id(id: &str) -> bool {
    id.len() == 32
        && id
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

pub fn install_method() -> &'static str {
    if apt::installed() {
        "apt"
    } else {
        "manual"
    }
}

fn open_dir(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_DIRECTORY)
        .open(path)
}

fn open_read(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
}

fn check_metadata(file: &File, owner: u32, mode: u32) -> io::Result<()> {
    let meta = file.metadata()?;
    if meta.uid() != owner || meta.mode() & 0o7777 != mode {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "unsafe update ownership or permissions",
        ));
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
enum Phase {
    #[default]
    Idle,
    Checking,
    Downloading,
    Staged,
    StagingFailed,
    Installing,
    Succeeded,
    RolledBack,
    #[serde(rename = "needs-manual-repair")]
    NeedsManualRepair,
    Failed,
}

#[derive(Default, Deserialize, Serialize)]
struct Status {
    #[serde(default)]
    attempt: u64,
    #[serde(default)]
    attempt_id: Option<String>,
    #[serde(default)]
    updated_at: u64,
    state: Phase,
    version: Option<String>,
    reason: Option<String>,
}

pub fn status() -> serde_json::Value {
    let paths = Paths::system();
    let owner = effective_uid();
    let supported = can_stage(owner, &paths) && paths.staging_lock(owner).is_ok();
    let mut status = paths.status(owner);
    if paths.installed == Path::new(apt::BINARY) {
        apt::add_refresh_status(&paths, &mut status);
    }
    status.host_view(supported)
}

impl Status {
    fn host_view(&self, supported: bool) -> serde_json::Value {
        serde_json::json!({
            "supported": supported,
            "state": self.state,
            "version": self.version,
            "reason": self.reason,
            "attemptId": self.attempt_id,
        })
    }
}

fn can_stage(owner: u32, paths: &Paths) -> bool {
    owner != 0
        && update::can_check()
        && (paths.installed != Path::new(apt::BINARY) || apt::watching())
}

fn effective_uid() -> u32 {
    // SAFETY: geteuid has no arguments or memory requirements.
    unsafe { libc::geteuid() }
}

// Never follow symlinks or block on a FIFO in the staging area. Callers
// validate directory ownership and permissions before using this reader.
fn read_file(path: &Path, limit: usize, owner: u32) -> io::Result<Vec<u8>> {
    let file = open_read(path)?;
    let meta = file.metadata()?;
    if !meta.is_file()
        || meta.uid() != owner
        || meta.nlink() != 1
        || meta.mode() & 0o7022 != 0
        || meta.len() > limit as u64
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid or oversized update file",
        ));
    }
    let mut bytes = Vec::new();
    file.take(limit as u64 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > limit {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "oversized update file",
        ));
    }
    Ok(bytes)
}

fn present(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(format!("could not inspect update file: {error}")),
    }
}

struct Stage {
    paths: Paths,
    status: Status,
    _lock: File,
}

#[derive(Debug)]
pub enum QueueError {
    Busy,
    Cooldown,
    Unavailable,
}

fn now_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

impl Stage {
    fn acquire(paths: Paths, now: u64) -> std::result::Result<Self, QueueError> {
        let owner = effective_uid();
        let lock = paths
            .staging_lock(owner)
            .map_err(|_| QueueError::Unavailable)?;
        let check_cooldown = || {
            let status = paths.status(owner);
            if status.updated_at != 0
                && status.updated_at <= now.saturating_add(COOLDOWN.as_secs())
                && now.saturating_sub(status.updated_at) < COOLDOWN.as_secs()
            {
                return Err(QueueError::Cooldown);
            }
            Ok(status)
        };
        check_cooldown()?;
        lock.try_lock().map_err(|_| QueueError::Busy)?;
        let previous = check_cooldown()?;
        if present(&paths.update.join(PENDING)).map_err(|_| QueueError::Unavailable)? {
            return Err(QueueError::Busy);
        }
        match fs::symlink_metadata(paths.marker()) {
            // PathExists cannot trigger on a dangling symlink. Retire it
            // under the staging lock without following its target.
            Ok(meta) if meta.is_symlink() => {
                remove_if_present(&paths.marker()).map_err(|_| QueueError::Unavailable)?;
            }
            Ok(_) => return Err(QueueError::Busy),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(_) => return Err(QueueError::Unavailable),
        }
        let mut random = [0u8; 16];
        File::open("/dev/urandom")
            .and_then(|mut file| file.read_exact(&mut random))
            .map_err(|_| QueueError::Unavailable)?;
        let status = Status {
            attempt: previous.attempt.saturating_add(1),
            attempt_id: Some(random.iter().map(|byte| format!("{byte:02x}")).collect()),
            updated_at: now,
            state: Phase::Checking,
            ..Status::default()
        };
        save_stage_status(&paths.staged().join(STAGE_STATUS), &status)
            .map_err(|_| QueueError::Unavailable)?;
        Ok(Self {
            paths,
            status,
            _lock: lock,
        })
    }

    fn system() -> std::result::Result<Self, QueueError> {
        let paths = Paths::system();
        if !can_stage(effective_uid(), &paths) {
            return Err(QueueError::Unavailable);
        }
        Self::acquire(paths, now_seconds())
    }

    fn attempt_id(&self) -> &str {
        self.status
            .attempt_id
            .as_deref()
            .expect("reserved attempt ID")
    }

    fn run(
        self,
        expected: Option<&str>,
        latest: impl FnOnce() -> ReleaseResult<AgentRelease>,
        download: impl FnOnce(&AgentRelease) -> ReleaseResult<(Vec<u8>, String)>,
    ) -> Result<String> {
        let staged = self.paths.staged();
        let status_path = staged.join(STAGE_STATUS);
        let mut status = self.status;
        status.version = expected.map(str::to_owned);
        let result = (|| {
            save_stage_status(&status_path, &status)?;
            let release = latest()?;
            let version = release.version();
            status.version = Some(version.clone());
            require_expected(expected, &version)?;
            if self.paths.installed == Path::new(apt::BINARY) {
                apt::stage(&self.paths, &release, &mut status)?;
                return Ok(version);
            }
            status.state = Phase::Downloading;
            save_stage_status(&status_path, &status)?;
            let (binary, signature) = download(&release)?;
            release.verify_binary(&binary, &signature)?;
            let asset = update::agent_asset_name()?;
            for (name, data) in [
                (asset, binary.as_slice()),
                (&format!("{asset}.minisig"), signature.as_bytes()),
                (update::MANIFEST_NAME, release.manifest_bytes()),
                (
                    update::MANIFEST_SIGNATURE_NAME,
                    release.manifest_signature(),
                ),
            ] {
                storage::write_atomic(&staged.join(name), data)?;
            }
            status.state = Phase::Staged;
            save_stage_status(&status_path, &status)?;
            // Publish the request only after all staged files and status exist.
            publish_request(&self.paths)?;
            Ok(version)
        })();
        if let Err(reason) = &result {
            status.state = Phase::StagingFailed;
            status.updated_at = now_seconds().max(status.updated_at);
            status.reason = Some(String::clone(reason));
            if let Err(error) = save_stage_status(&status_path, &status) {
                crate::log(&error);
            }
        }
        result
    }
}

fn publish_request(paths: &Paths) -> Result<()> {
    publish_request_bytes(paths, b"")
}

fn publish_request_bytes(paths: &Paths, request: &[u8]) -> Result<()> {
    storage::write_atomic(&paths.marker(), request)?;
    sync_dir(&paths.staged()).map_err(|error| format!("could not sync staged request: {error}"))
}

fn require_expected(expected: Option<&str>, latest: &str) -> Result<()> {
    if expected.is_some_and(|version| version != latest) {
        return Err("requested version is not the latest signed release".into());
    }
    Ok(())
}

fn save_stage_status(path: &Path, status: &Status) -> Result<()> {
    storage::write_atomic(
        path,
        &serde_json::to_vec(status).map_err(|e| e.to_string())?,
    )?;
    sync_dir(path.parent().expect("stage status has a parent"))
        .map_err(|error| format!("could not sync staging status: {error}"))
}

pub fn stage(expected: Option<&str>) -> Result<String> {
    if expected.is_some_and(|version| !update::valid_release_version(version)) {
        return Err("invalid expected agent release version".into());
    }
    Stage::system()
        .map_err(|err| match err {
            QueueError::Busy => "an agent update is already staged or running",
            QueueError::Cooldown => "agent updates are limited to one attempt every five minutes",
            QueueError::Unavailable => {
                "agent updates require a signed build and configured staging owned by gamelink"
            }
        })?
        .run(
            expected,
            || AgentRelease::latest(VERSION),
            AgentRelease::download_binary,
        )
}

fn latest_for_admin() -> ReleaseResult<AgentRelease> {
    // The disposable systemd test has no network and mounts signed fixtures.
    #[cfg(debug_assertions)]
    if option_env!("GAMELINK_APT_TEST_VERSION").is_some() {
        let read = |name| {
            fs::read_to_string(Path::new("/proof").join(name))
                .map_err(|_| "could not read signed apt test fixture")
        };
        return AgentRelease::from_staged(
            VERSION,
            read(update::MANIFEST_NAME)?,
            read(update::MANIFEST_SIGNATURE_NAME)?,
        );
    }
    AgentRelease::latest(VERSION)
}

pub fn queue(expected: Option<String>) -> std::result::Result<String, QueueError> {
    let stage = Stage::system()?;
    let attempt_id = stage.attempt_id().to_owned();
    thread::Builder::new()
        .name("agent-update".into())
        .spawn(move || {
            if let Err(reason) = stage.run(
                expected.as_deref(),
                latest_for_admin,
                AgentRelease::download_binary,
            ) {
                crate::log(&format!("agent update staging failed: {reason}"));
            }
        })
        .map_err(|_| QueueError::Unavailable)?;
    Ok(attempt_id)
}

// Regular CLI commands hold a shared lock when the updater is configured.
// The service and the side-effect-free version probe run outside this gate.
pub fn cli_lock() -> Result<Option<File>> {
    let paths = Paths::system();
    if paths.update_dir().is_err() {
        return Ok(None);
    }
    let lock = paths
        .swap_lock()
        .map_err(|error| format!("could not open agent CLI lock: {error}"))?;
    lock.try_lock_shared()
        .map_err(|_| "agent update is installing; wait for the service to return")?;
    // Recovery uses the durable transaction as its gate after a helper crash.
    if present(&paths.update.join(PENDING))? {
        return Err("agent update is installing; wait for the service to return".into());
    }
    Ok(Some(lock))
}

struct Service<S, V> {
    systemctl: S,
    version: V,
    health_window: Duration,
    lock_wait: Duration,
}

type Systemctl<'a> = Box<dyn Fn(&[&str]) -> Result<String> + 'a>;
type VersionProbe<'a> = Box<dyn Fn(&Path) -> Result<String> + 'a>;
type ConfiguredService<'a> = Service<Systemctl<'a>, VersionProbe<'a>>;

impl<S, V> Service<S, V>
where
    S: Fn(&[&str]) -> Result<String>,
    V: Fn(&Path) -> Result<String>,
{
    fn action(&self, action: &str) -> Result<()> {
        (self.systemctl)(&[action, UNIT])
            .map(|_| ())
            .map_err(|error| format!("could not {action} the agent service: {error}"))
    }

    fn invocation(&self) -> Result<String> {
        let output = (self.systemctl)(&["show", "--property=ActiveState,InvocationID", UNIT])?;
        if output.lines().any(|line| line == "ActiveState=active") {
            if let Some(id) = output
                .lines()
                .find_map(|line| line.strip_prefix("InvocationID="))
            {
                if id.len() == 32
                    && id.bytes().all(|byte| byte.is_ascii_hexdigit())
                    && id.bytes().any(|byte| byte != b'0')
                {
                    return Ok(id.to_owned());
                }
            }
        }
        Err("agent service is not active".into())
    }

    fn health(&self, installed: &Path, version: &str) -> Result<()> {
        let invocation = self.invocation()?;
        let start = Instant::now();
        if (self.version)(installed)?.trim() != format!("gamelink-agent {version}") {
            return Err("installed agent did not report the signed release version".into());
        }
        loop {
            if self.invocation()? != invocation {
                return Err("agent service restarted during the health check".into());
            }
            let remaining = self.health_window.saturating_sub(start.elapsed());
            if remaining.is_zero() {
                return Ok(());
            }
            thread::sleep(remaining.min(Duration::from_millis(500)));
        }
    }
}

fn write_temp(dir: &Path, bytes: &[u8], mode: u32) -> io::Result<tempfile::NamedTempFile> {
    let mut temp = tempfile::Builder::new()
        .prefix(".gamelink-update-")
        .tempfile_in(dir)?;
    temp.write_all(bytes)?;
    temp.as_file()
        .set_permissions(fs::Permissions::from_mode(mode))?;
    temp.as_file().sync_all()?;
    Ok(temp)
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
enum InstallPhase {
    Staged,
    Stopping,
    Swapped,
    HealthChecking,
    Committed,
    RolledBack,
    NeedsManualRepair,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Checkpoint {
    Published(InstallPhase),
    Recorded(InstallPhase),
    Verified,
}

#[derive(Deserialize, Serialize)]
struct Transaction {
    phase: InstallPhase,
    attempt: u64,
    #[serde(default)]
    attempt_id: Option<String>,
    updated_at: u64,
    version: Option<String>,
    reason: Option<String>,
}

impl Transaction {
    fn status(self) -> Status {
        let state = match self.phase {
            InstallPhase::Staged
            | InstallPhase::Stopping
            | InstallPhase::Swapped
            | InstallPhase::HealthChecking => Phase::Installing,
            InstallPhase::Committed => Phase::Succeeded,
            InstallPhase::RolledBack => Phase::RolledBack,
            InstallPhase::NeedsManualRepair => Phase::NeedsManualRepair,
            InstallPhase::Failed => Phase::Failed,
        };
        Status {
            state,
            attempt: self.attempt,
            attempt_id: self.attempt_id,
            updated_at: self.updated_at,
            version: self.version,
            reason: self.reason,
        }
    }

    fn save(
        &mut self,
        paths: &Paths,
        phase: InstallPhase,
        checkpoint: &impl Fn(Checkpoint),
    ) -> Result<()> {
        self.phase = phase;
        self.updated_at = now_seconds();
        let data = serde_json::to_vec(self).map_err(|error| error.to_string())?;
        write_temp(&paths.update, &data, 0o644)
            .and_then(|temp| {
                temp.persist(paths.update.join(PENDING))
                    .map_err(|error| error.error)
            })
            .map_err(|error| format!("could not save installer transaction: {error}"))?;
        checkpoint(Checkpoint::Published(phase));
        sync_dir(&paths.update)
            .map_err(|error| format!("could not sync installer transaction: {error}"))?;
        checkpoint(Checkpoint::Recorded(phase));
        Ok(())
    }
}

fn lock_within(lock: &File, wait: Duration, name: &str) -> Result<()> {
    let start = Instant::now();
    loop {
        match lock.try_lock() {
            Ok(()) => return Ok(()),
            Err(std::fs::TryLockError::WouldBlock) if start.elapsed() < wait => {
                thread::sleep(
                    wait.saturating_sub(start.elapsed())
                        .min(Duration::from_millis(50)),
                );
            }
            Err(error) => return Err(format!("could not lock {name}: {error}")),
        }
    }
}

fn sync_dir(path: &Path) -> io::Result<()> {
    #[cfg(test)]
    tests::check_sync(path)?;
    open_dir(path)?.sync_all()
}

fn remove_if_present(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        result => result,
    }
}

fn claim_request(paths: &Paths, owner: u32) -> Result<()> {
    let staged = paths.staging_dir(owner)?;
    fs::rename(paths.marker(), paths.update.join(CLAIMED_REQUEST))
        .and_then(|()| sync_dir(&paths.update))
        .and_then(|()| staged.sync_all())
        .map_err(|error| format!("could not claim the update request: {error}"))
}

fn rename_noreplace(source: &Path, dest: &Path) -> io::Result<()> {
    let source = CString::new(source.as_os_str().as_encoded_bytes())?;
    let dest = CString::new(dest.as_os_str().as_encoded_bytes())?;
    // SAFETY: Both paths are NUL-terminated and live throughout the syscall.
    let result = unsafe {
        libc::syscall(
            libc::SYS_renameat2,
            libc::AT_FDCWD,
            source.as_ptr(),
            libc::AT_FDCWD,
            dest.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

fn retire_request(
    paths: &Paths,
    staged_owner: u32,
    valid: impl FnOnce(&Path, u32) -> bool,
) -> io::Result<()> {
    let claimed = paths.update.join(CLAIMED_REQUEST);
    if valid(&claimed, staged_owner) {
        return remove_if_present(&claimed);
    }
    match claimed.symlink_metadata() {
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
        Ok(_) => {}
    }
    let quarantine = paths.update.join(QUARANTINE);
    match fs::DirBuilder::new().mode(0o700).create(&quarantine) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error),
    }
    let dir = open_dir(&quarantine)?;
    check_metadata(&dir, paths.installer_uid, 0o700)?;
    let slot = quarantine.join(REQUEST);
    match slot.symlink_metadata() {
        Ok(_) => {
            // Keep one quarantined entry. Return the old entry to staging
            // under an inactive name so gamelink can dispose of it without root.
            tempfile::Builder::new()
                .prefix(".retired-request-")
                .disable_cleanup(true)
                .make_in(paths.staged(), |dest| rename_noreplace(&slot, dest))?;
            sync_dir(&paths.staged())?;
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    fs::rename(claimed, slot)?;
    dir.sync_all()
}

fn refresh_helper(paths: &Paths) -> io::Result<()> {
    let next = paths.helper().with_extension("next");
    remove_if_present(&next)?;
    fs::hard_link(&paths.installed, &next)?;
    fs::rename(&next, paths.helper())?;
    // Recovery can replay this rename with both names already hard-linked.
    remove_if_present(&next)
}

fn finish(paths: &Paths, staged_owner: u32, phase: InstallPhase) -> Result<()> {
    // The terminal phase must be durable before retiring rollback data.
    // Leave the root-owned path trigger in place until cleanup is durable.
    sync_dir(&paths.update)
        .and_then(|()| {
            if phase == InstallPhase::Committed {
                refresh_helper(paths)?;
            }
            Ok(())
        })
        .and_then(|()| remove_if_present(&paths.candidate()))
        .and_then(|()| remove_if_present(&paths.backup()))
        .and_then(|()| sync_dir(paths.bin_dir()))
        .and_then(|()| {
            complete_transaction(paths, staged_owner, |path, owner| {
                read_file(path, 0, owner).is_ok()
            })
        })
        .map_err(|error| format!("could not finish installer cleanup: {error}"))
}

fn complete_transaction(
    paths: &Paths,
    staged_owner: u32,
    valid: impl FnOnce(&Path, u32) -> bool,
) -> io::Result<()> {
    retire_request(paths, staged_owner, valid)
        .and_then(|()| sync_dir(&paths.update))
        .and_then(|()| {
            fs::rename(
                paths.update.join(PENDING),
                paths.update.join(INSTALL_STATUS),
            )
        })
        .and_then(|()| sync_dir(&paths.update))
}

impl<S, V> Service<S, V>
where
    S: Fn(&[&str]) -> Result<String>,
    V: Fn(&Path) -> Result<String>,
{
    fn restore(&self, paths: &Paths) -> Result<()> {
        self.action("stop")?;
        let restore = paths.installed.with_file_name(".gamelink-agent.restore");
        remove_if_present(&restore)
            .and_then(|()| fs::hard_link(paths.backup(), &restore))
            .and_then(|()| fs::rename(&restore, &paths.installed))
            // rename is a no-op when both names already point to the backup.
            .and_then(|()| remove_if_present(&restore))
            .and_then(|()| sync_dir(paths.bin_dir()))
            .map_err(|error| format!("could not restore the previous binary: {error}"))?;
        self.action("start")
    }

    fn roll_back(
        &self,
        paths: &Paths,
        staged_owner: u32,
        mut transaction: Transaction,
        reason: String,
        checkpoint: &impl Fn(Checkpoint),
    ) -> Result<()> {
        self.restore(paths)
            .map_err(|error| format!("{reason}; recovery failed: {error}"))?;
        let reason = format!("{reason}; previous agent restarted");
        transaction.reason = Some(reason.clone());
        transaction.save(paths, InstallPhase::RolledBack, checkpoint)?;
        finish(paths, staged_owner, transaction.phase)?;
        Err(reason)
    }

    fn recover(
        &self,
        paths: &Paths,
        staged_owner: u32,
        mut transaction: Transaction,
        checkpoint: &impl Fn(Checkpoint),
    ) -> Result<()> {
        let reason = "an agent installation was interrupted".to_owned();
        match transaction.phase {
            InstallPhase::Staged => {
                // The trigger may fire before the original helper claimed
                // the request. There has been no service or binary change.
                if !present(&paths.update.join(CLAIMED_REQUEST))? && present(&paths.marker())? {
                    claim_request(paths, staged_owner)?;
                }
                transaction.reason = Some(reason);
                transaction.save(paths, InstallPhase::Failed, checkpoint)?;
                finish(paths, staged_owner, transaction.phase)
            }
            InstallPhase::Stopping | InstallPhase::Swapped | InstallPhase::HealthChecking => {
                // The exclusive CLI lock was acquired before these phases.
                // The pending record guards new commands while recovering,
                // so recovery needs only the root-owned installer lock.
                self.roll_back(paths, staged_owner, transaction, reason, checkpoint)
            }
            InstallPhase::Committed
            | InstallPhase::RolledBack
            | InstallPhase::NeedsManualRepair
            | InstallPhase::Failed => finish(paths, staged_owner, transaction.phase),
        }
    }
}

fn installer_lock(paths: &Paths) -> Result<File> {
    let run = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(paths.update.join("install.lock"))
        .and_then(|run| {
            check_metadata(&run, paths.installer_uid, 0o600)?;
            if !run.metadata()?.is_file() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "unsafe installer lock",
                ));
            }
            Ok(run)
        })
        .map_err(|error| format!("could not open installer lock: {error}"))?;
    run.try_lock()
        .map_err(|_| "an agent installer is already running")?;
    Ok(run)
}

fn install<S, V>(
    paths: &Paths,
    staged_owner: u32,
    installed_version: &str,
    service: &Service<S, V>,
    verify_helper: impl FnOnce() -> Result<()>,
    checkpoint: impl Fn(Checkpoint),
) -> Result<()>
where
    S: Fn(&[&str]) -> Result<String>,
    V: Fn(&Path) -> Result<String>,
{
    let staging_lock = paths.update_dir()?;
    let _run = installer_lock(paths)?;
    verify_helper()?;

    if let Some(transaction) = paths.transaction(PENDING)? {
        return service.recover(paths, staged_owner, transaction, &checkpoint);
    }
    paths.staging_dir(staged_owner)?;
    if !present(&paths.marker())? {
        return Err("no staged agent update request".into());
    }
    if present(&paths.backup())? {
        return Err("previous agent backup exists without an active installer transaction".into());
    }
    let staged = paths.staging_status(staged_owner).unwrap_or_default();
    let mut transaction = Transaction {
        phase: InstallPhase::Staged,
        attempt: staged.attempt,
        attempt_id: staged.attempt_id,
        updated_at: 0,
        version: None,
        reason: None,
    };
    transaction.save(paths, InstallPhase::Staged, &checkpoint)?;
    let prepared = (|| {
        // Claim first so a busy staging process or malformed request cannot
        // leave the unprivileged trigger in place.
        claim_request(paths, staged_owner)?;
        read_file(&paths.update.join(CLAIMED_REQUEST), 0, staged_owner)
            .map_err(|error| format!("invalid staged update request: {error}"))?;
        // The path unit can wake before the staging process releases its lock.
        lock_within(&staging_lock, service.lock_wait, "agent staging")?;
        let (version, binary) = verify_staged(
            paths,
            staged_owner,
            installed_version,
            &mut transaction.version,
        )?;
        checkpoint(Checkpoint::Verified);
        let swap_lock = paths
            .swap_lock()
            .map_err(|error| format!("could not open agent CLI lock: {error}"))?;
        lock_within(&swap_lock, service.lock_wait, "agent CLI commands")?;
        // A fixed root-owned path lets recovery retire even a partially
        // written candidate after the helper is killed.
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(paths.candidate())
            .and_then(|mut candidate| {
                candidate.write_all(&binary)?;
                candidate.set_permissions(fs::Permissions::from_mode(0o755))?;
                candidate.sync_all()
            })
            .map_err(|error| format!("could not prepare the new agent binary: {error}"))?;
        fs::hard_link(&paths.installed, paths.backup())
            .and_then(|()| sync_dir(paths.bin_dir()))
            .map_err(|error| format!("could not back up the installed agent: {error}"))?;
        Ok((version, swap_lock))
    })();
    let (version, _swap_lock) = match prepared {
        Ok(prepared) => prepared,
        Err(reason) => {
            transaction.reason = Some(String::clone(&reason));
            transaction.save(paths, InstallPhase::Failed, &checkpoint)?;
            finish(paths, staged_owner, transaction.phase)?;
            return Err(reason);
        }
    };

    transaction.save(paths, InstallPhase::Stopping, &checkpoint)?;
    let swapped = (|| {
        service.action("stop")?;
        fs::rename(paths.candidate(), &paths.installed)
            .map_err(|error| format!("could not replace the agent binary: {error}"))?;
        sync_dir(paths.bin_dir())
            .map_err(|error| format!("could not sync the new agent binary: {error}"))
    })();
    if let Err(reason) = swapped {
        return service.roll_back(paths, staged_owner, transaction, reason, &checkpoint);
    }
    transaction.save(paths, InstallPhase::Swapped, &checkpoint)?;
    if let Err(reason) = service.action("start") {
        return service.roll_back(paths, staged_owner, transaction, reason, &checkpoint);
    }
    transaction.save(paths, InstallPhase::HealthChecking, &checkpoint)?;
    if let Err(reason) = service.health(&paths.installed, &version) {
        return service.roll_back(paths, staged_owner, transaction, reason, &checkpoint);
    }
    transaction.save(paths, InstallPhase::Committed, &checkpoint)?;
    finish(paths, staged_owner, transaction.phase)
}

fn verify_staged(
    paths: &Paths,
    owner: u32,
    installed: &str,
    status_version: &mut Option<String>,
) -> Result<(String, Vec<u8>)> {
    let release = staged_release(paths, owner, installed)?;
    let version = release.version();
    *status_version = Some(version.clone());
    let asset = update::agent_asset_name()?;
    let binary = staged_file(paths, owner, asset, update::MAX_AGENT_BINARY)?;
    let signature = staged_text(
        paths,
        owner,
        &format!("{asset}.minisig"),
        update::MAX_SIGNATURE,
    )?;
    // Install only this verified in-memory snapshot. Never copy by path after
    // verification, because the unprivileged account can replace staged files.
    release.verify_binary(&binary, &signature)?;
    Ok((version, binary))
}

fn staged_file(paths: &Paths, owner: u32, name: &str, limit: usize) -> Result<Vec<u8>> {
    read_file(&paths.staged().join(name), limit, owner)
        .map_err(|error| format!("could not read staged {name}: {error}"))
}

fn staged_text(paths: &Paths, owner: u32, name: &str, limit: usize) -> Result<String> {
    String::from_utf8(staged_file(paths, owner, name, limit)?)
        .map_err(|_| format!("staged {name} is not UTF-8"))
}

fn staged_release(paths: &Paths, owner: u32, installed: &str) -> Result<AgentRelease> {
    Ok(AgentRelease::from_staged(
        installed,
        staged_text(paths, owner, update::MANIFEST_NAME, update::MAX_MANIFEST)?,
        staged_text(
            paths,
            owner,
            update::MANIFEST_SIGNATURE_NAME,
            update::MAX_SIGNATURE,
        )?,
    )?)
}

fn command_output(command: &mut Command) -> Result<String> {
    let mut child = command
        .env_clear()
        .stdin(Stdio::null())
        .stderr(Stdio::inherit())
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|error| error.to_string())?;
    let mut output = Vec::new();
    let read = child
        .stdout
        .take()
        .expect("command output is piped")
        .take(4097)
        .read_to_end(&mut output);
    let status = child.wait().map_err(|error| error.to_string())?;
    if !status.success() {
        return Err(status.to_string());
    }
    read.map_err(|error| error.to_string())?;
    if output.len() > 4096 {
        return Err("command output is too large".into());
    }
    String::from_utf8(output).map_err(|_| "unexpected command output".into())
}

fn gamelink_account() -> Result<(u32, u32)> {
    // SAFETY: the argument is NUL-terminated. This runs before any agent
    // threads are started and the libc entry is copied before another lookup.
    let account = unsafe { libc::getpwnam(c"gamelink".as_ptr()).as_ref() }
        .ok_or("the gamelink system account is required")?;
    if account.pw_uid == 0 || account.pw_gid == 0 {
        return Err("the gamelink system account requires an unprivileged UID and GID".into());
    }
    Ok((account.pw_uid, account.pw_gid))
}

fn same_file(a: &Metadata, b: &Metadata) -> bool {
    (a.dev(), a.ino()) == (b.dev(), b.ino())
}

fn verify_running_helper(paths: &Paths) -> Result<()> {
    let parent = fs::symlink_metadata(paths.bin_dir()).map_err(|error| error.to_string())?;
    let helper = fs::symlink_metadata(paths.helper()).map_err(|error| error.to_string())?;
    let running = fs::metadata("/proc/self/exe").map_err(|error| error.to_string())?;
    if !parent.is_dir()
        || parent.uid() != paths.installer_uid
        || parent.mode() & 0o022 != 0
        || !helper.is_file()
        || helper.uid() != paths.installer_uid
        || helper.mode() & 0o7777 != 0o755
        || !same_file(&helper, &running)
    {
        return Err("install-staged must run from the root-owned update helper".into());
    }
    if !present(&paths.update.join(PENDING))? {
        let installed =
            fs::symlink_metadata(&paths.installed).map_err(|error| error.to_string())?;
        if !same_file(&helper, &installed) {
            return Err("relink the update helper to the current installed agent".into());
        }
    }
    Ok(())
}

fn systemctl(args: &[&str]) -> Result<String> {
    command_output(
        Command::new("/usr/bin/systemctl")
            .arg("--no-ask-password")
            .args(args),
    )
}

fn system_service(uid: u32, gid: u32) -> ConfiguredService<'static> {
    Service {
        systemctl: Box::new(systemctl),
        version: Box::new(move |path: &Path| {
            // CommandExt::uid also clears supplementary groups.
            command_output(
                Command::new("/usr/bin/timeout")
                    .args(["--kill-after=1s", "5s"])
                    .arg(path)
                    .arg("--version")
                    .gid(gid)
                    .uid(uid),
            )
        }),
        health_window: HEALTH_WINDOW,
        lock_wait: LOCK_WAIT,
    }
}

pub fn install_staged() -> Result<()> {
    if effective_uid() != 0 || apt::installed() {
        return Err("install-staged requires the manual root update helper".into());
    }
    let paths = Paths::manual();
    let (uid, gid) = gamelink_account()?;
    install(
        &paths,
        uid,
        VERSION,
        &system_service(uid, gid),
        || verify_running_helper(&paths),
        |_| {},
    )
}

pub use apt::install_apt;

#[cfg(test)]
mod tests;
