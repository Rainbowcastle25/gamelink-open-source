#!/usr/bin/env python3
"""Build an apt repository from published, Minisign-verified GameLink releases."""

import argparse
import gzip
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys

from release import (KEYS, LICENSES_ASSET, NUMBER, ROOT, agent_name, load_manifest_pins,
                     run, sign_minisign, verify_minisign, write_json)
from release_deb import ARCHES, build_deb, check_deb_metadata, deb_name, deb_version
from release_tools import NFPM, SIGNERS, install_tool, require_digest, sha256


APT_SECRET = "GAMELINK_APT_SIGNING_KEY"
MANIFEST = "release-manifest.json"
STABLE_TAG = re.compile(rf"v{NUMBER}\.{NUMBER}\.{NUMBER}")


def scratch_directory():
    scratch = ROOT / ".tools"
    scratch.mkdir(exist_ok=True)
    return Path(run("mktemp", "-d", scratch / "apt-repo.XXXXXXXX",
                    capture_output=True, text=True).stdout.strip())


def gpg_home(work):
    home = work / "gnupg"
    home.mkdir(mode=0o700)
    return ("gpg", "--no-options", "--homedir", home, "--batch", "--no-tty")


def public_fingerprint(gpg, public_key):
    if not public_key.is_file():
        raise ValueError(f"missing apt public key: {public_key}")
    armor = public_key.read_bytes()
    if (not armor.startswith(b"-----BEGIN PGP PUBLIC KEY BLOCK-----")
            or not armor.rstrip().endswith(b"-----END PGP PUBLIC KEY BLOCK-----")):
        raise ValueError(f"invalid apt public key: {public_key}")
    result = run(*gpg, "--with-colons", "--show-keys", public_key, capture_output=True, text=True)
    records = [line.split(":") for line in result.stdout.splitlines()]
    primary = [record for record in records if record[0] == "pub"]
    fingerprint = next((record[9] for record in records if record[0] == "fpr"), "")
    if (len(primary) != 1 or primary[0][1] in ("e", "r", "d", "i")
            or "s" not in primary[0][11].lower()
            or not re.fullmatch(r"[A-F0-9]{40}", fingerprint)):
        raise ValueError(f"expected one usable apt signing public key: {public_key}")
    return fingerprint


def sign_repository(repo, work, public_key, private_key):
    gpg = gpg_home(work)
    fingerprint = public_fingerprint(gpg, public_key)
    if not private_key:
        raise ValueError(f"missing apt signing secret: {APT_SECRET}")
    if (repo / "apt.asc").read_bytes() != public_key.read_bytes():
        raise ValueError("apt repository public key differs from the committed key")
    release = repo / "dists/stable/Release"
    inrelease = work / "InRelease"
    detached = work / "Release.gpg"
    keyring = work / "apt.gpg"
    run(*gpg, "--dearmor", "--output", keyring, public_key)
    # Import only into this job's private home and select the committed key
    # explicitly. The secret never appears in argv or a repository file.
    run(*gpg, "--quiet", "--import", input=private_key, text=True)
    signer = (*gpg, "--local-user", fingerprint, "--pinentry-mode", "loopback", "--passphrase", "",
              "--digest-algo", "SHA256")
    run(*signer, "--output", inrelease, "--clearsign", release)
    run(*signer, "--output", detached, "--armor", "--detach-sign", release)
    verified_release = work / "verified-Release"
    run("gpgv", "--keyring", keyring, "--output", verified_release, inrelease)
    run("gpgv", "--keyring", keyring, detached, release)
    if verified_release.read_bytes() != release.read_bytes():
        raise ValueError("InRelease content differs from Release")
    shutil.move(inrelease, release.with_name("InRelease"))
    shutil.move(detached, release.with_suffix(".gpg"))


