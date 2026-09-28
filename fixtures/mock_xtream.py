#!/usr/bin/env python3
"""Minimal mock Xtream Codes panel for local end-to-end testing (no real content).

    python3 fixtures/mock_xtream.py 8090 /path/to/live.ts

Serves player_api.php (auth, categories, live/vod/series lists, series_info), xmltv.php (EPG for
the first 200 channels, 3 days), get.php (M3U), and /live|/movie|/series/<u>/<p>/<id>.<ext>
(all mapped to the same local MPEG-TS file so playback works offline), plus /portal.php — a tiny
Stalker/Ministra portal (MAC 00:1A:79:12:34:56, 60 channels, create_link → the same stream).
Credentials: user / pass. Everything is synthetic.
"""
import gzip
import json
import os
import random
import sys
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import parse_qs, urlparse

PORT = int(sys.argv[1]) if len(sys.argv) > 1 else 8090
LIVE_KBPS = int(os.environ.get("MOCK_LIVE_KBPS", "8000"))
MEDIA = sys.argv[2] if len(sys.argv) > 2 else None
USER, PASS = "user", "pass"
STALKER_TOKEN = "mock-stalker-token"
N_LIVE, N_MOVIES, N_SERIES = 1000, 240, 24
LIVE_CATS = ["US | News", "US | Sports", "UK | Entertainment", "FR | Général", "DE | Sport", "Kids", "Música", "24/7", "Adult XXX"]
VOD_CATS = ["Action", "Comedy", "Drama", "Sci-Fi", "Documentary", "Kids"]
SERIES_CATS = ["Drama", "Comedy", "Crime"]
random.seed(7)
SHOWS = ["Morning Brief", "World Report", "Match of the Day", "The Late Show", "Nature Hour", "Cooking with Fire", "Tech Weekly", "Crime Files", "Kids Zone", "Movie Night", "Talk Tonight", "Science Now"]


def live_streams():
    out = []
    for i in range(1, N_LIVE + 1):
        cat = (i - 1) % len(LIVE_CATS)
        out.append({
            "num": i, "name": f"{LIVE_CATS[cat].split(' | ')[0]} Chännel {i} HD", "stream_type": "live",
            "stream_id": i, "stream_icon": f"http://127.0.0.1:{PORT}/logo/{i}.png",
            "epg_channel_id": f"ch{i}.mock" if i <= 200 else "",
            "added": str(1_700_000_000 + i), "category_id": str(cat + 1), "custom_sid": "",
            "tv_archive": 1 if i % 3 == 0 else 0, "direct_source": "", "tv_archive_duration": 3 if i % 3 == 0 else 0,
        })
    return out


def vod_streams():
    out = []
    for i in range(1, N_MOVIES + 1):
        out.append({
            "num": i, "name": f"Mock Movie {i} ({1990 + i % 35})", "stream_type": "movie", "stream_id": 5000 + i,
            "stream_icon": f"http://127.0.0.1:{PORT}/poster/{i}.jpg", "rating": str(round(4 + (i % 60) / 10, 1)),
            "rating_5based": round((4 + (i % 60) / 10) / 2, 1), "added": str(1_690_000_000 + i * 3600),
            "category_id": str((i % len(VOD_CATS)) + 1), "container_extension": "mkv", "custom_sid": "", "direct_source": "",
        })
    return out


def series_list():
    out = []
    for i in range(1, N_SERIES + 1):
        out.append({
            "num": i, "name": f"Mock Series {i}", "series_id": 9000 + i, "cover": f"http://127.0.0.1:{PORT}/poster/s{i}.jpg",
            "plot": f"A synthetic series number {i} used for testing the desktop player.", "cast": "", "director": "",
            "genre": SERIES_CATS[i % len(SERIES_CATS)], "releaseDate": f"{2000 + i % 25}-03-01", "last_modified": str(1_695_000_000 + i * 7200),
            "rating": "7", "rating_5based": 3.5, "backdrop_path": [f"http://127.0.0.1:{PORT}/backdrop/{i}.jpg"],
            "youtube_trailer": "", "episode_run_time": "42", "category_id": str((i % len(SERIES_CATS)) + 1),
        })
    return out


