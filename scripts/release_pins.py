"""Inspect the binaries inside unsigned release assets without running them."""

import re
import struct
import subprocess


# The Rust verifier consumes the base64 Ed public keys as ASCII. Look for
# every such record so an extra key (including during rotation) fails the check.
MANIFEST_PIN = re.compile(rb"RW[QRST][A-Za-z0-9+/]{53}")
# Tauri codegen embeds the base64 updater public file as a string literal.
TAURI_PIN = re.compile(
    rb"dW50cnVzdGVkIGNvbW1lbnQ6[A-Za-z0-9+/]{1,200}={0,2}(?=requireSignedVersion)"
)


def check_binary_pins(binary, name, pins, desktop=False):
    manifest, tauri, _ = pins
    if set(MANIFEST_PIN.findall(binary)) != {pin.encode("ascii") for pin in manifest}:
        raise ValueError(f"{name}: embedded manifest pins do not match the selected pins")
    if desktop:
        # Tauri's literals may be adjacent in the executable without NUL
        # padding, so use the next updater config field to bound the value.
        updater_pins = TAURI_PIN.findall(binary)
        if set(updater_pins) != {tauri.encode("ascii")}:
            raise ValueError(f"{name}: embedded Tauri updater pin does not match the selected pin")
    print(f"Verified embedded pins: {name}", flush=True)


def appimage_offset(path):
    # Type 2 x86_64 AppImages append SquashFS just after the ELF section table.
    with path.open("rb") as file:
        header = file.read(64)
        if len(header) != 64 or header[:6] != b"\x7fELF\x02\x01" or header[8:11] != b"AI\x02":
            raise ValueError(f"{path.name}: expected an ELF64 Type 2 AppImage")
        table = struct.unpack_from("<Q", header, 40)[0]
        entry_size, entry_count = struct.unpack_from("<HH", header, 58)
        offset = table + entry_size * entry_count
        if not 64 <= offset <= path.stat().st_size - 4:
            raise ValueError(f"{path.name}: invalid SquashFS offset")
        file.seek(offset)
        if file.read(4) != b"hsqs":
            raise ValueError(f"{path.name}: missing SquashFS filesystem")
    return offset


def check_bundle_pins(bundle, path, work, pins, env):
    if bundle == "appimage":
        command = ("unsquashfs", "-offset", str(appimage_offset(path)), "-cat",
                   path, "usr/bin/gamelink-desktop")
        magic = b"\x7fELF"
    else:
        command = ("7z", "x", "-so", "-spd", "-tNsis", path, "gamelink-desktop.exe")
        magic = b"MZ"

    # Stream only the exact executable to a file we own. The archive never
    # chooses an output path and the executable is never launched.
    payload = work / "desktop-payload"
    with payload.open("wb") as file:
        subprocess.run(command, check=True, stdout=file, env=env)
    if payload.stat().st_size > 256 * 1024 * 1024:
        raise ValueError(f"{path.name}: oversized desktop payload")
    binary = payload.read_bytes()
    if not binary.startswith(magic):
        raise ValueError(f"{path.name}: missing desktop executable")
    check_binary_pins(binary, path.name, pins, desktop=True)
