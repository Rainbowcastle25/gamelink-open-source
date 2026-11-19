//! Linux agent updater. The staged directory is untrusted even when the
//! request came from the local agent. Only the installed helper swaps binaries.
use crate::{Result, VERSION};
use gamelink_core::{
    storage,
    update::{self, AgentRelease},
};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    os::unix::{
        fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
        process::CommandExt,
    },
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

type ReleaseResult<T> = std::result::Result<T, &'static str>;

const UPDATE: &str = "/var/lib/gamelink-agent/update";
const INSTALLED: &str = "/usr/local/bin/gamelink-agent";
const UNIT: &str = "gamelink-agent.service";
const REQUEST: &str = "request";
const STAGE_STATUS: &str = "status.json";
const INSTALL_STATUS: &str = "install-status.json";
const SWAP_LOCK: &str = "swap.lock";
const HEALTH_WINDOW: Duration = Duration::from_secs(10);

#[derive(Clone)]
struct Paths {
    update: PathBuf,
    installed: PathBuf,
}

impl Paths {
    fn system() -> Self {
        Self {
            update: UPDATE.into(),
            installed: INSTALLED.into(),
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

    fn bin_dir(&self) -> &Path {
        self.installed.parent().expect("agent binary has a parent")
    }

    // The sticky state home lets gamelink keep writing its existing files
    // while protecting the root-owned update directory from replacement.
    fn trusted_update_dir(&self) -> Result<()> {
        let home = safe_dir(self.update.parent().expect("update has a parent"))?;
        let update = safe_dir(&self.update)?;
        if home.uid() != 0
            || (home.mode() & 0o022 != 0 && home.mode() & libc::S_ISVTX == 0)
            || update.uid() != 0
            || update.mode() & 0o022 != 0
        {
            return Err("agent update directory ownership or permissions are unsafe".into());
        }
        Ok(())
    }

    fn staging_owner(&self) -> Result<u32> {
        self.trusted_update_dir()?;
        let staged = safe_dir(&self.staged())?;
        if staged.uid() == 0 || staged.mode() & 0o077 != 0 {
            return Err(
                "agent staging directory requires an unprivileged owner and mode 0700".into(),
            );
        }
        Ok(staged.uid())
    }

    fn stage_lock(&self) -> Result<File> {
        File::open(&self.update).map_err(|_| "could not open agent staging lock".into())
    }

    fn swap_lock(&self) -> io::Result<File> {
        let lock = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(self.update.join(SWAP_LOCK))?;
        let meta = lock.metadata()?;
        if !meta.is_file()
            || meta.mode() & 0o022 != 0
            || meta.uid() != fs::metadata(&self.update)?.uid()
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "unsafe agent CLI lock",
            ));
        }
        Ok(lock)
    }

    fn status(&self) -> Status {
        // The attempt associates progress with the root result for display
        // only. It plays no part in the installer's verification.
        let staged = read_status(&self.staged().join(STAGE_STATUS));
        let installed = read_status(&self.update.join(INSTALL_STATUS));
        match (staged, installed) {
            (Some(staged), Some(installed))
                if staged.attempt == installed.attempt && staged.state == Phase::Staged =>
            {
                installed
            }
            (Some(staged), _) => staged,
            (None, Some(installed)) => installed,
            (None, None) => Status::default(),
        }
    }
}

fn safe_dir(path: &Path) -> Result<fs::Metadata> {
    fs::symlink_metadata(path)
        .ok()
        .filter(|meta| meta.is_dir())
        .ok_or_else(|| "agent update directories are missing or unsafe".into())
}

#[derive(Clone, Copy, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
enum Phase {
    #[default]
    Idle,
    Checking,
    Downloading,
    Staged,
    Installing,
    Succeeded,
    Failed,
}

#[derive(Default, Deserialize, Serialize)]
struct Status {
    #[serde(default)]
    attempt: u64,
    state: Phase,
    version: Option<String>,
    reason: Option<String>,
}

fn read_status(path: &Path) -> Option<Status> {
    serde_json::from_slice(&read_file(path, 4096, None).ok()?).ok()
}