def series_info(series_id):
    sid = int(series_id)
    seasons = {}
    for s in (1, 2):
        seasons[str(s)] = [{
            "id": str(sid * 100 + s * 10 + e), "episode_num": e, "title": f"S{s}E{e} · Episode {e}",
            "container_extension": "mkv", "season": s,
            "info": {"duration_secs": 600 + e * 60, "duration": "00:11:00", "movie_image": f"http://127.0.0.1:{PORT}/still/{sid}-{s}-{e}.jpg", "plot": "Synthetic episode."},
        } for e in range(1, 5)]
    return {"seasons": [], "info": {"name": f"Mock Series {sid - 9000}", "cover": f"http://127.0.0.1:{PORT}/poster/s{sid - 9000}.jpg", "plot": "Synthetic.", "genre": "Drama"}, "episodes": seasons}


def xmltv():
    now = int(time.time())
    start0 = now - now % 1800 - 6 * 3600
    parts = ['<?xml version="1.0" encoding="UTF-8"?>\n<tv generator-info-name="mock-xtream">\n']
    for i in range(1, 201):
        parts.append(f'  <channel id="ch{i}.mock"><display-name>Chännel {i}</display-name></channel>\n')
    for i in range(1, 201):
        t = start0
        while t < now + 3 * 86400:
            dur = random.choice([1800, 1800, 3600, 5400, 7200])
            title = random.choice(SHOWS)
            s = time.strftime("%Y%m%d%H%M%S", time.gmtime(t))
            e = time.strftime("%Y%m%d%H%M%S", time.gmtime(t + dur))
            parts.append(f'  <programme start="{s} +0000" stop="{e} +0000" channel="ch{i}.mock"><title>{title}</title><desc>Synthetic description for {title} on channel {i} &amp; friends.</desc></programme>\n')
            t += dur
    parts.append("</tv>\n")
    return "".join(parts).encode("utf-8")


XMLTV_GZ = None


