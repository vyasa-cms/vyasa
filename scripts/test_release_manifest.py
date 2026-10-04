"""Tests for release-manifest.py: run with `python3 -m unittest scripts/test_release_manifest.py`."""
import hashlib
import importlib.util
import json
import pathlib
import tempfile
import unittest

from cryptography.hazmat.primitives import serialization as ser
from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey, Ed25519PublicKey

HERE = pathlib.Path(__file__).resolve().parent
SPEC = importlib.util.spec_from_file_location("release_manifest", HERE / "release-manifest.py")
rm = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(rm)


class ReleaseManifest(unittest.TestCase):
    def setUp(self):
        self.tmp = pathlib.Path(tempfile.mkdtemp())
        key = Ed25519PrivateKey.generate()
        self.key_file = self.tmp / "key"
        self.key_file.write_text(key.private_bytes(ser.Encoding.Raw, ser.PrivateFormat.Raw, ser.NoEncryption()).hex())
        self.public = key.public_key()
        self.dist = self.tmp / "dist"
        self.dist.mkdir()
        (self.dist / "vyasa-1.0.0-x86_64-unknown-linux-gnu.tar.gz").write_bytes(b"tarball bytes")
        self.out = self.tmp / "stable.json"

    def run_it(self, version, current=None):
        args = ["--version", version, "--dist", str(self.dist), "--key-file", str(self.key_file), "--out", str(self.out)]
        if current:
            args += ["--current", str(current)]
        return rm.main(args)

    def test_a_release_is_signed_and_becomes_latest(self):
        self.assertEqual(self.run_it("1.0.0"), 0)
        doc = json.loads(self.out.read_text())
        self.assertEqual(doc["latest"], "1.0.0")
        (release,) = doc["releases"]
        (artifact,) = release["artifacts"]
        self.assertEqual(artifact["platform"], "x86_64-unknown-linux-gnu")
        self.assertEqual(artifact["url"], "https://github.com/vyasa-cms/vyasa/releases/download/v1.0.0/vyasa-1.0.0-x86_64-unknown-linux-gnu.tar.gz")
        self.assertEqual(artifact["sha256"], hashlib.sha256(b"tarball bytes").hexdigest())
        self.public.verify(bytes.fromhex(artifact["signature"]), b"tarball bytes")
        migrations = len(list((HERE.parent / "crates/db/migrations").glob("*.sql")))
        self.assertEqual(release["migration_version"], migrations)

    def test_a_prerelease_is_refused(self):
        self.assertEqual(self.run_it("1.1.0-rc.1"), 2)
        self.assertFalse(self.out.exists())

    def test_rerunning_a_version_replaces_it(self):
        self.run_it("1.0.0")
        self.assertEqual(self.run_it("1.0.0", current=self.out), 0)
        self.assertEqual(len(json.loads(self.out.read_text())["releases"]), 1)


if __name__ == "__main__":
    unittest.main()
