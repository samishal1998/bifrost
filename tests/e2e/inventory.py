#!/usr/bin/env python3
"""E2E HTTP inventory for p10 (contract §12). Usage: inventory.py HOST:PORT FILE

Serves FILE (re-read on every request) as JSON to any GET path; 401 unless the request carries
`Authorization: Bearer $BF_TOKEN` (run.sh exports BF_TOKEN=s3cret; p10 restarts this server with another
BF_TOKEN to make the daemon's token wrong).
"""
import os
import sys
from http.server import BaseHTTPRequestHandler, HTTPServer

host, port = sys.argv[1].rsplit(":", 1)
path, want = sys.argv[2], "Bearer " + os.environ["BF_TOKEN"]


class Inventory(BaseHTTPRequestHandler):
    def do_GET(self):
        if self.headers.get("Authorization") == want:
            code, body = 200, open(path, "rb").read()
        else:
            code, body = 401, b'{"error":"unauthorized"}'
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *args):
        pass  # one request per discovery_interval: keep the run output quiet


HTTPServer((host, int(port)), Inventory).serve_forever()