pub fn status() -> serde_json::Value {
    let paths = Paths::system();
    let status = if paths.trusted_update_dir().is_ok() {
        paths.status()
    } else {
        Status::default()
    };
    serde_json::json!({"state":status.state,"version":status.version,"reason":status.reason})
}

// Never follow symlinks or block on a FIFO in the untrusted staging area.
// The installer checks the directories separately before using this reader.
fn read_file(path: &Path, limit: usize, owner: Option<u32>) -> io::Result<Vec<u8>> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    let meta = file.metadata()?;
    if !meta.is_file()
        || owner.is_some_and(|uid| meta.uid() != uid || meta.nlink() != 1)
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
    _lock: File,
}

#[derive(Debug)]
pub enum QueueError {
    Busy,
    Unavailable,
}

impl Stage {
    fn acquire(paths: Paths) -> std::result::Result<Self, QueueError> {
        let lock = paths.stage_lock().map_err(|_| QueueError::Unavailable)?;
        lock.try_lock().map_err(|_| QueueError::Busy)?;
        if present(&paths.marker()).map_err(|_| QueueError::Unavailable)? {
            return Err(QueueError::Busy);
        }
        Ok(Self { paths, _lock: lock })
    }

    fn system() -> std::result::Result<Self, QueueError> {
        let paths = Paths::system();
        // SAFETY: geteuid has no arguments or memory requirements.
        if paths.staging_owner().ok() != Some(unsafe { libc::geteuid() })
            || paths.swap_lock().is_err()
            || !update::can_check()
        {
            return Err(QueueError::Unavailable);
        }
        Self::acquire(paths)
    }

    fn run(
        self,
        expected: Option<&str>,
        latest: impl FnOnce() -> ReleaseResult<AgentRelease>,
        download: impl FnOnce(&AgentRelease) -> ReleaseResult<(Vec<u8>, String)>,
    ) -> Result<String> {
        let staged = self.paths.staged();
        let status_path = staged.join(STAGE_STATUS);
        let mut status = Status {
            attempt: self.paths.status().attempt.wrapping_add(1),
            state: Phase::Checking,
            version: expected.map(str::to_owned),
            reason: None,
        };
        let result = (|| {
            save_stage_status(&status_path, &status)?;
            let release = latest()?;
            let version = release.version();
            status.version = Some(version.clone());
            require_expected(expected, &version)?;
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
            storage::write_atomic(&self.paths.marker(), b"")?;
            Ok(version)
        })();
        if let Err(reason) = &result {
            status.state = Phase::Failed;
            status.reason = Some(String::clone(reason));
            if let Err(error) = save_stage_status(&status_path, &status) {
                crate::log(&error);
            }
        }
        result
    }
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
    )
}