def signed_package_digests(path, version, minisign, pins, env):
    signature = path.with_name(f"{path.name}.minisig")
    if path.stat().st_size > 64 * 1024 or signature.stat().st_size > 4 * 1024:
        raise ValueError(f"{path}: oversized manifest or signature")
    verify_minisign(minisign, path, signature, pins, env)
    manifest = json.loads(path.read_bytes())
    if manifest.get("schemaVersion") != 1 or manifest.get("version") != version:
        raise ValueError(f"{path}: signed manifest does not match v{version}")
    names = {deb_name(version, arch) for arch in ARCHES}
    digests = {}
    for entry in manifest["artifacts"]:
        name = entry["name"]
        if name.endswith(".deb"):
            digest = entry["sha256"]
            if name not in names or name in digests or not re.fullmatch(r"[0-9a-f]{64}", digest):
                raise ValueError(f"{path}: invalid Debian asset in signed manifest")
            digests[name] = digest
    if digests and set(digests) != names:
        raise ValueError(f"{path}: incomplete set of Debian assets in signed manifest")
    return digests


def verify_packages(directory, version, digests, minisign, pins, env):
    for arch in ARCHES:
        path = directory / deb_name(version, arch)
        require_digest(path, digests[path.name])
        verify_minisign(minisign, path, path.with_name(f"{path.name}.minisig"), pins, env)
        check_deb_metadata(path, version, arch, env)
        yield arch, path


def published_packages(repository, work, minisign, pins, env):
    result = run(
        "gh", "api", "--paginate", f"repos/{repository}/releases", "--jq",
        ".[] | select(.draft == false and .prerelease == false) | .tag_name",
        capture_output=True, text=True, env=env,
    )
    versions = sorted(
        {tag[1:] for tag in result.stdout.splitlines() if STABLE_TAG.fullmatch(tag)},
        key=lambda version: tuple(map(int, version.split("."))), reverse=True,
    )
    if not versions:
        raise ValueError("no published stable releases")
    for version in versions[:2]:
        target = work / version
        target.mkdir()
        command = ("gh", "release", "download", f"v{version}", "--repo", repository, "--dir", target)
        run(*command, "--pattern", MANIFEST, "--pattern", f"{MANIFEST}.minisig", env=env)
        digests = signed_package_digests(target / MANIFEST, version, minisign, pins, env)
        if not digests:
            if version == versions[0]:
                raise ValueError(f"current stable release v{version} has no signed Debian assets")
            print(f"Previous release v{version} predates Debian packages", flush=True)
            continue
        patterns = [arg for name in sorted(digests) for arg in ("--pattern", name, "--pattern", f"{name}.minisig")]
        run(*command, *patterns, env=env)
        yield from verify_packages(target, version, digests, minisign, pins, env)


def build_repository(repo, packages, public_key, env):
    repo.mkdir(parents=True, exist_ok=True)
    if any(repo.iterdir()):
        raise ValueError(f"apt repository directory must be empty: {repo}")
    shutil.copyfile(public_key, repo / "apt.asc")
    for arch, package in packages:
        pool = repo / "pool/main" / arch
        pool.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(package, pool / package.name)
    for arch in ARCHES:
        pool = Path("pool/main") / arch
        index = repo / "dists/stable/main" / f"binary-{arch}" / "Packages"
        index.parent.mkdir(parents=True)
        with index.open("wb") as output:
            run("apt-ftparchive", "packages", pool, cwd=repo, env=env, stdout=output)
        if not index.stat().st_size:
            raise ValueError(f"empty apt index for {arch}")
        index.with_suffix(".gz").write_bytes(gzip.compress(index.read_bytes(), mtime=0))
    release = Path("dists/stable/Release")
    # Keep the output outside dists/ until apt-ftparchive finishes scanning it.
    staged_release = repo / "Release"
    with staged_release.open("wb") as output:
        run("apt-ftparchive", "-o", "APT::FTPArchive::Release::Origin=GameLink",
            "-o", "APT::FTPArchive::Release::Label=GameLink",
            "-o", "APT::FTPArchive::Release::Suite=stable",
            "-o", "APT::FTPArchive::Release::Codename=stable",
            "-o", "APT::FTPArchive::Release::Architectures=amd64 arm64",
            "-o", "APT::FTPArchive::Release::Components=main",
            "release", release.parent, cwd=repo, env=env, stdout=output)
    staged_release.rename(repo / release)


