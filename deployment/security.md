# Secure Raspberry Pi / Linux deployment

Use a 64-bit Linux host with systemd. Install the release into `/opt/toolbox`,
owned by root, and keep it outside any public static-file directory. Install
FFmpeg (with ffprobe), yt-dlp, and its supported JS runtime on PATH. When using
pip, install yt-dlp in an operator-managed virtual environment and add its bin
directory with a systemd `Environment=PATH=...` override. Do not enable
MemoryDenyWriteExecute: a yt-dlp JS runtime may require JIT compilation.

## Dedicated service account

After extracting the release into `/opt/toolbox`, run on the deployment host:

```sh
sudo useradd --system --no-create-home --shell /usr/sbin/nologin toolbox
sudo install -d -o toolbox -g toolbox -m 0700 /opt/toolbox/storage /opt/toolbox/storage/tmp
sudo install -m 0644 /opt/toolbox/deployment/toolbox.service /etc/systemd/system/toolbox.service
sudo systemctl daemon-reload
sudo systemctl enable --now toolbox
sudo systemctl status toolbox
sudo journalctl -u toolbox --since today
```

For an existing installation, stop the old server and make the existing
`storage` tree owned by toolbox before switching services. Keep executable,
frontend, and configuration root-owned. The supplied service permits writes
only to storage, its state directory, and private temporary files. It caps
memory, CPU, tasks and file descriptors and kills the entire service process
group on shutdown. Adjust MemoryMax/CPUQuota to the host after monitoring real
conversions. This isolates the service account and filesystem, but is not a
per-parser network sandbox: yt-dlp needs outbound Internet access. Uploaded
MP4 handling constrains ffprobe/FFmpeg to local file/pipe protocols.

Copy `.envexample` to `.env` and review storage thresholds. The application
reads `.env` from its working directory. Environment values in the service
have precedence. Create overrides with `sudo systemctl edit toolbox` and
restart after changes. Keep ADDRESS at 127.0.0.1. For stricter protection from
other processes filling the disk, place storage on a filesystem with a quota.

## Cloudflare Tunnel and edge rules

Point cloudflared at `http://127.0.0.1:8080`. Do not forward the application
port from the router. The service trusts only loopback proxy addresses, so
local processes must also be trusted. Configure cloudflared/Cloudflare to
provide the real visitor CF-Connecting-IP, and verify that distinct visitors
receive independent application rate limits. Requests from untrusted socket
peers cannot override their IP with forwarding headers.

In the Cloudflare zone serving the actual public hostname:

1. Require HTTPS at the edge and use Full (strict) for HTTPS origins. A local
   HTTP Tunnel origin uses cloudflared's encrypted tunnel transport.
2. Enable HSTS with a staged max-age first, then 31536000 after verifying HTTPS
   for the hostname. Add includeSubDomains only if all subdomains support
   HTTPS. Leave preload disabled until that is a deliberate domain-wide policy.
3. Add an edge rate limiting rule for POST requests under `/api/`, especially
   `/api/youtube/info`, `/api/youtube/download`, `/api/youtube/download/mp3`,
   and `/api/convert/`. Start with a threshold appropriate for the audience,
   such as 10 per minute per client, if the plan supports that window. Rule
   windows, actions and counting characteristics depend on the Cloudflare plan.
4. Add an explicit cache bypass rule for `/api/*`, and remove any rule that
   overrides the application's `private, no-store` API response headers.
5. Verify the public site's security headers, successful uploads/downloads,
   and rate limits. Do not cache a POST response or a private download link.

These are operator actions in the Cloudflare account. The repository cannot
activate zone settings or establish HTTPS coverage for all your subdomains.

## Security maintenance

Schedule a regular maintenance window on the host:

```sh
sudo apt update
sudo apt upgrade
ffmpeg -version
ffprobe -version
yt-dlp --version
```

Update yt-dlp through its installation method: `yt-dlp -U` for an official
binary, or the operator-managed virtual environment's pip with
`python -m pip install --upgrade 'yt-dlp[default]'`. Do not run a pip upgrade
against an OS-managed Python environment. Review upstream security notices,
restart Toolbox after updates, then run the repository's media smoke tests
against the new binary. Keep rollback copies of the previous release and
configuration. Monitor free space and service failures; archives retain user
data and count against the storage budget.

References: [yt-dlp options](https://github.com/yt-dlp/yt-dlp#usage-and-options),
[ffprobe](https://ffmpeg.org/ffprobe.html),
[Cloudflare client headers](https://developers.cloudflare.com/fundamentals/reference/http-headers/),
[HSTS](https://developers.cloudflare.com/ssl/edge-certificates/additional-options/http-strict-transport-security/),
[rate limiting](https://developers.cloudflare.com/waf/rate-limiting-rules/),
[systemd sandboxing](https://www.freedesktop.org/software/systemd/man/latest/systemd.exec.html).

## Audit database and private media review

`storage/audit.sqlite3` and its SQLite WAL/SHM files contain private upload and
viewer IP records. The service's UMask=0077 protects files it creates. Keep
storage inaccessible to other accounts, and check ownership/permissions when
migrating existing storage. Do not expose SQLite or an audit dashboard over
public HTTP. Read the README administrator inspection queries for local access.
Update `PRIVACY_CONTACT_EMAIL` to a monitored address. Policy acknowledgement
is required for media POST calls, including scripted smoke/API requests.
Backups must be protected and follow deletion/retention procedures; automatic
live-database cleanup does not erase off-host copies. SQLite uses secure_delete
and checkpoints after cleanup; filesystem snapshots and physical media recovery
are outside the application's logical deletion guarantees.

Application HSTS is optional: set `HSTS_MAX_AGE_SECONDS` only after the public
hostname supports HTTPS and `SITE_URL` uses HTTPS. Default 0 omits the header.
The application adds neither includeSubDomains nor preload. Align application
and Cloudflare header policies; client cookies/forwarded headers do not enable HSTS.
