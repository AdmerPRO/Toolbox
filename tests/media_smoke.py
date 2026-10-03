"""Exercise real upload, conversion, download, and expiry using FFmpeg fixtures.

Run after cargo build: python tests/media_smoke.py --binary target/debug/admersite
The server and all generated files are isolated in a temporary directory.
"""

import argparse
import json
import os
import re
from pathlib import Path
import shutil
import socket
import subprocess
import tempfile
import time
import urllib.error
import urllib.request
import uuid
import xml.etree.ElementTree as ET


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", required=True, type=Path)
    binary = parser.parse_args().binary.resolve()
    with tempfile.TemporaryDirectory(prefix="toolbox-smoke-") as working:
        root = Path(working)
        shutil.copytree(Path(__file__).resolve().parents[1] / "frontend", root / "frontend")
        with socket.socket() as listener:
            listener.bind(("127.0.0.1", 0))
            port = listener.getsockname()[1]
        base = f"http://127.0.0.1:{port}"
        environment = dict(os.environ, ADDRESS="127.0.0.1", PORT=str(port), SITE_URL=base,
                           RATE_LIMIT_API_PER_MINUTE="100", RATE_LIMIT_JOBS_PER_MINUTE="100", TRUSTED_PROXY_IPS="", UPLOAD_TIMEOUT_SECONDS="2")
        with (root / "server.log").open("w") as log:
            server = subprocess.Popen([str(binary)], cwd=root, env=environment, stdout=log, stderr=log)
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
                    raise RuntimeError("Server did not start")

                def get(path, expected=200):
                    try:
                        with urllib.request.urlopen(base + path, timeout=10) as response:
                            assert response.status == expected
                            assert response.headers["X-Content-Type-Options"] == "nosniff"
                            if path.startswith("/api/"):
                                assert response.headers["Cache-Control"] == "private, no-store"
                            return response.read(), response.headers
                    except urllib.error.HTTPError as error:
                        assert error.code == expected, (error.code, error.read())
                        assert error.headers["X-Content-Type-Options"] == "nosniff"
                        if path.startswith("/api/"):
                            assert error.headers["Cache-Control"] == "private, no-store"
                        return error.read(), error.headers

                def upload(path, filename, content, output=None, expected=200):
                    boundary = "Toolbox" + uuid.uuid4().hex
                    body = bytearray()
                    if output is not None:
                        body.extend(f'--{boundary}\r\nContent-Disposition: form-data; name="format"\r\n\r\n{output}\r\n'.encode())
                    body.extend(f'--{boundary}\r\nContent-Disposition: form-data; name="file"; filename="{filename}"\r\nContent-Type: application/octet-stream\r\n\r\n'.encode())
                    body.extend(content)
                    body.extend(f'\r\n--{boundary}--\r\n'.encode())
                    request = urllib.request.Request(base + path, data=body, headers={"Content-Type": f"multipart/form-data; boundary={boundary}"})
                    try:
                        with urllib.request.urlopen(request, timeout=30) as response:
                            assert response.status == expected
                            return json.load(response)["download_url"]
                    except urllib.error.HTTPError as error:
                        assert error.code == expected, (error.code, error.read())
                        return None

                pages = ["/", "/images/", "/mp4tomp3/", "/resize/", "/mute/", "/youtubemp4/", "/youtubemp3/", "/privacy/"]
                descriptions = []
                for page in pages:
                    html, page_headers = get(page)
                    assert page_headers["X-Content-Type-Options"] == "nosniff"
                    assert page_headers["X-Frame-Options"] == "DENY"
                    assert "script-src 'self'" in page_headers["Content-Security-Policy"]
                    assert "frame-ancestors 'none'" in page_headers["Content-Security-Policy"]
                    assert page_headers["Referrer-Policy"] == "strict-origin-when-cross-origin"
                    assert "camera=()" in page_headers["Permissions-Policy"]
                    assert b"Privacy" in html
                    text = html.decode("utf-8")
                    descriptions.append(re.search(r'<meta name="description" content="([^"]+)"', text).group(1))
                    assert text.count('rel="canonical"') == 1
                    assert f'<link rel="canonical" href="{base}{page}">' in text
                    assert 'class="brand-cat" src="/assets/good_cat_image.png"' in text
                    assert '{{SITE_URL}}' not in text
                assert len(set(descriptions)) == len(pages)
                cat, cat_headers = get("/assets/good_cat_image.png")
                assert cat.startswith(b"\x89PNG") and cat_headers["Content-Type"] == "image/png"
                sitemap, sitemap_headers = get("/sitemap.xml")
                assert sitemap_headers["Content-Type"].startswith("application/xml")
                urls = [entry.text for entry in ET.fromstring(sitemap).findall("{http://www.sitemaps.org/schemas/sitemap/0.9}url/{http://www.sitemaps.org/schemas/sitemap/0.9}loc")]
                assert set(urls) == {base + page for page in pages}
                robots, _ = get("/robots.txt")
                assert robots.count(b"Sitemap:") == 1
                assert f"Sitemap: {base}/sitemap.xml".encode() in robots
                assert b"Disallow: /api/" in robots
                assert get("/images/index.html")[0] == get("/images/")[0]
                assert b"Archiving is not deletion" in get("/privacy/")[0]
                assert b"30 days after it is created" in get("/privacy/")[0]

                image = root / "sample.png"
                subprocess.run(["ffmpeg", "-v", "error", "-f", "lavfi", "-i", "color=c=red:s=320x280", "-frames:v", "1", str(image)], check=True)
                for extension, mime in [("png", "image/png"), ("jpg", "image/jpeg"), ("jpeg", "image/jpeg"), ("webp", "image/webp"), ("ico", "image/x-icon"), ("bmp", "image/bmp"), ("tiff", "image/tiff")]:
                    url = upload("/api/convert/image", "sample.png", image.read_bytes(), extension)
                    content, headers = get(url)
                    assert headers["Content-Type"] == mime
                    assert headers["Cache-Control"] == "private, no-store"
                    result = root / f"result.{extension}"
                    result.write_bytes(content)
                    subprocess.run(["ffmpeg", "-v", "error", "-i", str(result), "-f", "null", "-"], check=True)
                    # Every output format is also accepted as an input.
                    roundtrip = upload("/api/convert/image", result.name, content, "png")
                    assert get(roundtrip)[0].startswith(b"\x89PNG")

                resized = upload("/api/convert/resize?width=80&height=80", "sample.png", image.read_bytes(), "png")
                resized_file = root / "resized.png"
                resized_file.write_bytes(get(resized)[0])
                dimensions = json.loads(subprocess.check_output(["ffprobe", "-v", "error", "-show_entries", "stream=width,height", "-of", "json", str(resized_file)]))["streams"][0]
                assert dimensions == {"width": 80, "height": 70}, dimensions
                upload("/api/convert/resize?width=0&height=80", "sample.png", image.read_bytes(), "png", 400)
                upload("/api/convert/resize?width=4097&height=80", "sample.png", image.read_bytes(), "png", 400)

                video = root / "sample.mp4"
                subprocess.run(["ffmpeg", "-v", "error", "-f", "lavfi", "-i", "color=c=black:s=32x32:d=1", "-f", "lavfi", "-i", "sine=frequency=440:duration=1", "-c:v", "mpeg4", "-c:a", "aac", "-shortest", str(video)], check=True)
                url = upload("/api/convert/audio", "sample.mp4", video.read_bytes())
                content, headers = get(url)
                assert headers["Content-Type"] == "audio/mpeg"
                audio = root / "result.mp3"
                audio.write_bytes(content)
                subprocess.run(["ffmpeg", "-v", "error", "-i", str(audio), "-f", "null", "-"], check=True)
                muted_url = upload("/api/convert/mute", "sample.mp4", video.read_bytes())
                muted = root / "muted.mp4"
                muted_content, muted_headers = get(muted_url)
                assert muted_headers["Content-Type"] == "video/mp4"
                muted.write_bytes(muted_content)
                streams = json.loads(subprocess.check_output(["ffprobe", "-v", "error", "-show_entries", "stream=codec_type", "-of", "json", str(muted)]))["streams"]
                assert streams == [{"codec_type": "video"}], streams
                date, filename = url.rsplit("/", 2)[1:]
                job = root / "storage" / "active" / date / filename.rsplit(".", 1)[0]
                assert (job / "source.mp4").read_bytes() == video.read_bytes()
                old = time.time() - 7 * 24 * 3600 - 1
                os.utime(job / filename, (old, old))
                get(url, 410)
                get(f"/api/files/{date}/source.mp4", 400)
                get(f"/api/files/31022026/{filename}", 400)
                get(f"/api/files/{date}/invalid.mp3", 400)
                get("/storage/active/" + date + "/" + filename, 404)

                upload("/api/convert/image", "fake.png", b"invalid image", "png", 400)
                upload("/api/convert/image", "sample.png", image.read_bytes(), "exe", 400)
                upload("/api/convert/image", "sample.png", b"", "png", 400)
                upload("/api/convert/image", "large.png", b"x" * (20 * 1024 * 1024 + 1), "png", 413)
                upload("/api/convert/audio", "fake.mp4", b"invalid video", expected=400)
                upload("/api/convert/mute", "fake.mp4", b"invalid video", expected=400)
                silent = root / "silent.mp4"
                subprocess.run(["ffmpeg", "-v", "error", "-f", "lavfi", "-i", "color=c=black:s=32x32:d=1", "-an", "-c:v", "mpeg4", str(silent)], check=True)
                upload("/api/convert/audio", "silent.mp4", silent.read_bytes(), expected=400)
                assert not list((root / "storage" / "staging").iterdir()), "Working files leaked"
                # Keep an upload open: another operation from the same IP must be rejected.
                slow = socket.create_connection(("127.0.0.1", port), timeout=5)
                try:
                    slow.sendall((
                        "POST /api/convert/image HTTP/1.1\r\n"
                        f"Host: 127.0.0.1:{port}\r\n"
                        "Content-Type: multipart/form-data; boundary=SlowUpload\r\n"
                        "Content-Length: 100000\r\n\r\n"
                        "--SlowUpload\r\nContent-Disposition: form-data; name=\"file\"; filename=\"slow.png\"\r\n"
                        "Content-Type: image/png\r\n\r\npartial"
                    ).encode())
                    for _ in range(20):
                        try:
                            urllib.request.urlopen(urllib.request.Request(base + "/api/not-a-tool", data=b"", method="POST"), timeout=5).close()
                        except urllib.error.HTTPError as error:
                            if error.code == 429:
                                assert error.headers["Retry-After"] == "1"
                                assert b"operations running" in error.read()
                                break
                            assert error.code in (404, 405)
                        time.sleep(0.05)
                    else:
                        raise AssertionError("A parallel operation bypassed the per-client cap")
                    get("/")
                    get("/api/healthcheck")
                    import http.client
                    response = http.client.HTTPResponse(slow)
                    response.begin()
                    assert response.status == 408, response.status
                    response.read()
                finally:
                    slow.close()
                for _ in range(20):
                    try:
                        urllib.request.urlopen(urllib.request.Request(base + "/api/not-a-tool", data=b"", method="POST"), timeout=5).close()
                    except urllib.error.HTTPError as error:
                        if error.code in (404, 405):
                            break
                        assert error.code == 429
                    time.sleep(0.05)
                else:
                    raise AssertionError("Timed out upload did not release the client slot")
                recovered = upload("/api/convert/image", "sample.png", image.read_bytes(), "png")
                assert get(recovered)[0].startswith(b"\x89PNG")
                # Nonexistent API requests still consume the corresponding bucket.
                blocked = False
                for _ in range(101):
                    request = urllib.request.Request(base + "/api/not-a-tool", data=b"", method="POST")
                    try:
                        urllib.request.urlopen(request, timeout=5).close()
                    except urllib.error.HTTPError as error:
                        assert error.code in (404, 405, 429), error.code
                        if error.code == 429:
                            assert 1 <= int(error.headers["Retry-After"]) <= 60
                            blocked = True
                            break
                assert blocked, "POST rate limit was not enforced"
                request = urllib.request.Request(base + "/api/not-a-tool", data=b"", method="POST", headers={"X-Forwarded-For": "192.0.2.99", "CF-Connecting-IP": "192.0.2.99"})
                try:
                    urllib.request.urlopen(request, timeout=5).close()
                    raise AssertionError("Spoofed forwarding header bypassed the limit")
                except urllib.error.HTTPError as error:
                    assert error.code == 429
                get("/")
                get("/api/healthcheck")
                blocked = False
                for _ in range(101):
                    try:
                        urllib.request.urlopen(base + "/api/not-a-tool", timeout=5).close()
                    except urllib.error.HTTPError as error:
                        assert error.code in (404, 429), error.code
                        if error.code == 429:
                            assert 1 <= int(error.headers["Retry-After"]) <= 60
                            blocked = True
                            break
                assert blocked, "Read API rate limit was not enforced"
                # Restart with an impossible free-space reserve. Admission must fail
                # without invoking media tools, while completed files remain available.
                server.terminate()
                server.wait(timeout=10)
                environment["MIN_FREE_DISK_BYTES"] = str(2**64 - 1)
                server = subprocess.Popen([str(binary)], cwd=root, env=environment, stdout=log, stderr=log)
                for _ in range(100):
                    try:
                        urllib.request.urlopen(base + "/api/healthcheck", timeout=1).close()
                        break
                    except (urllib.error.URLError, TimeoutError):
                        if server.poll() is not None:
                            raise RuntimeError((root / "server.log").read_text())
                        time.sleep(0.1)
                else:
                    raise RuntimeError("Server did not restart")
                upload("/api/convert/image", "sample.png", image.read_bytes(), "png", expected=503)
                upload("/api/convert/audio", "sample.mp4", video.read_bytes(), expected=503)
                for endpoint, quality in [("/api/youtube/download", 720), ("/api/youtube/download/mp3", 192)]:
                    request = urllib.request.Request(base + endpoint,
                        data=json.dumps({"url": "https://youtu.be/dQw4w9WgXcQ", "quality": quality}).encode(),
                        headers={"Content-Type": "application/json"})
                    try:
                        urllib.request.urlopen(request, timeout=5).close()
                        raise AssertionError("Low disk reserve did not reject YouTube admission")
                    except urllib.error.HTTPError as error:
                        assert error.code == 503, (error.code, error.read())
                assert get(recovered)[0].startswith(b"\x89PNG")
                assert (job / filename).exists(), "Low disk archiving removed original files"
                assert not list((root / "storage" / "staging").iterdir()), "Rejected jobs leaked staging files"
                print("Media smoke checks passed: conversions, headers, slow upload timeout, storage admission, expiry, and invalid uploads.")
            finally:
                server.terminate()
                try:
                    server.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    server.kill()
                    server.wait()


if __name__ == "__main__":
    main()
