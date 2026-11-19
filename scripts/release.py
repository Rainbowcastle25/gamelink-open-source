#!/usr/bin/env python3
"""Stage unsigned GameLink release assets, then sign and verify them."""

import argparse
import base64
from datetime import datetime, timezone
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tarfile
import tomllib

from release_tools import bundle_environment, install_signers, sha256, verify_signers


ROOT = Path(__file__).resolve().parents[1]
DESKTOP = ROOT / "apps/desktop-tauri"
KEYS = ROOT / "release/keys"
SECRET_NAMES = (
    "GAMELINK_RELEASE_SIGNING_KEY",
    "TAURI_SIGNING_PRIVATE_KEY",
    "TAURI_SIGNING_PRIVATE_KEY_PASSWORD",
)
OTHER_TAURI_SIGNING_VARS = (
    "TAURI_SIGNING_PRIVATE_KEY_PATH",
    "TAURI_PRIVATE_KEY",
    "TAURI_PRIVATE_KEY_PATH",
    "TAURI_PRIVATE_KEY_PASSWORD",
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


def load_pins(directory):
    pins = read_key(directory / "manifest.pub").split()
    tauri_public = read_key(directory / "tauri.pub")
    tauri_lines = base64.b64decode(tauri_public, validate=True).decode().splitlines()
    if len(tauri_lines) != 2 or not all(valid_minisign_pin(pin) for pin in [*pins, tauri_lines[1]]):
        raise ValueError("invalid Minisign public key in the release pins")
    if tauri_lines[1] in pins:
        raise ValueError("use distinct signing keys for GameLink manifests and Tauri artifacts")
    return pins, tauri_public, tauri_lines[1]


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


def throwaway_keys(work, tools, env):
    manifest_key = work / "manifest.key"
    tauri_key = work / "tauri.key"
    manifest_public = work / "manifest.pub"
    run(tools / "minisign", "-G", "-W", "-s", manifest_key, "-p", manifest_public, env=env)
    run(tools / "cargo-tauri", "signer", "generate", "--ci", "--write-keys", tauri_key, env=env)
    manifest_public.write_text(read_key(manifest_public).splitlines()[1] + "\n")
    tauri_key.with_suffix(".key.pub").rename(work / "tauri.pub")
    return manifest_key.read_text(), tauri_key.read_text(), ""


def bundles(version, linux_only):
    yield "linux-x86_64", "x86_64-unknown-linux-gnu", "appimage", f"GameLink_{version}_amd64.AppImage"
    if not linux_only:
        yield "windows-x86_64", "x86_64-pc-windows-gnu", "nsis", f"GameLink_{version}_x64-setup.exe"


def agent_arches(linux_only):
    return ("amd64",) if linux_only else ("amd64", "arm64")


def agent_name(arch):
    return f"gamelink-agent-linux-{arch}"


def build_release(version, linux_only, work, out, tools, pins, env):
    out.mkdir(parents=True, exist_ok=True)
    if any(out.iterdir()):
        raise ValueError(f"release output directory must be empty: {out}")

    manifest_pins, tauri_public, _ = pins
    build_env = env | {"GAMELINK_UPDATE_PUBLIC_KEYS": " ".join(manifest_pins)}
    config = work / "tauri-release.json"
    write_json(config, {"plugins": {"updater": {"pubkey": tauri_public}}})

    bundle_env = bundle_environment(work, build_env, windows=not linux_only)
    for arch in agent_arches(linux_only):
        run("bash", "scripts/build-agent.sh", arch, out / agent_name(arch), env=build_env)

    for _, target, bundle, name in bundles(version, linux_only):
        run("bash", "scripts/build-desktop.sh", target,
            env=build_env | {"TAURI_CONFIG": config.read_text()})
        release_dir = DESKTOP / "target" / target / "release"
        if bundle == "nsis":
            # The GNU NSIS bundler includes this DLL next to the executable.
            shutil.copy2(ROOT / "dist/desktop" / target / "WebView2Loader.dll", release_dir)
        run(tools / "cargo-tauri", "bundle", "--ci", "--no-sign", "--target", target,
            "--bundles", bundle, "--config", config, cwd=DESKTOP, env=bundle_env)
        shutil.copy2(release_dir / "bundle" / bundle / name, out)

    with tarfile.open(out / "GameLink-licenses.tar.gz", "w:gz") as archive:
        for path in ("LICENSE", "THIRD_PARTY_NOTICES.md", "third_party/licenses"):
            archive.add(ROOT / path, arcname=path)
    print(f"Staged unsigned release assets: {out}", flush=True)


def verify_minisign(minisign, path, signature, pins, env):
    for pin in pins:
        result = subprocess.run(
            (minisign, "-Vq", "-m", path, "-x", signature, "-P", pin),
            cwd=ROOT, env=env, capture_output=True,
        )
        if result.returncode == 0:
            print(f"Verified {path.name} against its pinned public key", flush=True)
            return
    raise ValueError(f"{path.name} signature does not verify against the pinned public keys")


def sign_tauri(artifact, version, work, tools, tauri_pin, tauri_key, password, env):
    run(tools / "cargo-tauri", "signer", "sign", "--app-version", version, artifact, env=env | {
        "TAURI_SIGNING_PRIVATE_KEY": tauri_key.strip(),
        "TAURI_SIGNING_PRIVATE_KEY_PASSWORD": password,
    })
    signature_text = artifact.with_name(f"{artifact.name}.sig").read_text().strip()
    decoded = base64.b64decode(signature_text, validate=True)
    detached = work / f"{artifact.name}.minisig"
    detached.write_bytes(decoded)
    verify_minisign(tools / "minisign", artifact, detached, [tauri_pin], env)
    trusted_comment = decoded.decode().splitlines()[2]
    if f"version:{version}" not in trusted_comment.split("\t"):
        raise ValueError(f"{artifact.name} updater signature does not bind version {version}")
    return signature_text


def sign_release(version, severity, linux_only, work, out, tools, pins, keys, repository, env):
    verify_signers(tools)
    installers = {platform: out / name for platform, _, _, name in bundles(version, linux_only)}
    agents = [out / agent_name(arch) for arch in agent_arches(linux_only)]
    payloads = sorted([*installers.values(), *agents, out / "GameLink-licenses.tar.gz"])
    if set(out.iterdir()) != set(payloads) or any(
        path.is_symlink() or not path.is_file() or path.stat().st_size == 0 for path in payloads
    ):
        raise ValueError(f"expected only the complete set of unsigned release assets in {out}")

    manifest_key, tauri_key, password = keys
    manifest_pins, _, tauri_pin = pins
    releases = f"https://github.com/{repository}/releases"
    platforms = {}
    for platform, artifact in installers.items():
        signature = sign_tauri(artifact, version, work, tools, tauri_pin, tauri_key, password, env)
        platforms[platform] = {
            "url": f"{releases}/download/v{version}/{artifact.name}",
            "signature": signature,
        }

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
    # Send the private key on stdin. Do not write it to disk or put it in argv.
    minisign = tools / "minisign"
    run(minisign, "-S", "-s", "/dev/stdin", "-m", manifest, *agents,
        input=manifest_key, text=True, env=env)
    for path in [manifest, *agents]:
        verify_minisign(minisign, path, path.with_name(f"{path.name}.minisig"), manifest_pins, env)

    (out / "SHA256SUMS").write_text(
        "".join(f"{sha256(path)}  {path.name}\n" for path in sorted(out.iterdir())),
        encoding="ascii",
    )
    run("sha256sum", "--check", "SHA256SUMS", cwd=out, env=env)
    print(f"Verified release assets: {out}", flush=True)


def prepare_release(args):
    env = {
        name: value for name, value in os.environ.items()
        if name not in SECRET_NAMES + OTHER_TAURI_SIGNING_VARS
    }
    version, severity = release_metadata(args.tag, env)
    scratch = ROOT / ".tools"
    scratch.mkdir(exist_ok=True)
    work = Path(run("mktemp", "-d", scratch / "release.XXXXXXXX",
                    env=env, capture_output=True, text=True).stdout.strip())
    env["TMPDIR"] = str(work)
    out = args.output.resolve()
    tools = work / "signers" if args.validate else args.tools.resolve()
    if args.validate:
        install_signers(tools, work, env)
        keys = throwaway_keys(work, tools, env)
        pins = load_pins(work)
    else:
        pins = load_pins(KEYS)
        if args.sign:
            keys = tuple(os.environ.get(name) for name in SECRET_NAMES)
            if missing := [name for name, value in zip(SECRET_NAMES, keys) if not value]:
                raise ValueError(f"missing release secrets: {', '.join(missing)}")
        else:
            install_signers(tools, work, env)
    if args.build or args.validate:
        build_release(version, args.validate, work, out, tools, pins, env)
    if args.sign or args.validate:
        sign_release(version, severity, args.validate, work, out, tools, pins, keys,
                     args.repository, env)
    if args.validate:
        run("cargo", "test", "--manifest-path", "core/Cargo.toml", "--locked", "-p", "gamelink-core",
            "--features", "update-notifications", "--lib",
            "update::tests::verifies_release_pipeline_manifest", "--", "--ignored", "--exact",
            env=env | {
                "GAMELINK_UPDATE_PUBLIC_KEYS": " ".join(pins[0]),
                "GAMELINK_TEST_SIGNED_RELEASE_DIR": str(out),
            })


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument("--build", action="store_true", help="stage unsigned release assets and pinned signers")
    mode.add_argument("--sign", action="store_true", help="sign and verify staged release assets")
    mode.add_argument("--validate", action="store_true", help="build and sign Linux amd64 with throwaway keys")
    parser.add_argument("--tag", help="checked-out release tag (required for --build and --sign)")
    parser.add_argument("--repository", default="Rainbowcastle25/gamelink-open-source")
    parser.add_argument("--output", type=Path, default=ROOT / "dist/release")
    parser.add_argument("--tools", type=Path, default=ROOT / "dist/release-tools")
    args = parser.parse_args()
    if args.validate == bool(args.tag):
        parser.error("provide --tag for --build or --sign only")
    try:
        prepare_release(args)
    except (ValueError, OSError, subprocess.CalledProcessError) as error:
        sys.exit(f"release: {error}")


if __name__ == "__main__":
    main()