pub fn stage(expected: Option<&str>) -> Result<String> {
    if expected.is_some_and(|version| !update::valid_release_version(version)) {
        return Err("invalid expected agent release version".into());
    }
    Stage::system()
        .map_err(|err| match err {
            QueueError::Busy => "an agent update is already staged or running",
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

pub fn queue(expected: Option<String>) -> std::result::Result<(), QueueError> {
    let stage = Stage::system()?;
    thread::Builder::new()
        .name("agent-update".into())
        .spawn(move || {
            if let Err(reason) = stage.run(
                expected.as_deref(),
                || AgentRelease::latest(VERSION),
                AgentRelease::download_binary,
            ) {
                crate::log(&format!("agent update staging failed: {reason}"));
            }
        })
        .map_err(|_| QueueError::Unavailable)?;
    Ok(())
}

// Regular CLI commands hold a shared lock when the updater is configured.
// The service and the side-effect-free version probe run outside this gate.
pub fn cli_lock() -> Result<Option<File>> {
    let paths = Paths::system();
    if paths.trusted_update_dir().is_err() {
        return Ok(None);
    }
    let lock = paths
        .swap_lock()
        .map_err(|error| format!("could not open agent CLI lock: {error}"))?;
    lock.try_lock_shared()
        .map_err(|_| "agent update is installing; wait for the service to return")?;
    Ok(Some(lock))
}

struct Service<S, V> {
    systemctl: S,
    version: V,
    health_window: Duration,
}

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

fn save_install_status(paths: &Paths, status: &Status) -> Result<()> {
    let data = serde_json::to_vec(status).map_err(|error| error.to_string())?;
    write_temp(&paths.update, &data, 0o644)
        .and_then(|temp| {
            temp.persist(paths.update.join(INSTALL_STATUS))
                .map_err(|error| error.error)
        })
        .map_err(|error| format!("could not save installer status: {error}"))?;
    Ok(())
}

fn lock_swap(paths: &Paths) -> Result<File> {
    let lock = paths
        .swap_lock()
        .map_err(|error| format!("could not open agent CLI lock: {error}"))?;
    lock.lock()
        .map_err(|error| format!("could not lock out agent CLI commands: {error}"))?;
    Ok(lock)
}

fn sync_bin_dir(paths: &Paths) -> io::Result<()> {
    File::open(paths.bin_dir())?.sync_all()
}

fn remove_if_present(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        result => result,
    }
}

impl<S, V> Service<S, V>
where
    S: Fn(&[&str]) -> Result<String>,
    V: Fn(&Path) -> Result<String>,
{
    fn restore(&self, paths: &Paths) -> Result<()> {
        self.action("stop")?;
        fs::rename(paths.backup(), &paths.installed)
            .map_err(|error| format!("could not restore the previous binary: {error}"))?;
        let durable = remove_if_present(&paths.backup())
            .and_then(|()| sync_bin_dir(paths))
            .map_err(|error| format!("could not sync the restored binary: {error}"));
        durable.and(self.action("start"))
    }

    fn swap(
        &self,
        paths: &Paths,
        binary: &[u8],
        version: &str,
        finish: impl FnOnce() -> Result<()>,
    ) -> Result<()> {
        let candidate = write_temp(paths.bin_dir(), binary, 0o755)
            .map_err(|error| format!("could not prepare the new agent binary: {error}"))?;
        if let Err(reason) = self.action("stop") {
            return Err(with_recovery(reason, self.action("start")));
        }
        let backup = paths.backup();
        if let Err(error) = fs::hard_link(&paths.installed, &backup) {
            return Err(with_recovery(
                format!("could not back up the installed agent: {error}"),
                self.action("start"),
            ));
        }
        let result = (|| {
            sync_bin_dir(paths)
                .map_err(|error| format!("could not sync the agent backup: {error}"))?;
            candidate
                .persist(&paths.installed)
                .map_err(|error| format!("could not replace the agent binary: {}", error.error))?;
            sync_bin_dir(paths)
                .map_err(|error| format!("could not sync the new agent binary: {error}"))?;
            self.action("start")?;
            self.health(&paths.installed, version)?;
            finish()?;
            fs::remove_file(backup)
                .map_err(|error| format!("could not retire the agent backup: {error}"))
        })();
        result.map_err(|reason| with_recovery(reason, self.restore(paths)))
    }
}

fn with_recovery(reason: String, recovery: Result<()>) -> String {
    match recovery {
        Ok(()) => format!("{reason}; previous agent restarted"),
        Err(error) => format!("{reason}; recovery failed: {error}"),
    }
}

fn install<S, V>(
    paths: &Paths,
    staged_owner: u32,
    installed_version: &str,
    service: &Service<S, V>,
    verify_installed: impl FnOnce() -> Result<()>,
) -> Result<()>
where
    S: Fn(&[&str]) -> Result<String>,
    V: Fn(&Path) -> Result<String>,
{
    let run = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(paths.update.join("install.lock"))
        .map_err(|error| format!("could not open installer lock: {error}"))?;
    run.try_lock()
        .map_err(|_| "an agent installer is already running")?;
    let staging = paths.stage_lock()?;
    // The path unit can wake before the stage process releases its lock.
    staging.lock().map_err(|_| "could not lock agent staging")?;
    verify_installed()?;

    let marker = paths.marker();
    let backup = present(&paths.backup())?;
    if !present(&marker)? && !backup {
        return Err("no staged agent update request".into());
    }
    let attempt = read_file(&paths.staged().join(STAGE_STATUS), 4096, Some(staged_owner))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Status>(&bytes).ok())
        .map_or(0, |status| status.attempt);
    let mut status = Status {
        attempt,
        state: Phase::Installing,
        version: None,
        reason: None,
    };
    let clear_marker = || {
        remove_if_present(&marker)
            .map_err(|error| format!("could not clear the update request: {error}"))
    };
    let result = (|| {
        if backup {
            let _swap = lock_swap(paths)?;
            status.version =
                read_status(&paths.update.join(INSTALL_STATUS)).and_then(|old| old.version);
            return Err(with_recovery(
                "an agent installation was interrupted".into(),
                service.restore(paths),
            ));
        }
        let (version, binary) =
            verify_staged(paths, staged_owner, installed_version, &mut status.version)?;
        save_install_status(paths, &status)?;
        let _swap = lock_swap(paths)?;
        service.swap(paths, &binary, &version, || {
            clear_marker()?;
            status.state = Phase::Succeeded;
            save_install_status(paths, &status)
        })
    })();
    let Err(mut reason) = result else {
        return Ok(());
    };
    if let Err(clear) = clear_marker() {
        reason = format!("{reason}; {clear}");
    }
    status.state = Phase::Failed;
    status.reason = Some(reason.clone());
    if let Err(error) = save_install_status(paths, &status) {
        crate::log(&error);
    }
    Err(reason)
}

fn verify_staged(
    paths: &Paths,
    owner: u32,
    installed: &str,
    status_version: &mut Option<String>,
) -> Result<(String, Vec<u8>)> {
    let staged = paths.staged();
    let read = |name: &str, limit| {
        read_file(&staged.join(name), limit, Some(owner))
            .map_err(|error| format!("could not read staged {name}: {error}"))
    };
    let text = |name, limit| {
        String::from_utf8(read(name, limit)?).map_err(|_| format!("staged {name} is not UTF-8"))
    };
    let release = AgentRelease::from_staged(
        installed,
        text(update::MANIFEST_NAME, update::MAX_MANIFEST)?,
        text(update::MANIFEST_SIGNATURE_NAME, update::MAX_SIGNATURE)?,
    )?;
    let version = release.version();
    *status_version = Some(version.clone());
    let asset = update::agent_asset_name()?;
    let binary = read(asset, update::MAX_AGENT_BINARY)?;
    let signature = text(&format!("{asset}.minisig"), update::MAX_SIGNATURE)?;
    // Install only this verified in-memory snapshot. Never copy by path after
    // verification, because the unprivileged account can replace staged files.
    release.verify_binary(&binary, &signature)?;
    Ok((version, binary))
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

fn verify_current_binary(paths: &Paths) -> Result<()> {
    let parent = fs::symlink_metadata(paths.bin_dir()).map_err(|error| error.to_string())?;
    let installed = fs::symlink_metadata(&paths.installed).map_err(|error| error.to_string())?;
    let running = fs::metadata("/proc/self/exe").map_err(|error| error.to_string())?;
    if !parent.is_dir()
        || parent.uid() != 0
        || parent.mode() & 0o022 != 0
        || !installed.is_file()
        || installed.uid() != 0
        || installed.mode() & 0o7022 != 0
        || (installed.dev(), installed.ino()) != (running.dev(), running.ino())
    {
        return Err("install-staged must run from the current root-owned installed agent".into());
    }
    Ok(())
}

pub fn install_staged() -> Result<()> {
    // SAFETY: geteuid has no arguments or memory requirements.
    if unsafe { libc::geteuid() } != 0 {
        return Err("install-staged requires root".into());
    }
    let paths = Paths::system();
    let (uid, gid) = gamelink_account()?;
    if paths.staging_owner()? != uid {
        return Err("the staged directory must belong to gamelink".into());
    }
    let service = Service {
        systemctl: |args: &[&str]| {
            command_output(
                Command::new("/usr/bin/systemctl")
                    .arg("--no-ask-password")
                    .args(args),
            )
        },
        version: |path: &Path| {
            // CommandExt::uid also clears supplementary groups.
            command_output(
                Command::new("/usr/bin/timeout")
                    .args(["--kill-after=1s", "5s"])
                    .arg(path)
                    .arg("--version")
                    .gid(gid)
                    .uid(uid),
            )
        },
        health_window: HEALTH_WINDOW,
    };
    install(&paths, uid, VERSION, &service, || {
        verify_current_binary(&paths)
    })
}

#[cfg(test)]
mod tests;
