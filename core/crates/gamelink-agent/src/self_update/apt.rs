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
const INDEX_LIMIT: u64 = 1024 * 1024;
const REFRESH_ERROR: &str = "installer-refresh-error";
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

struct Previous {
    package: String,
    binary: String,
    enabled: String,
}

fn limited_text(file: File, limit: u64) -> io::Result<String> {
    let mut text = String::new();
    file.take(limit + 1).read_to_string(&mut text)?;
    if text.len() as u64 > limit {
        return Err(io::Error::other("oversized apt update file"));
    }
    Ok(text)
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

    fn can_roll_back(&self) -> bool {
        self.selection == "install"
            && (self.needs_manual_repair()
                || matches!(
                    self.state.as_str(),
                    "triggers-awaited" | "triggers-pending" | "installed"
                ))
    }

    fn needs_manual_repair(&self) -> bool {
        matches!(
            self.state.as_str(),
            "half-installed" | "half-configured" | "unpacked"
        )
    }
}

pub(super) fn add_refresh_status(paths: &Paths, status: &mut Status) {
    if let Some(warning) = read_file(&paths.update.join(REFRESH_ERROR), 4096, paths.installer_uid)
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())
    {
        status.reason = Some(match status.reason.take() {
            Some(reason) => format!("{reason}; {warning}"),
            None => warning,
        });
    }
}

fn sandbox_account() -> Result<(u32, u32)> {
    if cfg!(test) {
        return Ok((effective_uid(), unsafe { libc::getegid() }));
    }
    // SAFETY: the argument is NUL-terminated and the entry is read immediately.
    let account = unsafe { libc::getpwnam(c"_apt".as_ptr()).as_ref() }
        .filter(|account| account.pw_uid != 0 && account.pw_gid != 0)
        .ok_or("the _apt sandbox account is required")?;
    Ok((account.pw_uid, account.pw_gid))
}

fn prepare_download_dirs(dir: &Path) -> Result<()> {
    let (uid, gid) = sandbox_account()?;
    (|| -> io::Result<()> {
        fs::set_permissions(dir, fs::Permissions::from_mode(0o755))?;
        // The full apt command runs as _apt. It may manage the cache
        // and its partial directories without access to helper state.
        for name in ["lists", "archives"] {
            let path = dir.join(name);
            fs::DirBuilder::new().mode(0o700).create(&path)?;
            std::os::unix::fs::chown(&path, Some(uid), Some(gid))?;
        }
        let config = dir.join("apt.conf");
        fs::write(&config, "Dir::Etc::Parts \"-\";\nDir::Etc::Main \"-\";\n")?;
        fs::set_permissions(config, fs::Permissions::from_mode(0o644))
    })()
    .map_err(|err| err.to_string())
}

