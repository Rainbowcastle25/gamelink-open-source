//! Apt owns the package and its service lifecycle. The updater downloads from
//! one isolated apt source, then gives dpkg only the selected GameLink archive.
use super::*;
use sha2::{Digest, Sha256};
use std::os::fd::AsRawFd;

pub(super) const BINARY: &str = "/usr/bin/gamelink-agent";
const STATE: &str = "/var/lib/gamelink-agent-apt-update";
const INSTALL_METHOD: &str = "/usr/share/gamelink-agent/install-method";
const PATH_UNIT: &str = "gamelink-agent-apt-update.path";
const SOURCE: &str = "/etc/apt/sources.list.d/gamelink.list";
const KEYRING: &str = "/usr/share/keyrings/gamelink.gpg";
const PACKAGE: &str = "gamelink-agent";
const WORK: &str = "apt-work";
const REQUEST_LIMIT: usize = 256;
const PACKAGE_LIMIT: u64 = 128 * 1024 * 1024;

pub(super) fn installed() -> bool {
    read_file(Path::new(INSTALL_METHOD), 4, 0).is_ok_and(|marker| marker == b"apt\n")
}

pub(super) fn paths() -> Paths {
    Paths {
        update: STATE.into(),
        installed: BINARY.into(),
        installer_uid: 0,
    }
}

pub(super) fn watching() -> bool {
    open_read(&Path::new(STATE).join("installer"))
        .and_then(|file| check_metadata(&file, 0, 0o755))
        .is_ok()
        && systemctl(&["is-active", PATH_UNIT]).is_ok_and(|output| output.trim() == "active")
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Request {
    version: String,
    attempt_id: String,
}

fn read_request(path: &Path, owner: u32) -> Result<Request> {
    let bytes = read_file(path, REQUEST_LIMIT, owner)
        .map_err(|_| "could not read the staged apt update request")?;
    let request: Request =
        serde_json::from_slice(&bytes).map_err(|_| "invalid staged apt update request")?;
    if !update::valid_release_version(&request.version) || !valid_attempt_id(&request.attempt_id) {
        return Err("invalid apt update version or attempt ID".into());
    }
    Ok(request)
}

pub(super) fn stage(paths: &Paths, release: &AgentRelease, status: &mut Status) -> Result<()> {
    release.deb_sha256()?;
    let request = Request {
        version: release.version(),
        attempt_id: status.attempt_id.clone().expect("reserved attempt ID"),
    };
    for (name, bytes) in [
        (update::MANIFEST_NAME, release.manifest_bytes()),
        (
            update::MANIFEST_SIGNATURE_NAME,
            release.manifest_signature(),
        ),
    ] {
        storage::write_atomic(&paths.staged().join(name), bytes)?;
    }
    status.state = Phase::Staged;
    save_stage_status(&paths.staged().join(STAGE_STATUS), status)?;
    publish_request_bytes(
        paths,
        &serde_json::to_vec(&request).map_err(|err| err.to_string())?,
    )
}

fn deb_version(version: &str) -> String {
    version.replacen('-', "~", 1)
}

fn release_version(version: &str) -> Result<String> {
    let version = version.replacen('~', "-", 1);
    if !update::valid_release_version(&version) {
        return Err("invalid installed GameLink Debian version".into());
    }
    Ok(version)
}

#[derive(Deserialize, Serialize)]
struct Previous {
    package: String,
    binary: String,
    enabled: String,
}

struct PackageStatus {
    selection: String,
    error: String,
    state: String,
    version: String,
}

impl PackageStatus {
    fn configured(&self) -> bool {
        self.error == "ok" && self.state == "installed"
    }

    fn can_restore(&self) -> bool {
        self.selection == "install"
            && matches!(
                self.state.as_str(),
                "half-installed"
                    | "unpacked"
                    | "half-configured"
                    | "triggers-awaited"
                    | "triggers-pending"
                    | "installed"
            )
    }
}

struct Apt<'a, C> {
    work: &'a Path,
    command: C,
}

