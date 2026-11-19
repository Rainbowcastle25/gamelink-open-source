#!/usr/bin/env python3
"""Exercise the release signing gates with local Git refs and fixture bytes."""

import base64
import os
from pathlib import Path
import shutil
import subprocess
import sys
import unittest

import release
from release_pins import check_binary_pins
from release_tools import SIGNERS


def run(cwd, *command, check=True):
    return subprocess.run(command, cwd=cwd, check=check, capture_output=True, text=True)


def fixture_pin(byte):
    return base64.b64encode(b"Ed" + byte * 40).decode("ascii")


def fixture_tauri(pin):
    return base64.b64encode(f"untrusted comment: minisign public key: 1\n{pin}\n".encode()).decode("ascii")


class ReleaseGates(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        scratch = release.ROOT / ".tools"
        scratch.mkdir(exist_ok=True)
        cls.work = Path(run(release.ROOT, "mktemp", "-d", scratch / "release-gates.XXXXXXXX").stdout.strip())
        source = cls.work / "source"
        cls.checkout = cls.work / "checkout"
        source.mkdir()
        run(source, "git", "init", "--quiet", "--initial-branch=master")
        run(source, "git", "config", "user.name", "Release gate tests")
        run(source, "git", "config", "user.email", "release-gates@example.invalid")
        run(source, "git", "commit", "--quiet", "--allow-empty", "-m", "Base")
        cls.base = run(source, "git", "rev-parse", "HEAD").stdout.strip()
        run(source, "git", "commit", "--quiet", "--allow-empty", "-m", "Release")
        cls.master = run(source, "git", "rev-parse", "HEAD").stdout.strip()
        run(source, "git", "tag", "-a", "v1.0.0", "-m", "Release", "-m",
            "GameLink-Release-Severity: security")
        run(source, "git", "checkout", "--quiet", "--detach", cls.base)
        run(source, "git", "commit", "--quiet", "--allow-empty", "-m", "Off master")
        cls.other = run(source, "git", "rev-parse", "HEAD").stdout.strip()
        run(source, "git", "tag", "v1.1.0")
        run(source, "git", "checkout", "--quiet", "master")
        run(cls.work, "git", "clone", "--quiet", "--no-tags", source, cls.checkout)
        run(cls.checkout, "git", "tag", "v1.0.0", cls.base)
        scripts = cls.checkout / "scripts"
        scripts.mkdir()
        for name in ("release.py", "release_pins.py", "release_tools.py"):
            shutil.copyfile(release.ROOT / "scripts" / name, scripts / name)

    def source_check(self, tag, commit):
        return run(self.checkout, sys.executable, "-B", "scripts/release.py",
                   "--check-source", "--tag", tag, "--commit", commit, check=False)

    def test_annotated_release_tag_on_master_passes(self):
        result = self.source_check("v1.0.0", self.master)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("on origin/master (security)", result.stdout)

    def test_non_master_commit_is_rejected_even_when_tag_matches(self):
        result = self.source_check("v1.1.0", self.other)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("is not an ancestor of origin/master", result.stderr)

    def test_tag_head_mismatch_is_rejected_even_if_local_tag_matches(self):
        result = self.source_check("v1.0.0", self.base)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("does not point to build commit", result.stderr)

    def test_wrong_signer_hashes_are_rejected_before_asset_access(self):
        tools = self.work / "wrong-signers"
        tools.mkdir()
        for name in SIGNERS:
            (tools / name).write_bytes(b"untrusted executable")
        with self.assertRaisesRegex(ValueError, "SHA-256 mismatch"):
            release.check_staging("1.0.0", False, self.work, self.work / "missing-assets", tools,
                                  ([], "", ""), os.environ)

    def test_missing_or_extra_manifest_pins_are_rejected(self):
        manifest, extra = fixture_pin(b"A"), fixture_pin(b"B")
        pins = ([manifest], "", "")
        check_binary_pins(manifest.encode(), "agent", pins)
        for binary in (extra, f"{manifest} {extra}"):
            with self.subTest(binary=binary), self.assertRaisesRegex(ValueError, "embedded manifest pins"):
                check_binary_pins(binary.encode(), "agent", pins)

    def test_wrong_desktop_updater_pin_is_rejected(self):
        manifest = fixture_pin(b"A")
        tauri = fixture_tauri(fixture_pin(b"B"))
        other = fixture_tauri(fixture_pin(b"C"))
        pins = ([manifest], tauri, "")
        check_binary_pins(f"{manifest} {tauri}requireSignedVersion".encode(), "desktop", pins, desktop=True)
        with self.assertRaisesRegex(ValueError, "embedded Tauri updater pin"):
            check_binary_pins(f"{manifest} {other}requireSignedVersion".encode(), "desktop", pins, desktop=True)


if __name__ == "__main__":
    unittest.main(verbosity=2)
