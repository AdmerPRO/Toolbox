# Tool Box

**Tool Box** is a free, all-in-one web application that provides various useful tools for working with media files and more.

The project is currently **under heavy development**.

## Features

* YouTube MP4 downloads with a resolution selector, up to 2160p
* YouTube MP3 audio downloads at 128, 192, 256 or 320 kbps
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

Pages: `/`, `/youtubemp4/`, `/youtubemp3/`.
Downloads are stored under `storage/ytmp4` and `storage/ytmp3` using unique names.
Stored files remain on disk until removed by the operator. At most three
metadata/download processes run concurrently; downloads expire after 30 minutes.
Download only content you own or have permission to download.

## Checks

```sh
cargo fmt --check
cargo test
cargo clippy --all-targets -- -D warnings
node --check frontend/shared/downloader.js
```

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