impl<C> Apt<'_, C>
where
    C: Fn(&str, &[&str], &Path) -> Result<String>,
{
    fn command(&self, program: &str, args: &[&str]) -> Result<String> {
        (self.command)(program, args, self.work)
            .map_err(|error| format!("{program} failed: {error}"))
    }

    fn apt(&self, program: &str, args: &[&str]) -> Result<String> {
        let lists = format!("Dir::State::lists={}/lists", self.work.display());
        let mut scoped = vec![
            "-o",
            &lists,
            "-o",
            "Dir::Etc::sourcelist=/etc/apt/sources.list.d/gamelink.list",
            "-o",
            "Dir::Etc::sourceparts=-",
            "-o",
            "Dir::Etc::preferences=/dev/null",
            "-o",
            "Dir::Etc::preferencesparts=-",
            "-o",
            "Dir::Cache::pkgcache=",
            "-o",
            "Dir::Cache::srcpkgcache=",
            "-o",
            "APT::Get::List-Cleanup=0",
            "-o",
            "APT::Update::Error-Mode=any",
            "-o",
            "Acquire::AllowInsecureRepositories=false",
            "-o",
            "Acquire::AllowDowngradeToInsecureRepositories=false",
            "-o",
            "Acquire::Retries=0",
            "-o",
            "Acquire::http::Timeout=20",
            "-o",
            "Acquire::https::Timeout=20",
        ];
        scoped.extend_from_slice(args);
        self.command(program, &scoped)
    }

    fn fetch(&self, target: &str, previous: &str, sha256: &str) -> Result<()> {
        self.apt("apt-get", &["update"])?;
        let policy = self.apt("apt-cache", &["policy", PACKAGE])?;
        let candidate = policy
            .lines()
            .find_map(|line| line.trim().strip_prefix("Candidate: "));
        if candidate != Some(target) {
            return Err("requested version is not the latest GameLink apt package".into());
        }
        self.download(target, "target.deb")?;
        let archive = File::open(self.work.join("target.deb")).map_err(|err| err.to_string())?;
        let mut digest = Sha256::new();
        let size = io::copy(&mut archive.take(PACKAGE_LIMIT + 1), &mut digest)
            .map_err(|err| err.to_string())?;
        if size > PACKAGE_LIMIT || !format!("{:x}", digest.finalize()).eq_ignore_ascii_case(sha256)
        {
            return Err("GameLink apt package does not match the signed release digest".into());
        }
        // Cache the signed previous package before changing anything. Recovery
        // remains possible if apt is down or has already rotated the archive.
        self.download(previous, "previous.deb")
    }

    fn download(&self, version: &str, archive: &str) -> Result<()> {
        self.apt("apt-get", &["download", &format!("{PACKAGE}={version}")])?;
        let incoming = fs::read_dir(self.work)
            .map_err(|err| err.to_string())?
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name().into_string())
            .find_map(|name| {
                name.ok()
                    .filter(|name| name.starts_with("gamelink-agent_") && name.ends_with(".deb"))
            })
            .ok_or("apt did not download the GameLink package")?;
        let fields = self.command("dpkg-deb", &["--field", &incoming, "Package", "Version"])?;
        if fields != format!("Package: {PACKAGE}\nVersion: {version}\n") {
            return Err("unexpected GameLink Debian package name or version".into());
        }
        File::open(self.work.join(&incoming))
            .and_then(|file| file.sync_all())
            .and_then(|()| fs::rename(self.work.join(incoming), self.work.join(archive)))
            .and_then(|()| sync_dir(self.work))
            .map_err(|err| err.to_string())
    }

    fn package(&self) -> Result<Option<PackageStatus>> {
        let output = self.command(
            "dpkg-query",
            &[
                "-W",
                "-f=${Package} ${db:Status-Want} ${db:Status-Eflag} ${db:Status-Status} ${Version}\n",
                PACKAGE,
            ],
        )?;
        let Some(record) = output
            .lines()
            .find_map(|line| line.strip_prefix("gamelink-agent "))
        else {
            return Ok(None);
        };
        match record.split_whitespace().collect::<Vec<_>>().as_slice() {
            [selection, error, state, version] => Ok(Some(PackageStatus {
                selection: (*selection).into(),
                error: (*error).into(),
                state: (*state).into(),
                version: (*version).into(),
            })),
            [_, _, "not-installed", ..] => Ok(None),
            _ => Err("invalid GameLink dpkg status".into()),
        }
    }

    fn installed(&self) -> Result<PackageStatus> {
        self.package()?
            .filter(PackageStatus::configured)
            .ok_or_else(|| "GameLink apt package is not configured".into())
    }

    fn install(&self, archive: &str) -> Result<()> {
        self.command(
            "dpkg",
            &["--force-confold", "--no-triggers", "--install", archive],
        )?;
        Ok(())
    }
}

