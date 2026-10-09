#!/usr/bin/python3
"""Root-owned Linux worker wrapper. Never fall back to unsandboxed execution."""
import os
from pathlib import Path
import shutil
import sys


def worker_args(arguments, cwd):
    directory, tool, *args = arguments
    if tool not in {"yt-dlp", "ffmpeg", "ffprobe"}:
        raise ValueError("Unsupported media tool")
    executable = shutil.which(tool)
    if not executable:
        raise ValueError("Media tool unavailable")
    root = cwd.resolve()
    staging = root / "storage" / "staging"
    jobs = set()
    candidates = ([directory] if directory != "-" else []) + args
    for value in candidates:
        path = Path(value)
        path = (root / path).resolve() if not path.is_absolute() else path.resolve()
        if path.is_relative_to(staging):
            relative = path.relative_to(staging)
            if relative.parts:
                jobs.add(staging / relative.parts[0])
    if len(jobs) > 1:
        raise ValueError("Worker cannot access multiple jobs")
    command = ["/usr/bin/bwrap", "--die-with-parent", "--new-session",
               "--unshare-user", "--unshare-pid", "--unshare-ipc", "--unshare-uts",
               "--cap-drop", "ALL", "--clearenv", "--setenv", "PATH", "/usr/local/bin:/usr/bin:/bin",
               "--setenv", "HOME", "/tmp", "--setenv", "TMPDIR", "/tmp"]
    # Only runtime dependencies are readable. Audit DB, other jobs, .env and sockets are absent.
    for path in ["/usr", "/bin", "/lib", "/lib64"]:
        if Path(path).exists():
            command += ["--ro-bind", path, path]
    for path in ["/etc/resolv.conf", "/etc/hosts", "/etc/nsswitch.conf", "/etc/ssl", "/etc/ld.so.cache"]:
        if Path(path).exists():
            command += ["--ro-bind", path, path]
    command += ["--proc", "/proc", "--dev", "/dev", "--tmpfs", "/tmp", "--dir", str(root)]
    for job in jobs:
        if not job.is_dir() or job.is_symlink():
            raise ValueError("Invalid staging job")
        command += ["--bind", str(job), str(job)]
    if tool != "yt-dlp":
        command += ["--unshare-net"]
    command += ["--chdir", str(root), "--", executable, *args]
    return command


def main():
    import resource
    try:
        command = worker_args(sys.argv[1:], Path.cwd())
        # Hard per-process limits inherited by descendants, plus service-wide cgroup limits.
        for kind, maximum in [(resource.RLIMIT_AS, 1024 * 1024 * 1024),
                              (resource.RLIMIT_CPU, 600),
                              (resource.RLIMIT_FSIZE, 500 * 1024 * 1024),
                              (resource.RLIMIT_NOFILE, 256), (resource.RLIMIT_CORE, 0)]:
            current = resource.getrlimit(kind)[1]
            limit = maximum if current == resource.RLIM_INFINITY else min(maximum, current)
            resource.setrlimit(kind, (limit, limit))
        os.execv(command[0], command)
    except (ValueError, OSError) as error:
        print(f"Media sandbox unavailable: {error}", file=sys.stderr)
        sys.exit(1)


if __name__ == "__main__":
    main()
