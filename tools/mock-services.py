#!/usr/bin/env python3
"""Mock home services for testing Clippy's integrations and custom actions without real servers.
Run: python3 tools/mock-services.py [port]   (default 8765), then point an integration at http://127.0.0.1:<port>.
Expected secrets (CLIPPY_SECRET_<NAME> in test mode): any non-empty value; Pi-hole password "pw"; qBittorrent password "pw"."""
import json, sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

LOG = []
LIGHTS = {"light.desk": "on", "light.ceiling": "off", "light.stuck": "off"}  # light.stuck ignores commands, like an offline bulb
class H(BaseHTTPRequestHandler):
    def log_message(self, *a): pass
    def send(self, code, obj=None, headers=None, raw=None):
        body = raw if raw is not None else (b"" if obj is None else json.dumps(obj).encode())
        self.send_response(code)
        for k, v in (headers or {}).items(): self.send_header(k, v)
        self.send_header("Content-Length", str(len(body))); self.end_headers(); self.wfile.write(body)
    def body(self):
        n = int(self.headers.get("Content-Length") or 0)
        return self.rfile.read(n) if n else b""
    def do_GET(self): self.route("GET")
    def do_POST(self): self.route("POST")
    def do_PUT(self): self.route("PUT")
    def route(self, m):
        p = self.path; b = self.body()
        LOG.append((m, p, len(b)))
        auth = self.headers.get("Authorization", "")
        if p == "/_log": return self.send(200, LOG)
        # Home Assistant
        if p == "/api/": return self.send(200, {"message": "API running."}) if auth.startswith("Bearer ") else self.send(401, {"message": "no"})
        if p == "/api/states": return self.send(200, [
            {"entity_id": "light.desk", "state": LIGHTS["light.desk"], "attributes": {"friendly_name": "Desk lamp"}},
            {"entity_id": "light.ceiling", "state": LIGHTS["light.ceiling"], "attributes": {"friendly_name": "Ceiling"}},
            {"entity_id": "light.stuck", "state": LIGHTS["light.stuck"], "attributes": {"friendly_name": "Porch light"}},
            {"entity_id": "light.broken", "state": "unavailable", "attributes": {"friendly_name": "Broken bulb"}},
            {"entity_id": "switch.ac", "state": "on", "attributes": {"friendly_name": "AC plug"}},
            {"entity_id": "sensor.living_temp", "state": "26.4", "attributes": {"friendly_name": "Living room temperature", "device_class": "temperature", "unit_of_measurement": "°C"}},
            {"entity_id": "sensor.living_hum", "state": "61", "attributes": {"friendly_name": "Living room humidity", "device_class": "humidity", "unit_of_measurement": "%"}},
            {"entity_id": "sensor.phone_battery", "state": "unknown", "attributes": {"friendly_name": "Phone battery", "device_class": "battery"}},
            {"entity_id": "climate.bedroom", "state": "cool", "attributes": {"friendly_name": "Bedroom AC", "hvac_action": "cooling", "current_temperature": 25, "temperature": 23, "current_humidity": 55}},
            {"entity_id": "binary_sensor.front_door", "state": "off", "attributes": {"friendly_name": "Front door", "device_class": "door"}},
            {"entity_id": "weather.home", "state": "partlycloudy", "attributes": {"friendly_name": "Home", "temperature": 29, "temperature_unit": "°C", "humidity": 70}}])
        if p.startswith("/api/services/light/"):
            ids = json.loads(b or b"{}").get("entity_id", [])
            for x in ids:
                if x in LIGHTS and x != "light.stuck": LIGHTS[x] = "on" if p.endswith("turn_on") else "off"
            return self.send(200, [{"entity_id": x} for x in ids])
        # Proxmox
        if p == "/api2/json/cluster/resources":
            if not auth.startswith("PVEAPIToken=root@pam!clippy="): return self.send(401, {"data": None})
            return self.send(200, {"data": [
                {"type": "node", "node": "pve", "status": "online", "cpu": 0.12, "mem": 8e9, "maxmem": 16e9},
                {"type": "lxc", "vmid": 101, "name": "docker", "node": "pve", "status": "running"},
                {"type": "qemu", "vmid": 200, "name": "windows", "node": "pve", "status": "stopped"},
                {"type": "qemu", "vmid": 900, "name": "tmpl", "node": "pve", "status": "stopped", "template": 1}]})
        if p.startswith("/api2/json/nodes/") and p.endswith("/status/start"): return self.send(200, {"data": "UPID:pve:start"})
        # Portainer
        if p == "/api/endpoints": return self.send(200, [{"Id": 3, "Name": "local"}])
        if p.startswith("/api/endpoints/3/docker/containers/json"): return self.send(200, [
            {"Id": "a" * 64, "Names": ["/web"], "State": "running", "Status": "Up 2 hours (healthy)"},
            {"Id": "b" * 64, "Names": ["/db"], "State": "running", "Status": "Up 5 minutes (unhealthy)"},
            {"Id": "c" * 64, "Names": ["/old"], "State": "exited", "Status": "Exited (1) 3 hours ago"}])
        if "/docker/containers/" in p and p.endswith("/restart"): return self.send(204)
        # Pi-hole v6
        if p == "/api/auth": return self.send(200, {"session": {"valid": json.loads(b or b"{}").get("password") == "pw", "sid": "SID123"}})
        if p == "/api/stats/summary": return self.send(200, {"queries": {"total": 12000, "blocked": 1500, "percent_blocked": 12.5}, "gravity": {"domains_being_blocked": 160000}}) if self.headers.get("X-FTL-SID") == "SID123" else self.send(401, {"error": {"key": "unauthorized"}})
        if p == "/api/dns/blocking": return self.send(200, {"blocking": "enabled" if m == "GET" else ("disabled" if b'"blocking": false' in b or b'"blocking":false' in b else "enabled"), "timer": None})
        # Jellyfin
        if p.startswith("/Sessions"): return self.send(200, [{"UserName": "sam", "DeviceName": "TV", "NowPlayingItem": {"Name": "Pilot", "SeriesName": "Severance"}, "PlayState": {"IsPaused": False}}, {"UserName": "x"}])
        if p.startswith("/Items"): return self.send(200, {"Items": [{"Id": "i1", "Name": "Dune"}, {"Id": "i2", "Name": "Ep 2", "SeriesName": "Andor"}]})
        # Radarr / Sonarr
        if p.startswith("/api/v3/queue"): return self.send(200, {"records": [{"title": "x", "movie": {"title": "Dune Part Two"}, "series": {"title": "Andor"}, "episode": {"seasonNumber": 2, "episodeNumber": 3}, "size": 1000, "sizeleft": 250, "timeleft": "00:10:00"}]})
        if p.startswith("/api/v3/calendar"): return self.send(200, [{"title": "Superman", "digitalRelease": "2026-11-25T00:00:00Z", "airDateUtc": "2026-11-24T02:00:00Z", "series": {"title": "Slow Horses"}, "seasonNumber": 5, "episodeNumber": 1}])
        if p.startswith("/api/v3/history"): return self.send(200, {"records": [{"id": 77, "sourceTitle": "raw.release.name", "movie": {"title": "Oppenheimer"}, "series": {"title": "Shogun"}, "episode": {"seasonNumber": 1, "episodeNumber": 4}, "date": "2026-11-22T07:00:00Z"}]})
        # qBittorrent
        if p == "/api/v2/auth/login":
            ok = b"password=pw" in b
            return self.send(200, raw=b"Ok." if ok else b"Fails.", headers={"Set-Cookie": "SID=qb1; path=/"} if ok else {})
        if p == "/api/v2/torrents/info":
            if "SID=qb1" not in (self.headers.get("Cookie") or ""): return self.send(403, raw=b"Forbidden")
            return self.send(200, [{"hash": "h1", "name": "ubuntu.iso", "progress": 0.42, "state": "downloading", "dlspeed": 5e6, "eta": 60}, {"hash": "h2", "name": "done.mkv", "progress": 1, "state": "uploading", "dlspeed": 0, "eta": 0}])
        # custom actions (an upload server and a downloader)
        if p.startswith("/api/upload") and m == "PUT":
            if self.headers.get("X-Owner") != "owner-token-1234567890": return self.send(403, {"error": "bad owner"})
            return self.send(200, {"id": "f7k2", "bytes": len(b)})
        if p == "/api/jobs" and m == "POST": return self.send(200, {"ids": ["j1"], "echo": json.loads(b or b"{}")})
        if p == "/api/state": return self.send(200, {"jobs": [{"id": "j1", "status": "done", "item": {"title": "Big Buck Bunny"}}, {"id": "j9", "status": "running"}]})
        return self.send(404, {"error": "not found: " + p})

port = int(sys.argv[1]) if len(sys.argv) > 1 else 8765
ThreadingHTTPServer(("127.0.0.1", port), H).serve_forever()