fn terminal<C>(
    paths: &Paths,
    owner: u32,
    apt: &Apt<'_, C>,
    mut tx: Transaction,
    phase: InstallPhase,
    reason: Option<String>,
) -> Result<()>
where
    C: Fn(&str, &[&str], &Path) -> Result<String>,
{
    tx.reason = reason;
    tx.save(paths, phase, &|_| {})?;
    finish(paths, owner, apt)
}

fn finish<C>(paths: &Paths, owner: u32, apt: &Apt<'_, C>) -> Result<()>
where
    C: Fn(&str, &[&str], &Path) -> Result<String>,
{
    // postinst leaves the recovery copy in place while pending.json exists.
    // Only replace it after reaching a terminal phase, with a configured package.
    if apt.package()?.is_some_and(|package| package.configured()) {
        let next = paths.update.join("installer.next");
        remove_if_present(&next)
            .and_then(|()| fs::copy(&paths.installed, &next))
            .and_then(|_| File::open(&next)?.sync_all())
            .and_then(|()| fs::rename(&next, paths.update.join("installer")))
            .and_then(|()| sync_dir(&paths.update))
            .map_err(|err| format!("could not refresh apt installer: {err}"))?;
    }
    complete_transaction(paths, owner, |path, uid| read_request(path, uid).is_ok())
        .map_err(|error| format!("could not finalize apt update: {error}"))
}

fn rollback<C, S, V>(
    paths: &Paths,
    owner: u32,
    apt: &Apt<'_, C>,
    service: &Service<S, V>,
    tx: Transaction,
    reason: String,
) -> Result<()>
where
    C: Fn(&str, &[&str], &Path) -> Result<String>,
    S: Fn(&[&str]) -> Result<String>,
    V: Fn(&Path) -> Result<String>,
{
    let data = fs::read(apt.work.join("previous.json")).map_err(|err| err.to_string())?;
    let previous: Previous = serde_json::from_slice(&data).map_err(|err| err.to_string())?;
    let version = tx
        .version
        .as_deref()
        .filter(|version| update::valid_release_version(version))
        .ok_or("apt transaction is missing its target version")?;
    let target = deb_version(version);
    let current = apt.package()?;
    let (phase, reason) = match current {
        Some(package) if package.version == target && package.can_restore() => {
            if package.configured() && service.health(&paths.installed, version).is_ok() {
                return terminal(paths, owner, apt, tx, InstallPhase::Committed, None);
            }
            let result = apt
                .install("previous.deb")
                .and_then(|()| restore_service(paths, apt, service, &previous));
            rollback_outcome(result, reason)
        }
        Some(package)
            if package.version == previous.package
                && package.configured()
                && package.can_restore() =>
        {
            (
                InstallPhase::Failed,
                format!("{reason}; the previous GameLink package remains installed"),
            )
        }
        _ => (
            InstallPhase::Failed,
            format!("{reason}; apt package conflict: another package operation superseded the GameLink update"),
        ),
    };
    terminal(paths, owner, apt, tx, phase, Some(reason.clone()))?;
    Err(reason)
}

fn restore_service<C, S, V>(
    paths: &Paths,
    apt: &Apt<'_, C>,
    service: &Service<S, V>,
    previous: &Previous,
) -> Result<()>
where
    C: Fn(&str, &[&str], &Path) -> Result<String>,
    S: Fn(&[&str]) -> Result<String>,
    V: Fn(&Path) -> Result<String>,
{
    if apt.installed()?.version != previous.package {
        return Err("previous GameLink package version was not restored".into());
    }
    if service.invocation().is_err() && enabled(service)? == previous.enabled {
        // Preserve the prior service state using the same policy gate as postinst.
        apt.command("policy-rc.d", &[UNIT, "start"])?;
        apt.command("deb-systemd-invoke", &["start", UNIT])?;
    }
    service.health(&paths.installed, &previous.binary)
}

fn rollback_outcome(result: Result<()>, reason: String) -> (InstallPhase, String) {
    match result {
        Ok(()) => (
            InstallPhase::RolledBack,
            format!("{reason}; previous agent restarted"),
        ),
        Err(error) => (
            InstallPhase::Failed,
            format!("{reason}; apt rollback failed: {error}"),
        ),
    }
}

fn enabled<S, V>(service: &Service<S, V>) -> Result<String>
where
    S: Fn(&[&str]) -> Result<String>,
{
    let result = (service.systemctl)(&["is-enabled", UNIT])?;
    match result.trim() {
        value @ ("enabled" | "enabled-runtime") => Ok(value.into()),
        _ => Err("apt desktop updates require an enabled GameLink service".into()),
    }
}

fn run<C, S, V>(
    paths: &Paths,
    owner: u32,
    apt: &Apt<'_, C>,
    service: &Service<S, V>,
    verify_release: impl FnOnce(&str, &str) -> Result<String>,
    package_lock: impl FnOnce() -> Result<File>,
    verify_source: impl FnOnce() -> Result<()>,
) -> Result<()>
where
    C: Fn(&str, &[&str], &Path) -> Result<String>,
    S: Fn(&[&str]) -> Result<String>,
    V: Fn(&Path) -> Result<String>,
{
    let staging_lock = paths.update_dir()?;
    let _run = installer_lock(paths)?;
    let pending = paths.transaction(PENDING)?;
    if pending.is_none() {
        paths.staging_dir(owner)?;
        if !present(&paths.marker())? {
            return Err("no staged apt update request".into());
        }
    }
    // Keep postinst's check for pending.json serialized with both the first
    // transaction write and finalization, as well as all package operations.
    let _package_lock = package_lock()?;
    if let Some(tx) = pending {
        return match tx.phase {
            InstallPhase::Stopping | InstallPhase::Swapped | InstallPhase::HealthChecking => {
                rollback(
                    paths,
                    owner,
                    apt,
                    service,
                    tx,
                    "apt upgrade was interrupted".into(),
                )
            }
            InstallPhase::Committed | InstallPhase::Failed | InstallPhase::RolledBack => {
                finish(paths, owner, apt)
            }
            InstallPhase::Staged => {
                if !present(&paths.update.join(CLAIMED_REQUEST))? && present(&paths.marker())? {
                    claim_request(paths, owner)?;
                }
                terminal(
                    paths,
                    owner,
                    apt,
                    tx,
                    InstallPhase::Failed,
                    Some("apt upgrade was interrupted before installation".into()),
                )
            }
        };
    }

    let staged = paths.staging_status(owner).unwrap_or_default();
    let mut tx = Transaction {
        phase: InstallPhase::Staged,
        attempt: staged.attempt,
        attempt_id: staged.attempt_id,
        updated_at: 0,
        version: None,
        reason: None,
    };
    tx.save(paths, InstallPhase::Staged, &|_| {})?;
    let prepare = (|| {
        claim_request(paths, owner)?;
        lock_within(&staging_lock, service.lock_wait, "agent staging")?;
        let request = read_request(&paths.update.join(CLAIMED_REQUEST), owner)?;
        if staged.state != Phase::Staged
            || tx.attempt_id.as_deref() != Some(request.attempt_id.as_str())
            || staged.version.as_deref() != Some(request.version.as_str())
        {
            return Err("apt update does not match the staged attempt".into());
        }
        tx.version = Some(request.version.clone());
        if let Some(prior) = paths.transaction(INSTALL_STATUS)? {
            if now_seconds().saturating_sub(prior.updated_at) < COOLDOWN.as_secs() {
                return Err("agent updates are limited to one attempt every five minutes".into());
            }
        }
        let package = apt
            .package()?
            .ok_or("GameLink apt package is not configured")?;
        if package.selection == "hold" {
            return Err("GameLink is held on the server.".into());
        }
        if !package.configured() || package.selection != "install" {
            return Err("GameLink apt package is not configured for installation".into());
        }
        let previous_package = package.version;
        let installed = release_version(&previous_package)?;
        if update::release_is_newer(&request.version, &installed) != Some(true) {
            return Err("requested GameLink apt version is not newer than installed".into());
        }
        verify_source()?;
        match fs::remove_dir_all(apt.work) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            other => other.map_err(|error| error.to_string())?,
        }
        fs::DirBuilder::new()
            .mode(0o700)
            .create(apt.work)
            .map_err(|err| err.to_string())?;
        // Disable system-wide apt config fragments and hooks for these reads.
        fs::write(
            apt.work.join("apt.conf"),
            "Dir::Etc::Parts \"-\";\nDir::Etc::Main \"-\";\n",
        )
        .map_err(|err| err.to_string())?;
        let digest = verify_release(&installed, &request.version)?;
        let target = deb_version(&request.version);
        apt.fetch(&target, &previous_package, &digest)?;
        let swap_lock = paths.swap_lock().map_err(|err| err.to_string())?;
        lock_within(&swap_lock, service.lock_wait, "agent CLI commands")?;
        service.invocation()?;
        let binary = (service.version)(&paths.installed)?
            .trim()
            .strip_prefix("gamelink-agent ")
            .filter(|version| update::valid_release_version(version))
            .ok_or("installed agent did not report a valid version")?
            .to_owned();
        let previous = Previous {
            package: previous_package,
            binary,
            enabled: enabled(service)?,
        };
        let data = serde_json::to_vec(&previous).map_err(|err| err.to_string())?;
        write_temp(apt.work, &data, 0o600)
            .and_then(|file| {
                file.persist(apt.work.join("previous.json"))
                    .map_err(|error| error.error)
            })
            .and_then(|_| sync_dir(apt.work))
            .and_then(|()| sync_dir(&paths.update))
            .map_err(|err| err.to_string())?;
        Ok((request, target, swap_lock))
    })();
    let (request, target, _swap_lock) = match prepare {
        Ok(prepared) => prepared,
        Err(reason) => {
            terminal(
                paths,
                owner,
                apt,
                tx,
                InstallPhase::Failed,
                Some(String::clone(&reason)),
            )?;
            return Err(reason);
        }
    };
    tx.save(paths, InstallPhase::Stopping, &|_| {})?;
    let result = (|| {
        apt.install("target.deb")?;
        if apt.installed()?.version != target {
            return Err("requested GameLink apt version was not installed".into());
        }
        tx.save(paths, InstallPhase::HealthChecking, &|_| {})?;
        service.health(&paths.installed, &request.version)
    })();
    if let Err(reason) = result {
        return rollback(paths, owner, apt, service, tx, reason);
    }
    terminal(paths, owner, apt, tx, InstallPhase::Committed, None)
}

