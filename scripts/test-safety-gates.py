#!/usr/bin/env python3
"""Exercise the existing gates in disposable repositories; no live key material."""
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

REPO = Path(__file__).resolve().parents[1]


def command(root, *args):
    return subprocess.run(args, cwd=root, text=True, stdout=subprocess.PIPE,
                          stderr=subprocess.STDOUT, timeout=180)


class SecretGate(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(prefix="gamelink-secret-gate-")
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        command(self.root, "git", "init", "-q").check_returncode()
        self.script = self.root / "scripts/scan-secrets.sh"
        self.script.parent.mkdir()
        shutil.copyfile(REPO / "scripts/scan-secrets.sh", self.script)

    def scan(self):
        return command(self.root, "bash", str(self.script))

    def test_empty_repository_and_placeholders_pass(self):
        self.assertEqual(self.scan().returncode, 0)
        (self.root / "placeholders.txt").write_text(
            "nodekey:" + "a" * 64 + " nodekey:" + "0123456789abcdef" * 4)
        self.assertEqual(self.scan().returncode, 0)

    def test_unusual_filenames_fail_without_printing_material(self):
        token = "privkey:" + "1234abcd" * 8
        for name in ("with spaces.txt", "with\nnewline.txt", "-leading-option.txt"):
            with self.subTest(name=name):
                path = self.root / name
                path.write_text(token)
                result = self.scan()
                self.assertEqual(result.returncode, 1, result.stdout)
                self.assertNotIn(token, result.stdout)
                self.assertIn("content redacted", result.stdout)
                path.unlink()

    def test_placeholder_does_not_exempt_real_key_on_same_line(self):
        real = "nodekey:" + "1234abcd" * 8
        (self.root / "mixed.txt").write_text("nodekey:" + "0" * 64 + " " + real)
        result = self.scan()
        self.assertEqual(result.returncode, 1, result.stdout)
        self.assertNotIn(real, result.stdout)

    def test_placeholder_with_extra_hex_is_not_exempt(self):
        (self.root / "oversized.txt").write_text("nodekey:" + "0" * 65)
        self.assertEqual(self.scan().returncode, 1)

    def test_invitation_and_pem_fail(self):
        for token in ("glink1." + "aB_2" * 20, "BEGIN " + "PRIVATE KEY"):
            (self.root / "sample.txt").write_text(token)
            result = self.scan()
            self.assertEqual(result.returncode, 1, result.stdout)
            self.assertNotIn(token, result.stdout)

    def test_reviewed_fixture_exemption_is_bound_to_bytes(self):
        fixture = self.root / "core/fixtures/pure.json"
        fixture.parent.mkdir(parents=True)
        shutil.copyfile(REPO / "core/fixtures/pure.json", fixture)
        self.assertEqual(self.scan().returncode, 0)
        with fixture.open("a") as stream:
            stream.write("\n")
        self.assertEqual(self.scan().returncode, 1)


class DependencyGate(unittest.TestCase):
    def test_actual_forbidden_import_and_broken_graph_fail(self):
        with tempfile.TemporaryDirectory(prefix="gamelink-dependency-gate-") as tmp:
            root = Path(tmp)
            for name in ("go.mod", "go.sum", "build-tags.txt"):
                shutil.copyfile(REPO / name, root / name)
            shutil.copytree(REPO / "shim", root / "shim")
            (root / "scripts").mkdir()
            script = root / "scripts/check-forbidden-deps.sh"
            shutil.copyfile(REPO / "scripts/check-forbidden-deps.sh", script)
            baseline = command(root, "bash", str(script))
            self.assertEqual(baseline.returncode, 0, baseline.stdout)
            probe = root / "shim/agent/audit_probe.go"
            probe.write_text('package main\nimport _ "github.com/pkg/sftp"\n')
            result = command(root, "bash", str(script))
            self.assertEqual(result.returncode, 1, result.stdout)
            self.assertIn("FORBIDDEN code is linked into ./shim/agent (linux)", result.stdout)
            self.assertIn("github.com/pkg/sftp", result.stdout)
            probe.write_text('package main\nimport _ "invalid.example/missing"\n')
            result = command(root, "bash", str(script))
            self.assertEqual(result.returncode, 1, result.stdout)
            self.assertIn("go list failed", result.stdout)


if __name__ == "__main__":
    unittest.main(verbosity=2)
