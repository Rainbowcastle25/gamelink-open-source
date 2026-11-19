#!/usr/bin/env python3
"""Build, sign and verify a GameLink release without contacting GitHub Releases."""

import argparse
import base64
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tarfile
import tomllib


ROOT = Path(__file__).resolve().parents[1]
DESKTOP = ROOT / "apps/desktop-tauri"
TAURI = ("npx", "--yes", "@tauri-apps/cli@2.12.4")
SECRET_NAMES = (
    "GAMELINK_RELEASE_SIGNING_KEY",
    "TAURI_SIGNING_PRIVATE_KEY",
    "TAURI_SIGNING_PRIVATE_KEY_PASSWORD",
)
NUMBER = r"(?:0|[1-9][0-9]*)"
PRERELEASE = rf"(?:{NUMBER}|[0-9]*[A-Za-z-][0-9A-Za-z-]*)"
VERSION = re.compile(rf"{NUMBER}\.{NUMBER}\.{NUMBER}(?:-{PRERELEASE}(?:\.{PRERELEASE})*)?")


def run(*command, env=None, cwd=ROOT, **options):
    return subprocess.run(command, cwd=cwd, env=env, check=True, **options)


def read_key(path):
    if not path.is_file() or not (key := path.read_text().strip()):
        raise ValueError(f"missing public key: {path}; see docs/release-manifest.md")
    return key


def valid_minisign_pin(pin):
    raw = base64.b64decode(pin, validate=True)
    return len(raw) == 42 and raw[:2] == b"Ed"


def write_json(path, data):
    path.write_text(json.dumps(data, indent=2) + "\n", encoding="utf-8")


def release_metadata(tag, env):
    def version(path):
        return tomllib.loads(path.read_text())["package"]["version"]

    desktop = version(DESKTOP / "Cargo.toml")
    agent = version(ROOT / "core/crates/gamelink-agent/Cargo.toml")
    if not VERSION.fullmatch(desktop) or agent != desktop:
        raise ValueError("desktop and agent must have the same SemVer without build metadata")
    if tag is None:
        return desktop, "normal"
    if tag != f"v{desktop}":
        raise ValueError(f"release tag must match the desktop and agent versions: v{desktop}")

    ref = f"refs/tags/{tag}"
    tagged_commit = run(
        "git", "rev-parse", "--verify", f"{ref}^{{commit}}", env=env, capture_output=True,
    ).stdout
    head = run("git", "rev-parse", "HEAD", env=env, capture_output=True).stdout
    if tagged_commit != head:
        raise ValueError(f"{tag} does not point to the checked-out commit")
    severity = run(
        "git", "for-each-ref",
        "--format=%(trailers:key=GameLink-Release-Severity,valueonly)",
        ref, env=env, capture_output=True, text=True,
    ).stdout.strip() or "normal"
    if severity not in ("normal", "security"):
        raise ValueError("GameLink-Release-Severity must be normal or security")
    return desktop, severity


def release_keys(validate, work, build_env):
    if validate:
        manifest_key = work / "manifest.key"
        tauri_key = work / "tauri.key"
        run("minisign", "-G", "-W", "-s", manifest_key, "-p", work / "manifest.pub", env=build_env)
        run(*TAURI, "signer", "generate", "--ci", "--write-keys", tauri_key, env=build_env)
        pins = read_key(work / "manifest.pub").splitlines()[1]
        tauri_public = read_key(work / "tauri.key.pub")
        secrets = (manifest_key.read_text(), tauri_key.read_text(), "")
    else:
        secrets = tuple(os.environ.get(name) for name in SECRET_NAMES)
        missing = [name for name, value in zip(SECRET_NAMES, secrets) if not value]
        if missing:
            raise ValueError(f"missing release secrets: {', '.join(missing)}")
        pins = read_key(ROOT / "release/keys/manifest.pub")
        tauri_public = read_key(ROOT / "release/keys/tauri.pub")

    tauri_pin = base64.b64decode(tauri_public, validate=True).decode().splitlines()[-1]
    if not all(valid_minisign_pin(pin) for pin in [*pins.split(), tauri_pin]):
        raise ValueError("invalid Minisign public key in the release pins")
    if tauri_pin in pins.split():
        raise ValueError("use distinct signing keys for GameLink manifests and Tauri artifacts")
    build_env["GAMELINK_UPDATE_PUBLIC_KEYS"] = " ".join(pins.split())
    write_json(work / "tauri-release.json", {"plugins": {"updater": {"pubkey": tauri_public}}})
    return secrets, tauri_pin


