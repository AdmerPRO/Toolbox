"""Benchmark an isolated release server, then optionally run browser audits.

python tests/performance.py --binary target/release/admersite --report performance.json
Thresholds are CI regression budgets, not production capacity guarantees.
"""
import argparse
from concurrent.futures import ThreadPoolExecutor
import json
import os
from pathlib import Path
import shutil
import socket
import subprocess
import tempfile
import time
import urllib.request

PATHS = ('/api/healthcheck', '/', '/images/', '/qr/', '/root/style.css')


def request(base, path):
    started = time.perf_counter()
    with urllib.request.urlopen(base + path, timeout=5) as response:
        body = response.read()
        if response.status != 200 or not body:
            raise RuntimeError(f'{path}: empty or unsuccessful response')
    return (time.perf_counter() - started) * 1000


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, required=True)
    parser.add_argument('--report', type=Path, default=Path('performance.json'))
    parser.add_argument('--requests', type=int, default=200)
    parser.add_argument('--concurrency', type=int, default=8)
    parser.add_argument('--max-p95-ms', type=float, default=500)
    parser.add_argument('command', nargs=argparse.REMAINDER)
    args = parser.parse_args()
    if args.requests < 1 or args.concurrency < 1:
        parser.error('requests and concurrency must be positive')
    binary = args.binary.resolve()
    with tempfile.TemporaryDirectory(prefix='toolbox-performance-') as temporary:
        root = Path(temporary)
        shutil.copytree(Path(__file__).resolve().parents[1] / 'frontend', root / 'frontend')
        with socket.socket() as listener:
            listener.bind(('127.0.0.1', 0))
            port = listener.getsockname()[1]
        base = f'http://127.0.0.1:{port}'
        environment = dict(os.environ, ADDRESS='127.0.0.1', PORT=str(port), SITE_URL=base,
                           TRUSTED_PROXY_IPS='', PRIVACY_CONTACT_EMAIL='privacy@example.org',
                           RATE_LIMIT_API_PER_MINUTE='10000', HSTS_MAX_AGE_SECONDS='0')
        with (root / 'server.log').open('w') as log:
            server = subprocess.Popen([str(binary)], cwd=root, env=environment, stdout=log, stderr=log)
            try:
                deadline = time.monotonic() + 30
                while True:
                    try:
                        request(base, '/api/healthcheck')
                        break
                    except (OSError, TimeoutError):
                        if server.poll() is not None or time.monotonic() > deadline:
                            raise RuntimeError((root / 'server.log').read_text())
                        time.sleep(.1)
                results = []
                for endpoint in PATHS:
                    for _ in range(5):
                        request(base, endpoint)
                    started = time.perf_counter()
                    with ThreadPoolExecutor(max_workers=args.concurrency) as pool:
                        timings = sorted(pool.map(lambda _: request(base, endpoint), range(args.requests)))
                    elapsed = time.perf_counter() - started
                    p95 = timings[max(0, (len(timings) * 95 + 99) // 100 - 1)]
                    results.append(dict(path=endpoint, requests=args.requests, concurrency=args.concurrency,
                                        p95_ms=round(p95, 2), max_ms=round(timings[-1], 2),
                                        requests_per_second=round(args.requests / elapsed, 2)))
                args.report.parent.mkdir(parents=True, exist_ok=True)
                args.report.write_text(json.dumps({'max_p95_ms': args.max_p95_ms, 'results': results}, indent=2))
                print(json.dumps(results, indent=2))
                failures = [row for row in results if row['p95_ms'] > args.max_p95_ms]
                command = args.command
                if command and command[0] == '--':
                    command = command[1:]
                # Run audits even if the HTTP budget failed so both reports are available.
                if command:
                    subprocess.run(command, env=dict(os.environ, LHCI_BASE_URL=base), check=True)
                if failures:
                    raise RuntimeError(f'HTTP p95 budget exceeded: {failures}')
            finally:
                server.terminate()
                try:
                    server.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    server.kill()
                    server.wait()


if __name__ == '__main__':
    main()