class H(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, fmt, *args):  # quieter
        sys.stderr.write("%s %s\n" % (self.command, self.path.split("?")[0]))

    def send(self, code, body, ctype="application/json"):
        self.send_response(code)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def stalker(self, q):
        """Minimal Stalker/Ministra portal: MAC cookie 00:1A:79:12:34:56, bearer token after
        handshake, genres, get_all_channels (first 60 channels), create_link → local live stream."""
        cookie = self.headers.get("Cookie", "")
        if "mac=00%3A1A%3A79%3A12%3A34%3A56" not in cookie and "mac=00:1A:79:12:34:56" not in cookie:
            return self.send(200, b"Authorization failed", "text/plain")
        kind, action = q.get("type", [""])[0], q.get("action", [""])[0]
        if kind == "stb" and action == "handshake":
            return self.send(200, json.dumps({"js": {"token": STALKER_TOKEN, "random": "r"}}).encode())
        if self.headers.get("Authorization", "") != f"Bearer {STALKER_TOKEN}":
            return self.send(403, b"", "text/plain")
        if kind == "stb" and action == "get_profile":
            return self.send(200, json.dumps({"js": {"id": 7, "default_timezone": "UTC", "status": 0}}).encode())
        if kind == "account_info":
            return self.send(200, json.dumps({"js": {"phone": "Expires: 2027-06-30", "mac": "00:1A:79:12:34:56"}}).encode())
        if kind == "itv" and action == "get_genres":
            return self.send(200, json.dumps({"js": [{"id": str(i + 1), "title": LIVE_CATS[i]} for i in range(len(LIVE_CATS))]}).encode())
        if kind == "itv" and action == "get_all_channels":
            data = [{"id": str(i), "name": f"Portal Chännel {i}", "number": str(i), "cmd": f"ffmpeg http://localhost/ch/{i}_",
                     "tv_genre_id": str((i % len(LIVE_CATS)) + 1), "logo": "", "xmltv_id": f"ch{i}.mock", "archive": "0"}
                    for i in range(1, 61)]
            return self.send(200, json.dumps({"js": {"data": data}}).encode())
        if kind == "itv" and action == "create_link":
            cmd = q.get("cmd", [""])[0]
            ch = cmd.rsplit("/", 1)[-1].rstrip("_") or "1"
            return self.send(200, json.dumps({"js": {"id": ch, "cmd": f"ffmpeg http://127.0.0.1:{PORT}/live/{USER}/{PASS}/{ch}.ts?token=portal-{ch}"}}).encode())
        return self.send(200, json.dumps({"js": {}}).encode())

    def stream_live(self):
        """Loop the sample file forever as a chunked live stream, paced at LIVE_KBPS (like a real
        8 Mbps channel) so recorders/downloaders behave as they would against a provider."""
        self.send_response(200)
        self.send_header("Content-Type", "video/mp2t")
        self.send_header("Transfer-Encoding", "chunked")
        self.end_headers()
        chunk = 64 * 1024
        per_chunk = chunk * 8 / (LIVE_KBPS * 1000.0)
        try:
            while True:
                with open(MEDIA, "rb") as f:
                    while True:
                        data = f.read(chunk)
                        if not data:
                            break
                        self.wfile.write(b"%x\r\n%s\r\n" % (len(data), data))
                        self.wfile.flush()
                        time.sleep(per_chunk)
        except (BrokenPipeError, ConnectionResetError, OSError):
            return

    def do_GET(self):
        u = urlparse(self.path)
        q = parse_qs(u.query)
        if u.path == "/portal.php":
            return self.stalker(q)
        if u.path == "/player_api.php":
            if q.get("username", [""])[0] != USER or q.get("password", [""])[0] != PASS:
                return self.send(200, json.dumps({"user_info": {"auth": 0, "status": "Disabled"}}).encode())
            action = q.get("action", [""])[0]
            if action == "":
                body = {"user_info": {"username": USER, "password": PASS, "message": "", "auth": 1, "status": "Active",
                                      "exp_date": str(int(time.time()) + 86400 * 90), "is_trial": "0", "active_cons": "1",
                                      "created_at": "1690000000", "max_connections": "2", "allowed_output_formats": ["m3u8", "ts"]},
                        "server_info": {"url": "127.0.0.1", "port": str(PORT), "https_port": "443", "server_protocol": "http",
                                        "rtmp_port": "0", "timezone": "UTC", "timestamp_now": int(time.time()), "time_now": "now"}}
            elif action == "get_live_categories":
                body = [{"category_id": str(i + 1), "category_name": c, "parent_id": 0} for i, c in enumerate(LIVE_CATS)]
            elif action == "get_vod_categories":
                body = [{"category_id": str(i + 1), "category_name": c, "parent_id": 0} for i, c in enumerate(VOD_CATS)]
            elif action == "get_series_categories":
                body = [{"category_id": str(i + 1), "category_name": c, "parent_id": 0} for i, c in enumerate(SERIES_CATS)]
            elif action == "get_live_streams":
                body = live_streams()
            elif action == "get_vod_streams":
                body = vod_streams()
            elif action == "get_series":
                body = series_list()
            elif action == "get_series_info":
                body = series_info(q.get("series_id", ["9001"])[0])
            else:
                body = []
            return self.send(200, json.dumps(body).encode())
        if u.path == "/xmltv.php":
            global XMLTV_GZ
            if XMLTV_GZ is None:
                XMLTV_GZ = gzip.compress(xmltv())
            return self.send(200, XMLTV_GZ, "application/gzip")
        if u.path.startswith(("/live/", "/movie/", "/series/")):
            parts = u.path.split("/")
            if len(parts) >= 5 and parts[2] == USER and parts[3] == PASS and MEDIA and os.path.exists(MEDIA):
                if parts[1] == "live":
                    return self.stream_live()
                with open(MEDIA, "rb") as f:
                    data = f.read()
                return self.send(200, data, "video/mp2t")
            return self.send(403, b"forbidden", "text/plain")
        if u.path == "/get.php":
            return self.send(200, b"#EXTM3U\n", "audio/mpegurl")
        return self.send(404, b"<html><body>not here</body></html>", "text/html")


if __name__ == "__main__":
    print(f"mock xtream on http://127.0.0.1:{PORT}  user={USER} pass={PASS}", flush=True)
    ThreadingHTTPServer(("127.0.0.1", PORT), H).serve_forever()