fn dpkg_lock() -> Result<File> {
    // apt/dpkg use POSIX record locks, not flock. Hold the frontend lock
    // across installation, health checks and any downgrade.
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open("/var/lib/dpkg/lock-frontend")
        .map_err(|err| format!("could not open the apt frontend lock: {err}"))?;
    let start = Instant::now();
    while start.elapsed() < LOCK_WAIT {
        // SAFETY: zero initialization selects the whole file, and fcntl
        // receives a valid descriptor and pointer to a live flock.
        let mut lock: libc::flock = unsafe { std::mem::zeroed() };
        lock.l_type = libc::F_WRLCK as _;
        if unsafe { libc::fcntl(file.as_raw_fd(), libc::F_SETLK, &lock) } == 0 {
            return Ok(file);
        }
        let error = io::Error::last_os_error();
        if !matches!(error.raw_os_error(), Some(libc::EAGAIN | libc::EACCES)) {
            return Err(format!("could not acquire the apt frontend lock: {error}"));
        }
        thread::sleep(Duration::from_millis(100));
    }
    Err("apt is already busy on the server".into())
}

fn verify_source() -> Result<()> {
    let source = read_file(Path::new(SOURCE), 4096, 0)
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .ok_or("GameLink apt source is missing or unsafe")?;
    let mut lines = source
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'));
    let line = lines.next().ok_or("GameLink apt source is missing")?;
    let fields: Vec<_> = line.split_whitespace().collect();
    if lines.next().is_some()
        || !matches!(
            fields.as_slice(), ["deb", options, uri, "stable", "main"]
                if *options == format!("[signed-by={KEYRING}]")
                    && (uri.starts_with("https://") || uri.starts_with("file:/"))
        )
        || read_file(Path::new(KEYRING), 64 * 1024, 0).is_err()
    {
        return Err("use the GameLink apt source and signing key from the setup guide".into());
    }
    Ok(())
}

