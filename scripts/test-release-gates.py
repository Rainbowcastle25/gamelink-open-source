#!/usr/bin/env python3
"""Exercise the release signing gates with local Git refs and fixture bytes."""

import base64
import os
from pathlib import Path
import shutil
import subprocess
import sys
import unittest
from unittest.mock import patch

import apt_repo
import release
from release_deb import check_deb_pins
from release_pins import check_binary_pins
from release_tools import SIGNERS


def run(cwd, *command, check=True):
    return subprocess.run(command, cwd=cwd, check=check, capture_output=True, text=True)


def fixture_pin(byte):
    return base64.b64encode(b"Ed" + byte * 40).decode("ascii")


def fixture_tauri(pin):
    return base64.b64encode(f"untrusted comment: minisign public key: 1\n{pin}\n".encode()).decode("ascii")


class ReleaseGates(unittest.TestCase):
    def test_verifier_requires_exactly_one_passing_test(self):
        release.require_one_passed_test("running 1 test\ntest result: ok. 1 passed; 0 failed; 0 ignored\n")
        for output in ("test result: ok. 0 passed; 0 failed; 0 ignored\n", "", "test result: ok. 2 passed; 0 failed;\n"):
            with self.assertRaises(RuntimeError):
                release.require_one_passed_test(output)

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
        for name in ("release.py", "release_deb.py", "release_pins.py", "release_tools.py"):
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

    @unittest.skipUnless(shutil.which("dpkg-deb"), "requires dpkg-deb")
    def test_deb_pins_and_binary_must_match_raw_asset(self):
        layout = self.work / "deb-fixture"
        (layout / "DEBIAN").mkdir(parents=True)
        binary = layout / "usr/bin/gamelink-agent"
        binary.parent.mkdir(parents=True)
        (layout / "DEBIAN/control").write_text(
            "Package: gamelink-agent\nVersion: 1.0.0\nArchitecture: amd64\n"
            "Maintainer: Test <release-gates@example.invalid>\nDescription: gate fixture\n"
        )
        elf = b"\x7fELF\x02\x01" + bytes(12) + b"\x3e\x00"
        expected, extra = fixture_pin(b"A").encode(), fixture_pin(b"B").encode()
        raw = self.work / "raw-agent"
        raw.write_bytes(elf + expected)
        for suffix, payload, error in (
            ("correct", expected, None),
            ("wrong", extra, "embedded manifest pins"),
            ("extra", expected + b" " + extra, "embedded manifest pins"),
            ("different", expected + b" modified", "packaged agent differs"),
        ):
            with self.subTest(package=suffix):
                package = self.work / f"{suffix}.deb"
                binary.write_bytes(elf + payload)
                run(self.work, "dpkg-deb", "--build", "--root-owner-group", layout, package)
                args = (package, raw, "1.0.0", "amd64", ([expected.decode()], "", ""), os.environ)
                if error is None:
                    check_deb_pins(*args)
                else:
                    with self.assertRaisesRegex(ValueError, error):
                        check_deb_pins(*args)

    @unittest.skipUnless(all(shutil.which(tool) for tool in ("gpg", "gpgconf", "gpgv")), "requires GnuPG")
    def test_apt_signing_fails_closed(self):
        key_work = self.work / "apt-test-keys"
        key_work.mkdir()
        home = key_work / "gnupg"
        home.mkdir(mode=0o700)
        gpg = apt_repo.gpg_home(home)
        public = key_work / "apt.asc"
        keys = {}
        for identity in ("One", "Two"):
            user = f"{identity} <{identity.lower()}@example.invalid>"
            run(key_work, *gpg, "--pinentry-mode", "loopback", "--passphrase", "",
                "--quick-generate-key", user, "ed25519", "sign", "1d")
            if identity == "One":
                run(key_work, *gpg, "--armor", "--output", public, "--export", user)
            keys[identity] = run(key_work, *gpg, "--armor", "--export-secret-keys", user).stdout
        fingerprint = apt_repo.public_fingerprint(gpg, public)
        run(key_work, "gpgconf", "--homedir", home, "--kill", "gpg-agent")

        repo = self.work / "apt-unsigned-fixture"
        release_file = repo / "dists/stable/Release"
        release_file.parent.mkdir(parents=True)
        release_file.write_text("Suite: stable\n")
        shutil.copyfile(public, repo / "apt.asc")
        placeholder = key_work / "placeholder.asc"
        placeholder.write_text("Placeholder for the apt public key.")
        for case, pin, expected, key, exception in (
            ("missing-pin", key_work / "missing.asc", fingerprint, keys["One"], ValueError),
            ("placeholder-pin", placeholder, fingerprint, keys["One"], ValueError),
            ("wrong-fingerprint", public, "A" * 40, keys["One"], ValueError),
            ("missing-secret", public, fingerprint, "", ValueError),
            ("wrong-secret", public, fingerprint, keys["Two"], subprocess.CalledProcessError),
            ("success", public, fingerprint, keys["One"], None),
        ):
            with self.subTest(case=case):
                work = self.work / f"apt-sign-{case}"
                work.mkdir()
                public.with_suffix(".fingerprint").write_text(expected + "\n")
                with patch.object(apt_repo, "run", wraps=apt_repo.run) as signing_run:
                    if exception is None:
                        apt_repo.sign_repository(repo, work, pin, key)
                    else:
                        with self.assertRaises(exception):
                            apt_repo.sign_repository(repo, work, pin, key)
                last_command = signing_run.call_args.args
                self.assertEqual(last_command[:2], ("gpgconf", "--homedir"))
                self.assertEqual(last_command[3:], ("--kill", "gpg-agent"))
                self.assertFalse(Path(last_command[2]).exists())
                self.assertEqual(release_file.with_name("InRelease").exists(), exception is None)
                self.assertEqual(release_file.with_suffix(".gpg").exists(), exception is None)

    def test_wrong_desktop_updater_pin_is_rejected(self):
        manifest = fixture_pin(b"A")
        tauri = fixture_tauri(fixture_pin(b"B"))
        other = fixture_tauri(fixture_pin(b"C"))
        pins = ([manifest], tauri, "")
        check_binary_pins(f"{manifest} {tauri}requireSignedVersion".encode(), "desktop", pins, desktop=True)
        with self.assertRaisesRegex(ValueError, "embedded Tauri updater pin"):
            check_binary_pins(f"{manifest} {other}requireSignedVersion".encode(), "desktop", pins, desktop=True)

    def test_inline_summary_bytes_match_the_manifest_asset(self):
        out = self.work / "summaries"
        out.mkdir()
        payloads = []
        self.assertEqual(release.add_release_summary("", out, payloads), "")
        self.assertEqual(payloads, [])
        summary = release.add_release_summary("Fix connection drops.\n Add an updater.", out, payloads)
        self.assertEqual(summary, "Fix connection drops. Add an updater.")
        self.assertEqual([path.name for path in payloads], ["release-summary.txt"])
        self.assertEqual(payloads[0].read_bytes(), summary.encode("utf-8"))
        with self.assertRaisesRegex(ValueError, "512 UTF-8 bytes"):
            release.add_release_summary("é" * 257, out, [])


if __name__ == "__main__":
    unittest.main(verbosity=2)
