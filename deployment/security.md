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
sudo install -m 0644 /opt/toolbox/deployment/toolbox-egress.service /etc/systemd/system/toolbox-egress.service
sudo systemctl daemon-reload
# First complete the firewall and nginx setup below.
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
conversions. Production media commands also run through the fail-closed
`media-worker.py` bubblewrap wrapper: they see system runtimes and their single
staging job, not the audit database, .env, other jobs or host sockets. Local
FFmpeg/ffprobe have no network namespace access. yt-dlp inherits the service
UID's nftables egress policy, including all redirects and resolved addresses.
Per-process address space (1 GiB), CPU time (600 seconds), output file size
(500 MiB), descriptors (256) and core dumps (disabled) are constrained.
The service cgroup bounds the entire worker tree. Linux user namespaces and
bubblewrap must be supported; failure prevents processing, never falls back.
Install tools in /usr or /usr/local; runtime files outside these paths are
intentionally unavailable inside the sandbox.

Copy `.envexample` to `.env` and review storage thresholds. The application
reads `.env` from its working directory. Environment values in the service
have precedence. Create overrides with `sudo systemctl edit toolbox` and
restart after changes. Keep ADDRESS at 127.0.0.1. For stricter protection from
other processes filling the disk, place storage on a filesystem with a quota.

## Cloudflare Tunnel and edge rules

Point cloudflared at `http://127.0.0.1:8080` (nginx), with Toolbox on 8081. Do not forward the application
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
viewer IP records in plaintext. On Unix, startup enforces storage mode 0700
and database/WAL/SHM/journal mode 0600; new sidecars inherit database permissions.
The service's UMask=0077 also protects files it creates. On Windows, use restricted
filesystem ACLs. Keep
storage inaccessible to other accounts, and check ownership/permissions when
migrating existing storage. Do not expose SQLite or an audit dashboard over
public HTTP. Read the README administrator inspection queries for local access.
Update `PRIVACY_CONTACT_EMAIL` to a monitored address. Policy acknowledgement
is required for media POST calls, including scripted smoke/API requests.
Backups must be protected and follow deletion/retention procedures; automatic
live-database cleanup does not erase off-host copies. SQLite uses secure_delete
and checkpoints after cleanup; filesystem snapshots and physical media recovery
are outside the application's logical deletion guarantees.

The supplied production environment example and service set
`HSTS_MAX_AGE_SECONDS=31536000`. Enable it only after the public
hostname supports HTTPS and `SITE_URL` uses HTTPS. The application default 0
omits the header for local HTTP development.
The application adds neither includeSubDomains nor preload. Align application
and Cloudflare header policies; client cookies/forwarded headers do not enable HSTS.


## Required firewall and proxy installation

Install `bubblewrap`, `nftables`, `nginx` and Python 3.9+ before enabling the
service. The wrapper must be root-owned and executable, never writable by toolbox:

```sh
sudo chown root:root /opt/toolbox/deployment/media-worker.py
sudo chmod 0755 /opt/toolbox/deployment/media-worker.py
sudo nft --check --file /opt/toolbox/deployment/toolbox-egress.nft
sudo systemctl daemon-reload
sudo systemctl enable --now toolbox-egress
sudo install -m 0644 /opt/toolbox/deployment/nginx-toolbox.conf /etc/nginx/conf.d/toolbox.conf
sudo nginx -t
sudo systemctl reload nginx
```

Include the egress file from the host's persistent nftables configuration.
Do not flush unrelated firewall tables. Reload rules deliberately when changing
this table; initial loading creates it. The required root firewall oneshot service installs the table before Toolbox starts.
Its CAP_NET_ADMIN privilege is separate from the unprivileged media service.
Existing persistent tables are preserved; explicitly review their contents.
Verify rules survive reboot. Use a public DNS resolver in `/etc/resolv.conf`:
loopback stubs and private LAN resolvers are deliberately blocked. A public DNS
response resolving to a private destination is still rejected by the firewall.
This policy blocks IPv4/IPv6 private, loopback, link-local, metadata, mapped and
translation destinations and permits only public HTTP(S) and DNS. If your network
uses custom globally routable internal/metadata ranges, add them to the deny rules.
Do not give another untrusted local account the toolbox UID.

The nginx configuration limits per-visitor and global simultaneous requests,
request rate and idle header/body/keepalive times. The application separately
limits total upload time. Edge connection floods still require the Cloudflare
settings above. Only cloudflared may reach nginx's loopback listener; public
listeners are not included. Do not expose either 8080 or 8081 through a router.

## Result links, logs and backups

A result URL is a bearer secret: anyone with the URL can download the result.
Global conversion cache reuse intentionally shares an existing result for equal
inputs. This is not owner authentication. The app logs route templates rather
than download URLs and redacts UUID filenames and external URLs from tool errors.
The supplied nginx configuration disables access logging. Restrict journal and
nginx log access to administrators; upstream Tunnel/edge logs also need this policy.
Do not put result links into analytics, public issue reports or screenshots.

Encrypt off-host backups and restrict keys to backup administrators. Use SQLite's
backup API or stop the service before copying the database, preserving consistency.
Expire backups containing media and IP records within the published retention
window, including snapshots and replicas. Document restore procedures: restored
records must be reconciled against retained files before serving traffic. No repo
change can erase existing external backups.

## Host acceptance checks before production

1. Run `cargo audit --deny warnings` with current RustSec data and review installed
   `ffmpeg`, `ffprobe`, yt-dlp, JS runtime and distro security updates. CI now runs
   the Cargo audit weekly and for dependency changes; it does not scan OS binaries.
2. Verify `systemctl show toolbox -p User -p MemoryMax -p CPUQuotaPerSecUSec -p TasksMax`
   and root ownership of executable, wrapper and configuration. Confirm user
   namespaces work and valid MP4/MP3 jobs succeed under the production wrapper.
3. Under `sudo -u toolbox`, verify HTTP connections to 127.0.0.1, RFC1918,
   169.254.169.254, ::1, fe80:: and fc00:: fail; inspect nft counters. Repeat via
   HTTP redirects and a DNS name resolving to a denied address. Confirm public
   downloads work. Run a second job attempting to read another staging directory
   and the audit database; both must be inaccessible inside the wrapper.
4. Configure storage on a dedicated filesystem or a project/user quota. Set the
   hard quota to the storage budget plus a documented margin for SQLite/WAL.
   Verify quota enforcement using a disposable file, never production media.
   Application polling is not a replacement for this kernel-enforced quota.
5. Verify public HTTPS/HSTS, direct port closure, distinct visitor IP attribution,
   slow upload timeout and 429 responses. Run a bounded load test on staging,
   monitoring memory, CPU, descriptors, proxy connections and free disk.
6. Verify journal/storage ACLs, encrypted backup retention and restoration.

These checks need the real Linux host and Cloudflare account. Passing Windows
unit/smoke tests does not certify firewall, sandbox, TLS, quota or edge deployment.

Sandbox option reference: [bubblewrap manual](https://manpages.debian.org/testing/bubblewrap/bwrap.1.en.html).
