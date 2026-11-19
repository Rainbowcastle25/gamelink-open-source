"""Build Debian packages from the same binaries as the raw release assets."""

import hashlib
import json
from pathlib import Path
import posixpath
import subprocess
import tarfile

from release_pins import check_binary_pins
from release_tools import sha256


ROOT = Path(__file__).resolve().parents[1]
PACKAGING = ROOT / "packaging/debian"
PACKAGE = "gamelink-agent"
ARCHES = ("amd64", "arm64")
EXECUTABLE = "usr/bin/gamelink-agent"
ELF_MACHINE = {"amd64": b"\x3e\x00", "arm64": b"\xb7\x00"}


def deb_version(version):
    stable, separator, prerelease = version.partition("-")
    return f"{stable}~{prerelease}" if separator else stable


def deb_name(version, arch):
    return f"{PACKAGE}_{version}_{arch}.deb"


def build_deb(nfpm, binary, licenses, work, version, arch, package, env):
    unit = work / "gamelink-agent.service"
    example = (ROOT / "config/examples/gamelink-agent.service").read_text()
    header = "[Unit]\n"
    unit.write_text(header + example.split(header, 1)[1].replace(
        "/usr/local/bin/gamelink-agent", "/usr/bin/gamelink-agent",
    ))
    contents = [
        (binary, f"/{EXECUTABLE}", 0o755),
        (unit, "/lib/systemd/system/gamelink-agent.service", 0o644),
        (PACKAGING / "copyright", "/usr/share/doc/gamelink-agent/copyright", 0o644),
        (licenses, "/usr/share/doc/gamelink-agent/GameLink-licenses.tar.gz", 0o644),
    ]
    config = work / "nfpm.json"
    config.write_text(json.dumps({
        "name": PACKAGE,
        "arch": arch,
        "version": deb_version(version),
        "version_schema": "none",
        "section": "net",
        "priority": "optional",
        "maintainer": "GameLink maintainers <169835201+Rainbowcastle25@users.noreply.github.com>",
        "description": "GameLink server agent for private game tunnels",
        "homepage": "https://github.com/Rainbowcastle25/gamelink-open-source",
        "license": "GPL-3.0",
        "depends": ["init-system-helpers", "passwd"],
        "contents": [
            {"src": str(src), "dst": dst, "file_info": {"mode": mode}}
            for src, dst, mode in contents
        ],
        "scripts": {
            "preremove": str(PACKAGING / "prerm"),
            "postinstall": str(PACKAGING / "postinst"),
            "postremove": str(PACKAGING / "postrm"),
        },
    }))
    subprocess.run((nfpm, "package", "--packager", "deb", "--config", config,
                    "--target", package), env=env, check=True)


def check_deb_metadata(path, version, arch, env):
    expected = {f"Package: {PACKAGE}", f"Version: {deb_version(version)}", f"Architecture: {arch}"}
    result = subprocess.run(
        ("dpkg-deb", "--field", path, "Package", "Version", "Architecture"),
        env=env, check=True, capture_output=True, text=True,
    )
    if set(result.stdout.splitlines()) != expected:
        raise ValueError(f"{path.name}: unexpected Debian package metadata")


def check_deb_pins(path, raw_binary, version, arch, pins, env):
    check_deb_metadata(path, version, arch, env)
    # Inspect the exact regular member. The package never chooses a destination
    # and its maintainer scripts do not run.
    binary = None
    seen = set()
    with subprocess.Popen(("dpkg-deb", "--fsys-tarfile", path),
                          env=env, stdout=subprocess.PIPE) as process:
        with tarfile.open(fileobj=process.stdout, mode="r|") as archive:
            for member in archive:
                name = member.name.removeprefix("./")
                if (name != posixpath.normpath(name) or name.startswith(("/", "../"))
                        or name == ".." or name in seen or not (member.isdir() or member.isreg())):
                    raise ValueError(f"{path.name}: invalid Debian payload path {member.name!r}")
                seen.add(name)
                if name != EXECUTABLE:
                    continue
                if not member.isreg() or member.size > 64 * 1024 * 1024:
                    raise ValueError(f"{path.name}: invalid agent executable member")
                binary = archive.extractfile(member).read()
        # Drain any trailing tar padding before waiting for the producer.
        while process.stdout.read(64 * 1024):
            pass
        if process.wait() != 0:
            raise ValueError(f"{path.name}: cannot read Debian package payload")
    if (binary is None or not binary.startswith(b"\x7fELF\x02\x01")
            or binary[18:20] != ELF_MACHINE[arch]):
        raise ValueError(f"{path.name}: expected a Linux agent executable for {arch}")
    check_binary_pins(binary, path.name, pins)
    if hashlib.sha256(binary).hexdigest() != sha256(raw_binary):
        raise ValueError(f"{path.name}: packaged agent differs from {raw_binary.name}")