def validate_repository(version, assets, work, minisign, manifest_key, pins, env):
    # Model a previously installed Debian version with the same throwaway
    # agent binaries, then upgrade to the release's real package version.
    previous = "0.0.0-0"
    run("dpkg", "--compare-versions", deb_version(previous), "lt", deb_version(version), env=env)
    old_assets = work / "old-apt-assets"
    old_assets.mkdir()
    old_debs = [old_assets / deb_name(previous, arch) for arch in ARCHES]
    for arch, package in zip(ARCHES, old_debs, strict=True):
        build_deb(work / "nfpm", assets / agent_name(arch), assets / LICENSES_ASSET,
                  work, previous, arch, package, env)
    old_manifest = old_assets / MANIFEST
    write_json(old_manifest, {
        "schemaVersion": 1, "version": previous,
        "artifacts": [{"name": path.name, "sha256": sha256(path)} for path in old_debs],
    })
    sign_minisign(minisign, [old_manifest, *old_debs], manifest_key, pins, env)

    gpg = gpg_home(work)
    identity = "GameLink apt CI <apt-ci@example.invalid>"
    run(*gpg, "--pinentry-mode", "loopback", "--passphrase", "", "--quick-generate-key",
        identity, "ed25519", "sign", "1d", env=env)
    test_root = work / "apt-test"
    test_root.mkdir()
    public_key = test_root / "apt.asc"
    run(*gpg, "--armor", "--output", public_key, "--export", identity, env=env)
    private_key = run(*gpg, "--armor", "--export-secret-keys", identity, env=env,
                      capture_output=True, text=True).stdout
    packages = []
    for directory, release_version, repo_name in (
        (old_assets, previous, "first"), (assets, version, "upgrade"),
    ):
        digests = signed_package_digests(directory / MANIFEST, release_version, minisign, pins, env)
        packages.extend(verify_packages(directory, release_version, digests, minisign, pins, env))
        repo = test_root / repo_name
        build_repository(repo, packages, public_key, env)
        signing_work = work / repo_name
        signing_work.mkdir(mode=0o700)
        sign_repository(repo, signing_work, public_key, private_key)
    keyring = test_root / "apt.gpg"
    run(*gpg, "--dearmor", "--output", keyring, public_key, env=env)
    runtime = shutil.which("docker") or shutil.which("podman")
    if runtime is None:
        raise ValueError("apt install validation requires Docker or Podman")
    run(runtime, "run", "--network", "none",
        "--mount", f"type=bind,source={test_root},target=/proof,readonly",
        "--mount", f"type=bind,source={ROOT / 'scripts/test-apt-install.sh'},target=/test-apt-install.sh,readonly",
        "debian:bookworm-slim", "sh", "/test-apt-install.sh",
        deb_version(previous), deb_version(version), version, env=env)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument("--build", metavar="GITHUB_REPOSITORY")
    mode.add_argument("--sign", action="store_true")
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    private_key = os.environ.pop(APT_SECRET, "")
    work = scratch_directory()
    repo = args.output.resolve()
    public_key = KEYS / "apt.asc"
    try:
        if args.sign:
            sign_repository(repo, work, public_key, private_key)
        else:
            public_fingerprint(gpg_home(work), public_key)
            pins = load_manifest_pins(KEYS)
            minisign = install_tool("minisign", SIGNERS["minisign"], work, work, os.environ)
            packages = list(published_packages(args.build, work, minisign, pins, os.environ))
            build_repository(repo, packages, public_key, os.environ)
    except (ValueError, OSError, subprocess.CalledProcessError, KeyError, TypeError) as error:
        sys.exit(f"apt repo: {error}")


if __name__ == "__main__":
    main()