fn package_hash<'a>(packages: &'a str, version: &str, arch: &str) -> Result<&'a str> {
    let mut digests = packages.split("\n\n").filter_map(|record| {
        let field = |prefix| record.lines().find_map(|line| line.strip_prefix(prefix));
        if field("Package: ") == Some(PACKAGE)
            && field("Version: ") == Some(version)
            && field("Architecture: ") == Some(arch)
        {
            return field("SHA256: ");
        }
        None
    });
    digests
        .next()
        .filter(|digest| digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .filter(|_| digests.next().is_none())
        .ok_or_else(|| format!("missing or ambiguous GameLink apt digest for {version} on {arch}"))
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

    fn apt(&self, download: &Path, program: &str, args: &[&str]) -> Result<String> {
        let lists = format!("Dir::State::lists={}/lists", download.display());
        let archives = format!("Dir::Cache::archives={}/archives", download.display());
        let mut scoped = vec![
            "-o",
            &lists,
            "-o",
            &archives,
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
            "Acquire::GzipIndexes=false",
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
        (self.command)(program, &scoped, &download.join("archives"))
            .map_err(|error| format!("{program} failed: {error}"))
    }

    fn fetch(&self, target: &str, previous: &str, sha256: &str) -> Result<()> {
        let arch = self.command("dpkg", &["--print-architecture"])?;
        let arch = arch.trim();
        if !matches!(arch, "amd64" | "arm64") {
            return Err("unsupported GameLink Debian architecture".into());
        }
        let parent = self
            .work
            .parent()
            .and_then(Path::parent)
            .expect("apt work has a home");
        let download = tempfile::Builder::new()
            .prefix(".gamelink-apt-download-")
            .tempdir_in(parent)
            .map_err(|err| err.to_string())?;
        let download_dir = download.path();
        prepare_download_dirs(download_dir)?;

        self.apt(download_dir, "apt-get", &["update"])?;
        let packages = self.signed_index(&download_dir.join("lists"), arch)?;
        let package = format!("{PACKAGE}:{arch}");
        let policy = self.apt(download_dir, "apt-cache", &["policy", &package])?;
        let candidate = policy
            .lines()
            .find_map(|line| line.trim().strip_prefix("Candidate: "));
        if candidate != Some(target) {
            return Err("requested version is not the latest GameLink apt package".into());
        }
        let target_hash = package_hash(&packages, target, arch)?;
        if !target_hash.eq_ignore_ascii_case(sha256) {
            return Err("GameLink apt package does not match the signed release digest".into());
        }
        self.download(download_dir, arch, target, "target.deb", target_hash)?;
        // Cache the authenticated previous package before invoking dpkg.
        let previous_hash = package_hash(&packages, previous, arch)?;
        self.download(download_dir, arch, previous, "previous.deb", previous_hash)
    }

    fn signed_index(&self, lists: &Path, arch: &str) -> Result<String> {
        let mut releases = fs::read_dir(lists)
            .map_err(|err| err.to_string())?
            .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
            .filter(|name| name.ends_with("_InRelease"));
        let release = releases
            .next()
            .filter(|_| releases.next().is_none())
            .ok_or("apt did not download one GameLink InRelease file")?;
        let prefix = release.trim_end_matches("_InRelease");
        let data = open_read(&lists.join(&release))
            .and_then(|file| limited_text(file, INDEX_LIMIT))
            .map_err(|err| err.to_string())?;
        // Verify a private copy before trusting hash fields from apt's download area.
        let signed =
            write_temp(self.work, data.as_bytes(), 0o600).map_err(|err| err.to_string())?;
        let signed = signed.path().to_str().ok_or("invalid apt work path")?;
        self.command(
            "gpgv",
            &["--keyring", KEYRING, "--output", "Release", signed],
        )?;
        let verified =
            fs::read_to_string(self.work.join("Release")).map_err(|err| err.to_string())?;
        let index = format!("main/binary-{arch}/Packages");
        let expected = verified
            .split_once("\nSHA256:\n")
            .and_then(|(_, hashes)| {
                hashes
                    .lines()
                    .take_while(|line| line.starts_with(' '))
                    .find_map(|line| {
                        let mut fields = line.split_whitespace();
                        let digest = fields.next()?;
                        let _size = fields.next()?;
                        (fields.next()? == index && fields.next().is_none()).then_some(digest)
                    })
            })
            .ok_or("missing GameLink Packages SHA256 in the signed apt index")?;
        let packages = open_read(&lists.join(format!("{prefix}_main_binary-{arch}_Packages")))
            .and_then(|file| limited_text(file, INDEX_LIMIT))
            .map_err(|err| err.to_string())?;
        if !format!("{:x}", Sha256::digest(packages.as_bytes())).eq_ignore_ascii_case(expected) {
            return Err("GameLink Packages file does not match the signed apt index".into());
        }
        Ok(packages)
    }

    fn download(
        &self,
        dir: &Path,
        arch: &str,
        version: &str,
        archive: &str,
        hash: &str,
    ) -> Result<()> {
        self.apt(
            dir,
            "apt-get",
            &["download", &format!("{PACKAGE}:{arch}={version}")],
        )?;
        let name = format!("{PACKAGE}_{version}_{arch}.deb");
        let incoming =
            open_read(&dir.join("archives").join(name)).map_err(|err| err.to_string())?;
        if !incoming
            .metadata()
            .map_err(|err| err.to_string())?
            .is_file()
        {
            return Err("apt did not download a regular GameLink archive".into());
        }
        let mut private =
            tempfile::NamedTempFile::new_in(self.work).map_err(|err| err.to_string())?;
        let size = io::copy(&mut incoming.take(PACKAGE_LIMIT + 1), &mut private)
            .map_err(|err| err.to_string())?;
        let mut digest = Sha256::new();
        let file = File::open(private.path()).map_err(|err| err.to_string())?;
        io::copy(&mut file.take(PACKAGE_LIMIT + 1), &mut digest).map_err(|err| err.to_string())?;
        if size > PACKAGE_LIMIT || !format!("{:x}", digest.finalize()).eq_ignore_ascii_case(hash) {
            return Err("GameLink apt archive does not match the signed apt index digest".into());
        }
        let path = private.path().to_str().ok_or("invalid apt archive path")?;
        let fields = self.command(
            "dpkg-deb",
            &["--field", path, "Package", "Version", "Architecture"],
        )?;
        if fields != format!("Package: {PACKAGE}\nVersion: {version}\nArchitecture: {arch}\n") {
            return Err("unexpected GameLink Debian package name, version or architecture".into());
        }
        private
            .as_file()
            .sync_all()
            .and_then(|()| {
                private
                    .persist(self.work.join(archive))
                    .map_err(|err| err.error)
            })
            .and_then(|_| sync_dir(self.work))
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
    finish(paths, owner, apt, phase)
}

fn finish<C>(paths: &Paths, owner: u32, apt: &Apt<'_, C>, phase: InstallPhase) -> Result<()>
where
    C: Fn(&str, &[&str], &Path) -> Result<String>,
{
    // Release the durable CLI gate before attempting to refresh the helper.
    complete_transaction(paths, owner, |path, uid| read_request(path, uid).is_ok())
        .map_err(|error| format!("could not finalize apt update: {error}"))?;
    if phase == InstallPhase::NeedsManualRepair {
        return Ok(());
    }
    if let Err(reason) = refresh_installer(paths, apt) {
        crate::log(&reason);
    }
    Ok(())
}

fn refresh_installer<C>(paths: &Paths, apt: &Apt<'_, C>) -> Result<()>
where
    C: Fn(&str, &[&str], &Path) -> Result<String>,
{
    let error_path = paths.update.join(REFRESH_ERROR);
    let result = (|| -> Result<()> {
        if apt.package()?.is_some_and(|package| package.configured()) {
            let mut next =
                tempfile::NamedTempFile::new_in(&paths.update).map_err(|err| err.to_string())?;
            let mut installed = open_read(&paths.installed).map_err(|err| err.to_string())?;
            check_metadata(&installed, paths.installer_uid, 0o755)
                .map_err(|err| err.to_string())?;
            io::copy(&mut installed, &mut next)
                .and_then(|_| {
                    next.as_file()
                        .set_permissions(fs::Permissions::from_mode(0o755))
                })
                .and_then(|()| next.as_file().sync_all())
                .and_then(|()| {
                    next.persist(paths.update.join("installer"))
                        .map_err(|err| err.error)
                })
                .and_then(|_| remove_if_present(&error_path))
                .and_then(|()| sync_dir(&paths.update))
                .map_err(|err| err.to_string())?;
        }
        Ok(())
    })();
    if let Err(error) = result {
        let reason = format!("could not refresh apt installer: {error}");
        if let Err(error) = write_temp(&paths.update, reason.as_bytes(), 0o644)
            .and_then(|file| file.persist(error_path).map_err(|err| err.error))
            .and_then(|_| sync_dir(&paths.update))
        {
            crate::log(&format!(
                "could not record apt installer refresh failure: {error}"
            ));
        }
        return Err(reason);
    }
    Ok(())
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
    let request = if pending.is_none() {
        paths.staging_dir(owner)?;
        present(&paths.marker())?
    } else {
        false
    };
    // Keep postinst's check for pending.json serialized with both the first
    // transaction write and finalization, as well as all package operations.
    let _package_lock = package_lock()?;
    if let Some(tx) = pending {
        return match tx.phase {
            InstallPhase::Committed
            | InstallPhase::Failed
            | InstallPhase::RolledBack
            | InstallPhase::NeedsManualRepair => finish(paths, owner, apt, tx.phase),
            InstallPhase::Staged
            | InstallPhase::Stopping
            | InstallPhase::Swapped
            | InstallPhase::HealthChecking => {
                if !present(&paths.update.join(CLAIMED_REQUEST))? && present(&paths.marker())? {
                    claim_request(paths, owner)?;
                }
                terminal(
                    paths,
                    owner,
                    apt,
                    tx,
                    InstallPhase::NeedsManualRepair,
                    Some(update::APT_MANUAL_REPAIR.into()),
                )
            }
        };
    }

    if !request || present(&paths.update.join(REFRESH_ERROR))? {
        let refresh = refresh_installer(paths, apt);
        if !request {
            return refresh;
        }
        if let Err(reason) = refresh {
            crate::log(&reason);
        }
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
    let mut failure_phase = InstallPhase::Failed;
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
        let package = apt.package()?;
        if package
            .as_ref()
            .is_some_and(PackageStatus::needs_manual_repair)
        {
            failure_phase = InstallPhase::NeedsManualRepair;
            return Err(update::APT_MANUAL_REPAIR.into());
        }
        if let Some(prior) = paths.transaction(INSTALL_STATUS)? {
            if now_seconds().saturating_sub(prior.updated_at) < COOLDOWN.as_secs() {
                return Err("agent updates are limited to one attempt every five minutes".into());
            }
        }
        let package = package.ok_or("GameLink apt package is not configured")?;
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
        sync_dir(&paths.update).map_err(|err| err.to_string())?;
        Ok((request, previous, swap_lock))
    })();
    let (request, previous, _swap_lock) = match prepare {
        Ok(prepared) => prepared,
        Err(reason) => {
            terminal(
                paths,
                owner,
                apt,
                tx,
                failure_phase,
                Some(String::clone(&reason)),
            )?;
            return Err(reason);
        }
    };
    tx.save(paths, InstallPhase::Stopping, &|_| {})?;
    let install = apt.install("target.deb");
    let target = deb_version(&request.version);
    let package = apt.package()?;
    let target_configured = package
        .as_ref()
        .is_some_and(|package| package.version == target && package.configured());
    let result = install.and_then(|()| {
        if !target_configured {
            return Err("requested GameLink apt version was not installed".into());
        }
        tx.save(paths, InstallPhase::HealthChecking, &|_| {})?;
        service.health(&paths.installed, &request.version)
    });
    if let Err(reason) = result {
        // The frontend lock still covers this install and any downgrade.
        let (phase, reason) = if package.is_some_and(|package| {
            package.can_roll_back()
                && (package.version == target
                    || (package.version == previous.package && !package.configured()))
        }) {
            let restored = apt
                .install("previous.deb")
                .and_then(|()| restore_service(paths, apt, service, &previous));
            rollback_outcome(restored, reason)
        } else {
            (InstallPhase::Failed, reason)
        };
        terminal(paths, owner, apt, tx, phase, Some(reason.clone()))?;
        return Err(reason);
    }
    terminal(paths, owner, apt, tx, InstallPhase::Committed, None)
}

fn dpkg_lock() -> Result<File> {
    // apt/dpkg use POSIX record locks, not flock. Hold the frontend lock
    // across installation, health checks and any downgrade.
    let frontend = take_dpkg_lock("/var/lib/dpkg/lock-frontend")?;
    // An interrupted helper can leave its dpkg child running after the
    // frontend lock is released. Wait for the child before reading state.
    drop(take_dpkg_lock("/var/lib/dpkg/lock")?);
    Ok(frontend)
}

fn take_dpkg_lock(path: &str) -> Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .map_err(|err| format!("could not open apt lock {path}: {err}"))?;
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
            if matches!(program, "apt-get" | "apt-cache") {
                let (uid, gid) = sandbox_account()?;
                let config = dir
                    .parent()
                    .expect("apt archives have a parent")
                    .join("apt.conf");
                command
                    .uid(uid)
                    .gid(gid)
                    .env_clear()
                    .env("PATH", "/usr/sbin:/usr/bin:/sbin:/bin")
                    .env("APT_CONFIG", config)
                    .stdin(Stdio::null());
                if program == "apt-cache" {
                    let output = command.output().map_err(|err| err.to_string())?;
                    if !output.status.success() {
                        return Err(output.status.to_string());
                    }
                    return String::from_utf8(output.stdout).map_err(|err| err.to_string());
                }
                let status = command.status().map_err(|err| err.to_string())?;
                return if status.success() {
                    Ok(String::new())
                } else {
                    Err(status.to_string())
                };
            }
            if program == "deb-systemd-invoke"
                || (program == "dpkg" && args != ["--print-architecture"])
            {
                // These commands may emit substantial progress output.
                let status = command
                    .env_clear()
                    .env("PATH", "/usr/sbin:/usr/bin:/sbin:/bin")
                    .env("DEBIAN_FRONTEND", "noninteractive")
                    .env("DPKG_FRONTEND_LOCKED", "true")
                    .stdin(Stdio::null())
                    .status()
                    .map_err(|err| err.to_string())?;
                if !status.success() {
                    return Err(status.to_string());
                }
                return Ok(String::new());
            }
            if program == "dpkg-query" {
                command.env_clear().stdin(Stdio::null());
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
