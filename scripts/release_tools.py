"""Fixed tools for the Linux x86_64 release runner and local validation."""

import hashlib
from pathlib import Path
import shutil
import subprocess
import tarfile
import zipfile


SIGNERS = {
    "cargo-tauri": (
        "https://github.com/tauri-apps/tauri/releases/download/tauri-cli-v2.12.4/cargo-tauri-x86_64-unknown-linux-gnu.tgz",
        "820ae3c7580ca637cad15f65638960ee2a96e5a491e224691c86d6f25e6c9e8a",
        "cargo-tauri",
        "c1578459b8597c49a333ae1b17675326e8fd2647de3ea818aef4bf4dca73508a",
    ),
    "minisign": (
        "https://github.com/jedisct1/minisign/releases/download/0.12/minisign-0.12-linux.tar.gz",
        "9a599b48ba6eb7b1e80f12f36b94ceca7c00b7a5173c95c3efc88d9822957e73",
        "minisign-linux/x86_64/minisign",
        "2c74dffcc1c9a5ee55957c60971998ace2b89f22585631594ec2152c588af8db",
    ),
}

# Cache names are part of the Tauri 2.12.4 bundler's interface. The fixed plugin
# AppImage includes appimagetool and mksquashfs. Supply its output runtime too.
APPIMAGE_TOOLS = (
    (
        "AppRun-x86_64",
        "https://github.com/tauri-apps/binary-releases/releases/download/apprun-old/AppRun-x86_64",
        "f30140a43a0a59e46db21bdefdf749b9e9f2c6946e92afabbacf98b8ae73fb4f",
    ),
    (
        "linuxdeploy-07333c6-x86_64.AppImage",
        "https://github.com/tauri-apps/binary-releases/releases/download/linuxdeploy-07333c6/linuxdeploy-x86_64.AppImage",
        "36a2d7e274d12e1050d0e9ecfe11d339ed54720b2bec464c286d53f8b07f5c62",
    ),
    (
        "linuxdeploy-plugin-appimage.AppImage",
        "https://github.com/linuxdeploy/linuxdeploy-plugin-appimage/releases/download/1-alpha-20250213-1/linuxdeploy-plugin-appimage-x86_64.AppImage",
        "992d502a248e14ab185448ddf6f6e7d25558cb84d4623c354c3af350c25fccb3",
    ),
    (
        "runtime-x86_64",
        "https://github.com/AppImage/type2-runtime/releases/download/20251108/runtime-x86_64",
        "2fca8b443c92510f1483a883f60061ad09b46b978b2631c807cd873a47ec260d",
    ),
)


def sha256(path):
    with path.open("rb") as file:
        return hashlib.file_digest(file, "sha256").hexdigest()


def require_digest(path, digest):
    if path.is_symlink() or not path.is_file() or sha256(path) != digest:
        raise ValueError(f"missing file or SHA-256 mismatch: {path}")


def download(url, digest, path, env):
    path.parent.mkdir(parents=True, exist_ok=True)
    subprocess.run(
        ("curl", "--fail", "--silent", "--show-error", "--location",
         "--proto", "=https", "--proto-redir", "=https", "--retry", "3", "--output", path, url),
        env=env, check=True,
    )
    require_digest(path, digest)


def install_signers(tools, work, env):
    tools.mkdir(parents=True, exist_ok=True)
    if any(tools.iterdir()):
        raise ValueError(f"release signing tool directory must be empty: {tools}")
    for name, (url, archive_digest, member, _) in SIGNERS.items():
        archive_path = work / Path(url).name
        download(url, archive_digest, archive_path, env)
        with tarfile.open(archive_path) as archive:
            with archive.extractfile(member) as source, (tools / name).open("xb") as dest:
                shutil.copyfileobj(source, dest)
    verify_signers(tools)


def verify_signers(tools):
    # Artifact transfer does not preserve executable bits. Set them only after
    # checking the executable bytes independently of the build runner.
    if tools.is_symlink() or {path.name for path in tools.iterdir()} != SIGNERS.keys():
        raise ValueError(f"expected only the pinned signing tools in {tools}")
    for name, (_, _, _, binary_digest) in SIGNERS.items():
        path = tools / name
        require_digest(path, binary_digest)
        path.chmod(0o700)


def prepare_nsis(cache, work, env):
    plugins = cache / "NSIS/Plugins/x86-unicode/additional"
    download(
        "https://github.com/tauri-apps/nsis-tauri-utils/releases/download/nsis_tauri_utils-v0.5.3/nsis_tauri_utils.dll",
        "5ba143b5db4a87d32d6e7802e033330aae56cbceabe0d1e3ba41948385ad4709",
        plugins / "nsis_tauri_utils.dll", env,
    )
    # Even when using the distribution's Linux makensis, Tauri probes NSIS.exe
    # to determine whether to recreate its tool cache and download the plugin.
    archive_path = work / "nsis-3.13.zip"
    download(
        "https://github.com/tauri-apps/binary-releases/releases/download/nsis-3.13/nsis-3.13.zip",
        "ba63dffc4410ee89193e1cb5a41989991bd77c61068da17e3156d136b7b0b3d8",
        archive_path, env,
    )
    with zipfile.ZipFile(archive_path) as archive:
        (cache / "NSIS/NSIS.exe").write_bytes(archive.read("nsis-3.13/NSIS.exe"))


def bundle_environment(work, env, windows):
    cache = work / "cache/tauri"
    for name, url, digest in APPIMAGE_TOOLS:
        path = cache / name
        download(url, digest, path, env)
        path.chmod(0o700)
    if windows:
        prepare_nsis(cache, work, env)
    return env | {
        "XDG_CACHE_HOME": str(cache.parent),
        "LDAI_RUNTIME_FILE": str(cache / "runtime-x86_64"),
        # Block the bundler's GitHub download fallback if its cache layout changes.
        "TAURI_BUNDLER_TOOLS_GITHUB_MIRROR_TEMPLATE": "file:///disabled/<asset>",
    }