def desktop_bundle(target, bundle, name, work, out, build_env, signing_env, public_key, version):
    config = work / "tauri-release.json"
    run("bash", "scripts/build-desktop.sh", target,
        env=build_env | {"TAURI_CONFIG": config.read_text()})
    release_dir = DESKTOP / "target" / target / "release"
    if bundle == "nsis":
        # Tauri's GNU NSIS bundler includes this DLL when it is next to the executable.
        shutil.copy2(ROOT / "dist/desktop" / target / "WebView2Loader.dll", release_dir)
    run(*TAURI, "bundle", "--ci", "--target", target, "--bundles", bundle,
        "--config", config, cwd=DESKTOP, env=signing_env)
    artifact = release_dir / "bundle" / bundle / name
    signature = artifact.with_name(f"{name}.sig")
    signature_text = signature.read_text().strip()
    decoded = base64.b64decode(signature_text, validate=True)
    verified_signature = work / f"{name}.minisig"
    verified_signature.write_bytes(decoded)
    run("minisign", "-Vq", "-m", artifact, "-x", verified_signature, "-P", public_key, env=build_env)
    if f"version:{version}" not in decoded.decode().splitlines()[2].split("\t"):
        raise ValueError(f"{name} updater signature does not bind version {version}")
    shutil.copy2(artifact, out)
    shutil.copy2(signature, out)
    return signature_text


def sha256(path):
    with path.open("rb") as file:
        return hashlib.file_digest(file, "sha256").hexdigest()


def build_release(args):
    build_env = {name: value for name, value in os.environ.items() if name not in SECRET_NAMES}
    version, severity = release_metadata(args.tag, build_env)
    work = Path(run("mktemp", "-d", env=build_env, capture_output=True, text=True).stdout.strip())
    out = args.output.resolve()
    out.mkdir(parents=True, exist_ok=True)
    if any(out.iterdir()):
        raise ValueError(f"release output directory must be empty: {out}")

    (manifest_key, tauri_key, password), tauri_pin = release_keys(args.validate, work, build_env)
    signing_env = build_env | {
        "TAURI_SIGNING_PRIVATE_KEY": tauri_key.strip(),
        "TAURI_SIGNING_PRIVATE_KEY_PASSWORD": password,
    }

    agents = []
    for arch in ("amd64",) if args.validate else ("amd64", "arm64"):
        agent = out / f"gamelink-agent-linux-{arch}"
        run("bash", "scripts/build-agent.sh", arch, agent, env=build_env)
        agents.append(agent)

    bundles = [
        ("linux-x86_64", "x86_64-unknown-linux-gnu", "appimage", f"GameLink_{version}_amd64.AppImage"),
    ]
    if not args.validate:
        bundles.append(
            ("windows-x86_64", "x86_64-pc-windows-gnu", "nsis", f"GameLink_{version}_x64-setup.exe")
        )
    releases = f"https://github.com/{args.repository}/releases"
    platforms = {}
    for platform, target, bundle, name in bundles:
        signature = desktop_bundle(
            target, bundle, name, work, out, build_env, signing_env, tauri_pin, version,
        )
        platforms[platform] = {
            "url": f"{releases}/download/v{version}/{name}",
            "signature": signature,
        }

    with tarfile.open(out / "GameLink-licenses.tar.gz", "w:gz") as archive:
        for path in ("LICENSE", "THIRD_PARTY_NOTICES.md", "third_party/licenses"):
            archive.add(ROOT / path, arcname=path)
    payloads = sorted(path for path in out.iterdir() if not path.name.endswith(".sig"))
    now = datetime.now(timezone.utc)
    manifest = out / "release-manifest.json"
    write_json(manifest, {
        "schemaVersion": 1,
        "version": version,
        "releaseDate": now.date().isoformat(),
        "severity": severity,
        "releaseNotesUrl": f"{releases}/tag/v{version}",
        "artifacts": [{"name": path.name, "sha256": sha256(path)} for path in payloads],
    })
    write_json(out / "latest.json", {
        "version": version,
        "pub_date": now.strftime("%Y-%m-%dT%H:%M:%SZ"),
        "platforms": platforms,
    })
    # Read the secret from stdin so the hosted runner doesn't need a private key file.
    run("minisign", "-S", "-s", "/dev/stdin", "-m", manifest, *agents,
        input=manifest_key, text=True, env=build_env)
    (out / "SHA256SUMS").write_text(
        "".join(f"{sha256(path)}  {path.name}\n" for path in sorted(out.iterdir())),
        encoding="ascii",
    )
    run("sha256sum", "--check", "SHA256SUMS", cwd=out, env=build_env)
    run("cargo", "test", "--manifest-path", "core/Cargo.toml", "--locked", "-p", "gamelink-core",
        "--features", "update-notifications", "--lib",
        "update::tests::verifies_release_pipeline_manifest", "--", "--ignored", "--exact",
        env=build_env | {"GAMELINK_TEST_SIGNED_RELEASE_DIR": str(out)})
    print(f"Verified release assets: {out}", flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument("--validate", action="store_true", help="use throwaway keys and build Linux amd64 only")
    mode.add_argument("--tag", help="build a signed release at the checked-out tag")
    parser.add_argument("--repository", default="Rainbowcastle25/gamelink-open-source")
    parser.add_argument("--output", type=Path, default=ROOT / "dist/release")
    args = parser.parse_args()
    try:
        build_release(args)
    except (ValueError, OSError, subprocess.CalledProcessError) as error:
        sys.exit(f"release: {error}")


if __name__ == "__main__":
    main()
