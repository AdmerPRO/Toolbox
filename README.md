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
Canonical links, `/sitemap.xml`, and the sitemap entry in `/robots.txt` use
`https://tools.admerpro.pl` by default. Set `SITE_URL` to a different absolute
HTTP or HTTPS origin when deploying elsewhere. All eight public pages have
unique meta descriptions and canonical links; download endpoints are excluded
from the sitemap. The navigation uses the cat image as its brand icon.

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
run concurrently. YouTube downloads are limited to 500 MiB per result and 2 hours of recorded media.
Live streams, channels and playlists are rejected; accepted links are normalized
to one canonical video URL. YouTube processing times out after 10 minutes; uploaded
audio extraction times out after 10 minutes. Uploads time out after 5 minutes.
MP4 inputs are validated by ffprobe within 20 seconds, with at most 10 streams.
Download only content you own or have permission to download.

## Rate limiting

Each client IP has two independent 60-second windows: by default, 10 POST
requests to `/api/` (including metadata lookups and conversions), and 60 other
API requests (including downloads). Exceeding a limit returns HTTP 429 with
`Retry-After` in seconds. Failed requests also count. Public pages, static
assets, and `/api/healthcheck` are excluded. Existing concurrency limits
still apply independently.

A client may have only one POST API operation in progress by default,
including uploading, conversion, and YouTube processing. A parallel request
receives HTTP 429 with `Retry-After: 1`; retry after the current operation
finishes. Set `MAX_CONCURRENT_JOBS_PER_CLIENT` to change this cap. Slots are
released on completion, failure, or cancellation. A blocking image conversion
keeps its slot until its worker finishes even if the client disconnects.

Set `RATE_LIMIT_JOBS_PER_MINUTE` and `RATE_LIMIT_API_PER_MINUTE` to positive
integers to change the limits. Counters are held in bounded server memory,
expired entries are periodically removed during API requests, and restarting
the server resets them. Each server process has its own counters.

Direct connections use the socket peer IP. Behind a reverse proxy, set
`TRUSTED_PROXY_IPS` to its exact IP addresses, separated by commas, for example
`127.0.0.1,::1`. Only those proxies may supply `CF-Connecting-IP` or `X-Forwarded-For`.
A single valid `CF-Connecting-IP` takes precedence; malformed or repeated
values fall back to the proxy IP. Without that header, the server walks the chain from right to left and selects the nearest untrusted address.
Configure your proxy to replace or append the real connecting client's IP.
Without trusted proxy configuration, all traffic through a proxy shares its
IP's limit. Clients sharing a public IP also share the same limit.

## Resource and deployment security

The default storage budget is 30 GiB (`MAX_STORAGE_BYTES`), with at least
5 GiB left free on its filesystem (`MIN_FREE_DISK_BYTES`). Admission reserves
2 GiB for a YouTube job or 768 MiB for an upload conversion, across concurrent
jobs and archiving. Reservations are conservative and may reject work before
the configured budget is reached. Storage includes staging, originals, results,
legacy files, and archives. Capacity failures at admission return HTTP 503;
existing files remain available. Use one server process per storage directory.
The 250 ms process monitor stops jobs exceeding 1500 MiB (YouTube) or 768 MiB
(upload conversion), or consuming the free disk reserve. Final YouTube results
must be non-empty and at most 500 MiB. yt-dlp also receives `--max-filesize`,
a duration/live filter, bounded retries, disabled plugins/cache, and one FFmpeg
thread. Process output is bounded; timeout/cancellation stops the process tree.
Monitoring can briefly overshoot its threshold; filesystem quotas are needed
for strict limits across other processes writing to the same filesystem.
Archiving reserves space for originals plus an incompressible ZIP before it
starts; insufficient capacity leaves originals intact for the next attempt.
Set `UPLOAD_TIMEOUT_SECONDS` (1-3600) to change the complete multipart upload deadline.

Every response carries CSP, nosniff, frame denial, referrer and permissions
policies. CSP permits scripts/styles from this origin and YouTube thumbnails
from `i.ytimg.com` / `img.youtube.com`. API responses use `private, no-store`.

See [deployment/security.md](deployment/security.md) for the hardened Linux
service, Cloudflare Tunnel, HSTS, edge rate limiting, and maintenance steps.
The service and Cloudflare settings must be installed on the deployment host;
changing this repository does not activate them on an existing server.

## Private file audit database and privacy acknowledgement

The application automatically creates `storage/audit.sqlite3` (SQLite, bundled
with the executable, no separate database server needed). It is not served by
HTTP. The three tables are:

| Table | Records |
| --- | --- |
| `files` | Result UUID/type, UTC upload or YouTube job-start timestamp, uploader IP, input/result sizes in bytes, archive/deletion times, aggregate accepted download count |
| `file_access` | One row per result UUID and viewer IP, first/last access timestamp, cumulative request count |
| `ip_uploads` | One row linking each live file to its uploader IP and upload timestamp |

`files.archive_path` associates a file with its ZIP for lifecycle cleanup;
`files.policy_version` records the policy version acknowledged by the request.
The IP resolver is shared with rate limiting, including trusted Cloudflare
headers. Download counts record accepted GET/HEAD requests, not proof of viewing
or completed transfers. Missing, invalid and expired downloads are not counted.
YouTube requests use the requester's IP; input size describes retained downloads
rather than a browser upload. Separate YouTube inputs are kept when available.