pub fn install_apt() -> Result<()> {
    let paths = paths();
    if effective_uid() != 0 {
        return Err("install-apt requires a root session on a GameLink apt installation".into());
    }
    let (uid, gid) = gamelink_account()?;
    let work = paths.update.join(WORK);
    let apt = Apt {
        work: &work,
        command: |program: &str, args: &[&str], dir: &Path| {
            if program == "policy-rc.d" {
                let policy = Path::new("/usr/sbin/policy-rc.d");
                if !policy.metadata().is_ok_and(|meta| meta.mode() & 0o111 != 0) {
                    return Ok(String::new());
                }
                let status = Command::new(policy)
                    .args(args)
                    .env_clear()
                    .env("PATH", "/usr/sbin:/usr/bin:/sbin:/bin")
                    .stdin(Stdio::null())
                    .status()
                    .map_err(|err| err.to_string())?;
                return match status.code() {
                    Some(0 | 104) => Ok(String::new()),
                    _ => Err("local service policy denied restoring GameLink".into()),
                };
            }
            let mut command = Command::new(format!("/usr/bin/{program}"));
            command.args(args);
            if program != "dpkg-query" {
                command.current_dir(dir);
            }
            if matches!(program, "apt-get" | "dpkg" | "deb-systemd-invoke") {
                // These commands may emit substantial progress output.
                let status = command
                    .env_clear()
                    .env("PATH", "/usr/sbin:/usr/bin:/sbin:/bin")
                    .env("DEBIAN_FRONTEND", "noninteractive")
                    .env("APT_CONFIG", dir.join("apt.conf"))
                    .env("DPKG_FRONTEND_LOCKED", "true")
                    .stdin(Stdio::null())
                    .status()
                    .map_err(|err| err.to_string())?;
                if !status.success() {
                    return Err(status.to_string());
                }
                return Ok(String::new());
            }
            if matches!(program, "apt-cache" | "dpkg-query") {
                command.env_clear().stdin(Stdio::null());
                if program == "apt-cache" {
                    command.env("APT_CONFIG", dir.join("apt.conf"));
                }
                let output = command.output().map_err(|err| err.to_string())?;
                // Exit 1 with no match means the requested package is absent.
                // dpkg-query reports fatal database errors with exit 2.
                if program == "dpkg-query"
                    && output.status.code() == Some(1)
                    && output.stdout.is_empty()
                {
                    return Ok(String::new());
                }
                if !output.status.success() {
                    return Err(output.status.to_string());
                }
                return String::from_utf8(output.stdout).map_err(|err| err.to_string());
            }
            command_output(&mut command)
        },
    };
    run(
        &paths,
        uid,
        &apt,
        &system_service(uid, gid),
        |installed, expected| {
            let release = staged_release(&paths, uid, installed)?;
            require_expected(Some(expected), &release.version())?;
            Ok(release.deb_sha256()?.to_owned())
        },
        || {
            let lock = dpkg_lock()?;
            // The saved helper remains executable if an interrupted dpkg left the
            // package binary (or its marker) missing.
            let helper =
                open_read(&paths.update.join("installer")).map_err(|err| err.to_string())?;
            check_metadata(&helper, 0, 0o755).map_err(|err| err.to_string())?;
            let running = fs::metadata("/proc/self/exe").map_err(|err| err.to_string())?;
            if !same_file(&helper.metadata().map_err(|err| err.to_string())?, &running)
                || (!installed() && !present(&paths.update.join(PENDING))?)
            {
                return Err("install-apt requires the root-owned GameLink apt installer".into());
            }
            Ok(lock)
        },
        verify_source,
    )
}

#[cfg(test)]
mod tests;
