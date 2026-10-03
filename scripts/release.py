"""Prepare, package, verify, and publish native Tool Box releases."""

import argparse
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import socket
import subprocess
import tarfile
import tempfile
import time
import tomllib
import urllib.error
import urllib.request
import zipfile

TARGETS = (
    "x86_64-pc-windows-msvc",
    "x86_64-unknown-linux-gnu",
    "x86_64-apple-darwin",
    "aarch64-apple-darwin",
    "aarch64-unknown-linux-gnu",
)


def validate_tag(tag):
    if not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9._-]{0,119}", tag):
        raise ValueError("Invalid release tag")
    return tag


def checksum(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def prepare(nightly):
    if nightly:
        date = datetime.now(timezone.utc).strftime("%Y%m%d")
        tag = f"nightly-{date}-{os.environ['GITHUB_RUN_ID']}-{os.environ['GITHUB_RUN_ATTEMPT']}"
    else:
        version = tomllib.loads(Path("Cargo.toml").read_text())["package"]["version"]
        tag = validate_tag(os.environ["RELEASE_TAG"])
        if tag != f"v{version}":
            raise ValueError(f"Release tag must match Cargo.toml: v{version}")
    with Path(os.environ["GITHUB_OUTPUT"]).open("a") as output:
        output.write(f"tag={validate_tag(tag)}\n")
    print(f"Validated release tag: {tag}")


def package(source, binary, target, tag, destination):
    validate_tag(tag)
    if target not in TARGETS:
        raise ValueError("Unsupported target")
    if not binary.is_file():
        raise FileNotFoundError(binary)
    destination.mkdir(parents=True, exist_ok=True)
    name = f"toolbox-{tag}-{target}"
    with tempfile.TemporaryDirectory(prefix="toolbox-package-") as directory:
        bundle = Path(directory) / name
        bundle.mkdir()
        executable = bundle / ("admersite.exe" if "windows" in target else "admersite")
        shutil.copy2(binary, executable)
        executable.chmod(0o755)
        shutil.copytree(source / "frontend", bundle / "frontend")
        shutil.copytree(source / "deployment", bundle / "deployment")
        for filename in ["README.md", "LICENSE", ".envexample"]:
            shutil.copy2(source / filename, bundle / filename)
        version = tomllib.loads((source / "Cargo.toml").read_text())["package"]["version"]
        metadata = {"version": version, "tag": tag, "target": target,
                    "commit": os.environ.get("GITHUB_SHA", "local"),
                    "optimizations": {"opt_level": 3, "lto": "fat", "codegen_units": 1, "strip": "symbols", "panic": "abort"}}
        (bundle / "BUILD.json").write_text(json.dumps(metadata, indent=2) + "\n")
        (bundle / "RUNNING.md").write_text(
            "# Running Tool Box\n\n"
            "Extract the entire archive and run the program from this folder.\n"
            "Windows: `./admersite.exe`. Linux/macOS: `./admersite`.\n"
            "Open http://127.0.0.1:3000. Optionally copy `.envexample` to `.env`\n"
            "and set ADDRESS, PORT, and SITE_URL. Keep the frontend folder beside\n"
            "the program. Files are stored in the local storage folder.\n\n"
            "Install FFmpeg (including ffprobe) and yt-dlp separately and add\n"
            "them to PATH. YouTube may also need a supported JavaScript runtime.\n"
            "These third-party executables are not included in this package.\n\n"
            "Linux packages target Ubuntu 22.04 or newer (glibc 2.35+).\n"
            "The ARM64 package targets Raspberry Pi 4B with 64-bit Raspberry Pi\n"
            "OS Bookworm or newer, or 64-bit Ubuntu 22.04+. It does not support\n"
            "32-bit Raspberry Pi OS. System shared libraries must be installed.\n"
            "ARM64 builds are tested on hosted ARM64 runners, not physical Pi hardware.\n"
        )
        if "windows" in target:
            archive = destination / f"{name}.zip"
            with zipfile.ZipFile(archive, "w", compression=zipfile.ZIP_DEFLATED) as writer:
                for path in sorted(bundle.rglob("*")):
                    if path.is_file():
                        writer.write(path, path.relative_to(bundle.parent).as_posix())
        else:
            archive = destination / f"{name}.tar.gz"
            def executable_mode(member):
                if member.name.endswith("/admersite"):
                    member.mode = 0o755
                return member
            with tarfile.open(archive, "w:gz") as writer:
                writer.add(bundle, arcname=name, filter=executable_mode)
        archive.with_name(archive.name + ".sha256").write_text(f"{checksum(archive)}  {archive.name}\n")
    return archive


def verify(archive):
    expected = archive.with_name(archive.name + ".sha256").read_text().split()[0]
    if checksum(archive) != expected:
        raise ValueError("Package checksum mismatch")
    with tempfile.TemporaryDirectory(prefix="toolbox-verify-") as directory:
        root = Path(directory)
        if archive.suffix == ".zip":
            with zipfile.ZipFile(archive) as reader:
                for member in reader.namelist():
                    if Path(member).is_absolute() or ".." in Path(member).parts:
                        raise ValueError("Unsafe package member")
                reader.extractall(root)
        else:
            with tarfile.open(archive) as reader:
                reader.extractall(root, filter="data")
        bundle, = root.iterdir()
        binary = bundle / ("admersite.exe" if os.name == "nt" else "admersite")
        with socket.socket() as listener:
            listener.bind(("127.0.0.1", 0))
            port = listener.getsockname()[1]
        base = f"http://127.0.0.1:{port}"
        environment = dict(os.environ, ADDRESS="127.0.0.1", PORT=str(port), SITE_URL=base)
        with (root / "server.log").open("w") as log:
            server = subprocess.Popen([str(binary)], cwd=bundle, env=environment, stdout=log, stderr=log)
            try:
                for _ in range(100):
                    try:
                        urllib.request.urlopen(base + "/api/healthcheck", timeout=1).close()
                        break
                    except (urllib.error.URLError, TimeoutError):
                        if server.poll() is not None:
                            raise RuntimeError((root / "server.log").read_text())
                        time.sleep(0.1)
                else:
                    raise RuntimeError("Packaged server did not start")
                for page in ["/", "/images/", "/resize/", "/mp4tomp3/", "/mute/", "/youtubemp4/", "/youtubemp3/", "/privacy/", "/root/style.css", "/assets/good_cat_image.png", "/sitemap.xml", "/robots.txt"]:
                    with urllib.request.urlopen(base + page, timeout=5) as response:
                        if not response.read():
                            raise RuntimeError(f"Empty packaged page: {page}")
            finally:
                server.terminate()
                try:
                    server.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    server.kill()
                    server.wait()
    print(f"Verified checksum and relocated server: {archive.name}")


def publish():
    tag = validate_tag(os.environ["RELEASE_TAG"])
    directory = Path("dist")
    packages = sorted(list(directory.glob("*.zip")) + list(directory.glob("*.tar.gz")))
    if len(packages) != len(TARGETS) or any(not any(f"-{target}." in package.name for package in packages) for target in TARGETS):
        raise ValueError("Release must contain all five platform packages")
    lines = []
    for archive in packages:
        expected = archive.with_name(archive.name + ".sha256").read_text().split()[0]
        if checksum(archive) != expected:
            raise ValueError(f"Checksum mismatch: {archive.name}")
        lines.append(f"{expected}  {archive.name}\n")
    sums = directory / "SHA256SUMS.txt"
    sums.write_text("".join(lines))
    nightly = os.environ["RELEASE_NIGHTLY"] == "true"
    notes = directory / "release-notes.md"
    notes.write_text(
        f"{'Nightly' if nightly else 'Full'} Tool Box release built from `{os.environ['GITHUB_SHA']}`.\n\n"
        "Includes Windows x64, Ubuntu x64, macOS Intel/Apple Silicon, and Linux ARM64\n"
        "for Raspberry Pi 4B with a 64-bit OS. Each package includes the frontend,\n"
        "license, build metadata, configuration example, and running instructions.\n\n"
        "Optimized with opt-level 3, fat LTO, one codegen unit, stripped symbols,\n"
        "and panic abort. All checks and native package startup verification passed.\n"
        "Verify downloads against SHA256SUMS.txt. FFmpeg, yt-dlp, and any required\n"
        "JavaScript runtime must be installed separately. Linux needs glibc 2.35+.\n"
    )
    command = ["gh", "release", "create", tag, "--repo", os.environ["GITHUB_REPOSITORY"],
               "--target", os.environ["GITHUB_SHA"], "--title", f"Tool Box {tag}", "--notes-file", str(notes)]
    if nightly:
        command += ["--prerelease", "--latest=false"]
    if os.environ["RELEASE_DRAFT"] == "true":
        command.append("--draft")
    command += [str(path) for path in packages] + [str(sums)]
    # Creating a new release never replaces or deletes an existing release/tag.
    subprocess.run(command, check=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=["prepare", "package", "verify", "publish"])
    parser.add_argument("--nightly", action="store_true")
    arguments = parser.parse_args()
    if arguments.command == "prepare":
        prepare(arguments.nightly)
    elif arguments.command in ["package", "verify"]:
        target = os.environ["RELEASE_TARGET"]
        tag = validate_tag(os.environ["RELEASE_TAG"])
        extension = ".zip" if "windows" in target else ".tar.gz"
        if arguments.command == "package":
            binary = Path("target") / target / "release" / ("admersite.exe" if "windows" in target else "admersite")
            print(package(Path("."), binary, target, tag, Path("dist")))
        else:
            verify(Path("dist") / f"toolbox-{tag}-{target}{extension}")
    else:
        publish()


if __name__ == "__main__":
    main()