Successful publication and audit insertion are coordinated using a transaction
and filesystem rollback on commit failure. Database failures prevent publication
or download rather than silently omit a record. Maintenance archives whole job
folders only after every file is old enough; finalized archives keep audit IPs.
Deleting the final retained archive clears uploader IP, viewer access rows and
upload index rows. Metadata without IP remains with `deleted_at`. Other files
uploaded by the same IP keep their records; there is no separate IP registry.
Startup/hourly reconciliation also handles manual filesystem deletions. Existing
files are imported with unknown uploader IP and approximate historical times;
old records cannot reconstruct past access counts. No names, video titles or
submitted URLs are recorded in this database.

All public pages display a privacy acceptance dialog on first visit. Declining
keeps public pages and the full policy readable while disabling media tools.
Media POST endpoints require `privacy_policy=2026-10-03`, otherwise HTTP 428.
The version cookie lasts one year and is not an authentication credential or
proof of identity. Policy changes require updating the version in `src/audit.rs`,
`frontend/shared/privacy.js`, `frontend/root/script.js`, the policy and tests.
`PRIVACY_CONTACT_EMAIL` configures the contact address displayed in the policy;
default: `admin@tools.admerpro.com`. The policy explains administrator review of
uploads/results/archives for abuse investigation and lawful requests. Operators
must confirm their controller identity, legal basis, infrastructure log retention
and backup/deletion procedures for their own deployment before publishing.

### Administrator inspection

Review the database and retained media locally on the host, with a trusted
SQLite client. There is no public administrator API. For example:

```sh
sqlite3 -readonly storage/audit.sqlite3
```

```sql
SELECT id, file_type, uploaded_at, uploader_ip, original_size, stored_size,
       archived, archived_at, deleted_at, open_count, archive_path
FROM files ORDER BY uploaded_at DESC;
SELECT * FROM file_access WHERE file_id = 'FILE_UUID';
SELECT * FROM ip_uploads WHERE ip = 'CLIENT_IP';
```

Active originals/results are under `storage/active/DDMMYYYY/FILE_UUID/`;
archived content is located by `archive_path`. Limit host/database access to the
administrator, and avoid public reports containing private links or IPs. Review
untrusted media with the service's isolation in mind. For manual removal, delete
all originals/results and archive copies for the affected file; let reconciliation
clear IP records at the next maintenance run. A shared ZIP may contain other
files: do not delete it wholesale for a request concerning only one file. Rebuild
that ZIP excluding the affected job instead. Backups and provider logs require
separate operator deletion procedures.

## Checks

GitHub Actions builds and tests the project on Windows, macOS, Ubuntu, and
Ubuntu ARM64 on pushes and pull requests targeting `master`. The workflow can
also be started manually. Formatting, Clippy, and JavaScript syntax checks run
in a separate Ubuntu job, together with typos and release packaging tests.
`cargo check --locked --all-targets --all-features` also runs on each platform.

## Nightly and full releases

The **Nightly release** workflow runs daily at 02:17 UTC and can also be
started manually. Each successful run creates a new GitHub prerelease with
an immutable `nightly-YYYYMMDD-RUN_ID-ATTEMPT` tag. Nightly describes the build
schedule; both nightly and full releases use stable Rust.

To generate a full release, open **Actions > Full release > Run workflow**,
select the source branch, and enter a tag matching the package version in
`Cargo.toml`, such as `v0.1.0`. Update the package version and lockfile before
releasing a new version. The workflow creates a draft by default; disable the
draft option to publish immediately. Existing releases are never overwritten.
Repository Actions must allow `GITHUB_TOKEN` to write repository contents.

Both workflows first run the shared CI checks and then build five native
packages: Windows x64, Ubuntu x64, macOS Intel, macOS Apple Silicon, and Linux
ARM64 for Raspberry Pi 4B. Linux builds use Ubuntu 22.04 and require glibc
2.35 or newer. Pi 4B needs a 64-bit OS such as Raspberry Pi OS Bookworm or
Ubuntu 22.04 or newer; 32-bit Raspberry Pi OS is not supported. ARM64 tests
run on GitHub-hosted runners and do not verify physical Pi hardware.

Release builds use optimization level 3, fat LTO, one codegen unit, stripped
symbols, and panic abort. They use generic CPU targets rather than the build
machine's CPU features. Windows packages are ZIP files; Linux/macOS packages
are tar.gz files. Each includes the executable, frontend, license, README,
configuration example, running instructions, and build metadata. SHA256SUMS.txt
is attached to the release. Workflow artifacts also include individual
checksums and remain available for 14 days for nightly or 90 days for full
releases; published GitHub release assets are separate from this retention.

Extract the entire package and run the executable **from its extracted
folder** so it can find `frontend/`. FFmpeg, ffprobe, yt-dlp, and any required
JavaScript runtime must be installed separately. Every platform verifies the
extracted package's checksum and starts the packaged server to check its
pages and static assets before publication. Publication requires all five
packages to succeed. macOS binaries are not signed or notarized.

The ARM64 job checks compatibility with Raspberry Pi running a 64-bit Linux
OS. It runs on a GitHub-hosted Ubuntu runner, not physical Raspberry Pi
hardware, and does not cover 32-bit Raspberry Pi OS or external media tools.

```sh
cargo fmt --check
cargo test
cargo check --locked --all-targets --all-features
cargo clippy --all-targets -- -D warnings
typos
python -m unittest discover -s tests -p "test_release*.py"
node --check frontend/shared/downloader.js
node --check frontend/shared/converter.js
node --check frontend/shared/privacy.js
node --test tests/privacy.test.cjs
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
