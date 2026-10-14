# CI tests and performance budgets

Rust CI runs the Rust suite, real media conversion smoke checks, Python release
and worker tests, and JavaScript privacy and welcome behavior tests.

```sh
node --test tests/*.test.cjs
python -m unittest discover -s tests -p 'test_*.py'
cargo build --locked
python tests/media_smoke.py --binary target/debug/admersite
```

The Website performance workflow runs for master pushes, pull requests targeting
master, manual runs, and release builds. Release packaging waits for it to pass.
It builds an optimized server and starts it in a temporary directory with copied
frontend files and separate storage. It never loads the checkout's `.env` or
contacts the public production site.

## HTTP benchmark

Five routes each receive five warmup requests and 200 measured requests, with
eight concurrent clients. Every response must be successful and nonempty. A p95
response time above 500 ms fails the job. The JSON report includes p95, maximum
latency, and throughput. This measures local HTTP serving, not video conversion
capacity or production network latency. Throughput is reported without a hard
threshold because shared runner hardware varies.

```sh
cargo build --locked --release
python tests/performance.py --binary target/release/admersite --report reports/http-performance.json
```

Use `.exe` on Windows. Override `--requests`, `--concurrency`, or `--max-p95-ms`
for local experiments; CI uses the defaults.

## Lighthouse

Nine pages are audited three times each using Lighthouse's default mobile
simulation. Median performance must score at least 80/100; median LCP must be
at most 4000 ms, CLS at most 0.1, and total blocking time at most 300 ms.
Accessibility and SEO below 90/100 produce warnings pending baseline review.
These are laboratory regression budgets, not measurements of real users' INP.

With Chrome available:

```sh
npm install --global @lhci/cli@0.14.0
python tests/performance.py --binary target/release/admersite --report reports/http-performance.json -- lhci autorun --config=lighthouserc.cjs
```

GitHub Actions uploads `website-performance` for 14 days, including raw Lighthouse
results and generated HTML reports even when assertions fail. Inspect failing
audits before changing budgets. Timing budgets should only be adjusted with
evidence from repeated runs, not to hide a regression.
