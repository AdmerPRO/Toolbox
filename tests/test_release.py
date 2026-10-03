import importlib.util
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
import zipfile
import tarfile

spec = importlib.util.spec_from_file_location("release", Path(__file__).resolve().parents[1] / "scripts/release.py")
release = importlib.util.module_from_spec(spec)
spec.loader.exec_module(release)


class ReleaseTests(unittest.TestCase):
    def test_nightly_tags_include_run_and_attempt(self):
        with tempfile.TemporaryDirectory() as temporary:
            output = Path(temporary) / "outputs"
            with patch.dict(release.os.environ, {"GITHUB_OUTPUT": str(output), "GITHUB_RUN_ID": "123", "GITHUB_RUN_ATTEMPT": "2"}):
                release.prepare(True)
            self.assertRegex(output.read_text(), r"^tag=nightly-\d{8}-123-2\n$")

    def test_full_release_tag_must_match_manifest(self):
        source = Path(__file__).resolve().parents[1]
        version = release.tomllib.loads((source / "Cargo.toml").read_text())["package"]["version"]
        with tempfile.TemporaryDirectory() as temporary:
            output = Path(temporary) / "outputs"
            with patch.dict(release.os.environ, {"GITHUB_OUTPUT": str(output), "RELEASE_TAG": f"v{version}"}):
                release.prepare(False)
            self.assertEqual(output.read_text(), f"tag=v{version}\n")
            with patch.dict(release.os.environ, {"GITHUB_OUTPUT": str(output), "RELEASE_TAG": "v999.0.0"}):
                with self.assertRaisesRegex(ValueError, "must match"):
                    release.prepare(False)

    def test_rejects_unsafe_tags(self):
        for tag in ["../escape", "--draft", "bad tag", "v1\ninjected"]:
            with self.assertRaises(ValueError):
                release.validate_tag(tag)

    def test_packages_include_frontend_config_and_checksums(self):
        source = Path(__file__).resolve().parents[1]
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            binary = root / "binary"
            binary.write_bytes(b"test executable")
            for target in release.TARGETS:
                archive = release.package(source, binary, target, "v0.1.0", root / "dist")
                expected = archive.with_name(archive.name + ".sha256").read_text().split()[0]
                self.assertEqual(expected, release.checksum(archive))
                if archive.suffix == ".zip":
                    with zipfile.ZipFile(archive) as reader:
                        names = reader.namelist()
                else:
                    with tarfile.open(archive) as reader:
                        names = reader.getnames()
                        executable = next(member for member in reader.getmembers() if member.name.endswith("/admersite"))
                        self.assertTrue(executable.mode & 0o111)
                for suffix in ["/frontend/root/index.html", "/frontend/shared/privacy.js", "/frontend/shared/privacy.css", "/frontend/assets/good_cat_image.png", "/.envexample", "/BUILD.json", "/LICENSE", "/RUNNING.md", "/deployment/toolbox.service", "/deployment/security.md"]:
                    self.assertTrue(any(name.endswith(suffix) for name in names), suffix)
                self.assertFalse(any("/storage/" in name or name.endswith("/.env") for name in names))

    def test_verification_rejects_modified_archive(self):
        with tempfile.TemporaryDirectory() as temporary:
            archive = Path(temporary) / "modified.zip"
            archive.write_bytes(b"corrupted")
            archive.with_name(archive.name + ".sha256").write_text("0" * 64)
            with self.assertRaisesRegex(ValueError, "checksum"):
                release.verify(archive)


if __name__ == "__main__":
    unittest.main()
