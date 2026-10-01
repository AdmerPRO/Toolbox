# Tool Box

**Tool Box** is a free, all-in-one web application that provides various useful tools for working with media files and more.

The project is currently **under heavy development**.

## Features

* YouTube MP4 downloads with a resolution selector, up to 2160p
* YouTube MP3 audio downloads at 128, 192, 256 or 320 kbps
* Image conversion between PNG, JPG, JPEG, WebP, ICO, BMP, and TIFF
* Image resizing with preserved aspect ratio
* MP4 audio removal without video re-encoding
* MP3 extraction from uploaded MP4 videos at 192 kbps
* Seven-day file availability with automatic ZIP archiving by date
* Responsive interface with an animated red gradient and reduced-motion support

## Running locally

Install Rust, [yt-dlp](https://github.com/yt-dlp/yt-dlp#installation), and FFmpeg
(with ffprobe). The yt-dlp and FFmpeg executables must be available on PATH.
YouTube extraction may also require a JavaScript runtime supported by yt-dlp,
such as Deno. See the [yt-dlp dependency guide](https://github.com/yt-dlp/yt-dlp#dependencies).

```sh
cargo run
```

Open http://127.0.0.1:3000. Optionally set `ADDRESS` and `PORT` in `.env`.
Run commands from the project root so the frontend and storage paths resolve.

Pages: `/`, `/youtubemp4/`, `/youtubemp3/`, `/images/`, `/mp4tomp3/`, `/resize/`, `/mute/`, `/privacy/`.
FFmpeg is also required for uploaded MP4 audio extraction. Image conversion
uses the Rust `image` library and does not require an external image tool.
Images are limited to 20 MiB and 4096 x 4096 pixels; MP4 uploads to 200 MiB.
JPG/JPEG output replaces transparency with white; ICO output fits within
256 x 256 pixels. Only the first image frame/icon or audio track is used.

Successful jobs store their originals and results under
`storage/active/DDMMYYYY/<random-id>/`. Dates use UTC: `01102026` means
1 October 2026. Working files use `storage/staging` and are normally removed
after unsuccessful processing. An abrupt shutdown may leave staging files
that the operator must remove when no jobs are running.

Download links expire after 7 days. At startup and every hour, expired files
are compressed into ZIP archives under `storage/archives/DDMMYYYY/`.
Multiple archives may exist for one day. Archives are finalized and checked
before source files are removed; failures are logged and retried.
Older `storage/ytmp4` and `storage/ytmp3` files are included in this process.
Archives are not served by the website and are automatically deleted 30 days
after archive creation, checked at startup and hourly. Deletion failures are
logged and retried; downtime can extend retention. Archiving does not erase
data. Keep the storage directory outside publicly served directories and
provide users with an operator contact channel for privacy/deletion requests.
The [privacy policy](frontend/privacy/index.html) explains this behavior.

At most three YouTube metadata/download processes and two upload conversions
run concurrently. YouTube processing times out after 30 minutes; uploaded
audio extraction times out after 10 minutes.
Download only content you own or have permission to download.

## Checks

GitHub Actions builds and tests the project on Windows, macOS, Ubuntu, and
Ubuntu ARM64 on pushes and pull requests targeting `master`. The workflow can
also be started manually. Formatting, Clippy, and JavaScript syntax checks run
in a separate Ubuntu job.

The ARM64 job checks compatibility with Raspberry Pi running a 64-bit Linux
OS. It runs on a GitHub-hosted Ubuntu runner, not physical Raspberry Pi
hardware, and does not cover 32-bit Raspberry Pi OS or external media tools.

```sh
cargo fmt --check
cargo test
cargo clippy --all-targets -- -D warnings
node --check frontend/shared/downloader.js
node --check frontend/shared/converter.js
cargo build --locked
python tests/media_smoke.py --binary target/debug/admersite
```

On Windows, use `target/debug/admersite.exe` for the smoke test. It requires
Python and FFmpeg, runs the server in isolated temporary storage, and checks
real image conversion, MP4 audio extraction, expired downloads, and invalid
uploads. This test also runs in the Ubuntu GitHub Actions checks job.

## Why Tool Box?

* **Free to use**
* **No advertisements**
* Simple and easy-to-use interface
* Multiple tools in one place
* Open source

## Project Status

> **Under Development**

Tool Box is still being actively developed. Some features may be incomplete or unavailable, and new tools will be added over time.

## License

Tool Box is licensed under the **GNU General Public License v3.0 (GPL-3.0)**.

This means you are free to use, study, modify, and redistribute the software, as long as derivative works are also distributed under the GPL-3.0 license.

See the [`LICENSE`](LICENSE) file for the full license text.

---
