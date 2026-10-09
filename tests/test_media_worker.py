import importlib.util
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location("media_worker", Path(__file__).resolve().parents[1] / "deployment" / "media-worker.py")
worker = importlib.util.module_from_spec(spec)
spec.loader.exec_module(worker)


class WorkerArguments(unittest.TestCase):
    @patch.object(worker.shutil, "which", return_value="/usr/bin/ffprobe")
    def test_local_tools_have_no_network_and_only_one_job_mount(self, _which):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary).resolve()
            job = root / "storage" / "staging" / "job1"
            job.mkdir(parents=True)
            command = worker.worker_args(["-", "ffprobe", str(job / "input.mp4")], root)
            self.assertIn("--unshare-net", command)
            self.assertIn("--clearenv", command)
            self.assertIn("--unshare-pid", command)
            mounts = [command[index + 1] for index, value in enumerate(command) if value == "--bind"]
            self.assertEqual(mounts, [str(job)])
            self.assertNotIn(str(root / "storage"), mounts)

    @patch.object(worker.shutil, "which", return_value="/usr/local/bin/yt-dlp")
    def test_downloader_needs_network_but_not_storage_for_metadata(self, _which):
        with tempfile.TemporaryDirectory() as temporary:
            command = worker.worker_args(["-", "yt-dlp", "--", "https://www.youtube.com/watch?v=abcdefghijk"], Path(temporary))
            self.assertNotIn("--unshare-net", command)
            self.assertNotIn("--bind", command)

    @patch.object(worker.shutil, "which", return_value="/usr/bin/ffmpeg")
    def test_multiple_jobs_and_unsupported_tools_are_rejected(self, _which):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary).resolve()
            jobs = [root / "storage" / "staging" / name for name in ["a", "b"]]
            for job in jobs:
                job.mkdir(parents=True)
            with self.assertRaises(ValueError):
                worker.worker_args([str(jobs[0]), "ffmpeg", str(jobs[1] / "secret.mp4")], root)
            with self.assertRaises(ValueError):
                worker.worker_args(["-", "sh", "-c", "anything"], root)


if __name__ == "__main__":
    unittest.main()
