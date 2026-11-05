#!/usr/bin/env python3
r"""
End-to-end interoperability test.

Starts a real `veil` server together with a local HTTP target, then drives
traffic through it with an external client for each protocol:

  vless-tls  \  Xray-core (v2rayN's bundled xray.exe, or `xray` in PATH)
  vless-ws   |   -> real interop with the mainstream client ecosystem
  trojan-tls |
  vmess-ws   /

  hysteria2  \  `veil client` (our own QUIC framing, see docs/PROTOCOLS.md)

Usage:
    python scripts/interop.py                 # auto-detect xray
    XRAY=/usr/local/bin/xray python scripts/interop.py
    python scripts/interop.py --skip xray     # only test the veil client path
"""

from __future__ import annotations

import base64
import hashlib
import http.server
import json
import os
import shutil
import socket
import subprocess
import sys
import tempfile
import threading
import time
import urllib.request

HTTP_PORT = 18080
SOCKS_PORT = 11080

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
IS_WIN = os.name == "nt"
BIN = os.environ.get(
    "VEIL_BIN",
    os.path.join(ROOT, "target", "release", "veil.exe" if IS_WIN else "veil"),
)


def find_xray() -> str | None:
    env = os.environ.get("XRAY")
    if env and os.path.exists(env):
        return env
    p = shutil.which("xray")
    if p:
        return p
    guess = r"C:\Users\Administrator\Desktop\v2rayN-windows-64\bin\xray\xray.exe"
    if os.path.exists(guess):
        return guess
    return None


def leaf_pin(cert_file: str) -> str:
    """SHA-256 (hex) of the first certificate in a PEM chain — what Xray calls
    `pinnedPeerCertSha256`. Lets clients trust a self-signed node without
    importing the CA into the OS store."""
    text = open(cert_file, encoding="utf-8").read()
    body = text.split("-----BEGIN CERTIFICATE-----")[1].split("-----END CERTIFICATE-----")[0]
    der = base64.b64decode("".join(body.split()))
    return hashlib.sha256(der).hexdigest()


def load_toml(path: str) -> dict:
    try:
        import tomllib
    except ImportError:  # pragma: no cover
        sys.exit("python >= 3.11 is required (tomllib)")
    with open(path, "rb") as fh:
        return tomllib.load(fh)


def free_port(start: int) -> int:
    port = start
    while port < start + 200:
        with socket.socket() as s:
            try:
                s.bind(("127.0.0.1", port))
                return port
            except OSError:
                port += 1
    raise RuntimeError("no free port")


def wait_port(port: int, timeout: float = 15.0) -> bool:
    deadline = time.time() + timeout
    while time.time() < deadline:
        with socket.socket() as s:
            s.settimeout(0.4)
            if s.connect_ex(("127.0.0.1", port)) == 0:
                return True
        time.sleep(0.15)
    return False


class Target(http.server.BaseHTTPRequestHandler):
    body = b"VEIL-INTEROP-OK"

    def do_GET(self):  # noqa: N802
        self.send_response(200)
        self.send_header("Content-Length", str(len(self.body)))
        self.send_header("Content-Type", "text/plain")
        self.end_headers()
        self.wfile.write(self.body)

    def log_message(self, *args):
        pass


def fetch_through_socks(port: int) -> bytes:
    """curl is the least surprising way to speak SOCKS5."""
    url = f"http://127.0.0.1:{HTTP_PORT}/"
    out = subprocess.run(
        ["curl", "-s", "--max-time", "10", "--socks5-hostname", f"127.0.0.1:{port}", url],
        capture_output=True,
    )
    return out.stdout


def xray_config(
    protocol: str, transport: str, node: dict, user: dict, port: int, cert_file: str
) -> dict:
    if protocol == "vless":
        outbound = {
            "protocol": "vless",
            "settings": {
                "vnext": [
                    {
                        "address": "127.0.0.1",
                        "port": port,
                        "users": [{"id": user["uuid"], "encryption": "none"}],
                    }
                ]
            },
        }
    elif protocol == "trojan":
        outbound = {
            "protocol": "trojan",
            "settings": {
                "servers": [
                    {
                        "address": "127.0.0.1",
                        "port": port,
                        "password": user["password"],
                    }
                ]
            },
        }
    else:
        outbound = {
            "protocol": "vmess",
            "settings": {
                "vnext": [
                    {
                        "address": "127.0.0.1",
                        "port": port,
                        "users": [
                            {"id": user["uuid"], "alterId": 0, "security": "auto"}
                        ],
                    }
                ]
            },
        }

    stream = {
        "security": "tls",
        "tlsSettings": {
            "serverName": "localhost",
            # Xray >= 25 removed allowInsecure; pinning the leaf certificate is
            # the supported way to talk to a node with a self-signed cert.
            "pinnedPeerCertSha256": leaf_pin(cert_file),
        },
    }
    if transport == "ws":
        stream["network"] = "ws"
        stream["wsSettings"] = {"path": node["path"], "headers": {"Host": "localhost"}}
    else:
        stream["network"] = "tcp"
    outbound["streamSettings"] = stream

    return {
        "log": {"loglevel": "debug"},
        "inbounds": [
            {
                "listen": "127.0.0.1",
                "port": SOCKS_PORT,
                "protocol": "socks",
                "settings": {"udp": False},
            }
        ],
        "outbounds": [outbound, {"protocol": "freedom", "tag": "direct"}],
    }


def main() -> int:
    if not os.path.exists(BIN):
        sys.exit(f"veil binary not found at {BIN}; run `cargo build --release` first")

    args = sys.argv[1:]
    skip_xray = "--skip" in args and "xray" in args
    xray = None if skip_xray else find_xray()

    work = tempfile.mkdtemp(prefix="veil-interop-")
    conf = os.path.join(work, "veil.toml")
    subprocess.run([BIN, "init", "-c", conf, "--domain", "localhost"], check=True,
                   stdout=subprocess.DEVNULL)

    # Move every inbound onto a high, non-privileged port.
    cfg = load_toml(conf)
    ports = {}
    for i, ib in enumerate(cfg["inbounds"]):
        ports[ib["tag"]] = 14000 + i
    text = open(conf, encoding="utf-8").read()
    for tag, port in ports.items():
        old = None
        for ib in cfg["inbounds"]:
            if ib["tag"] == tag:
                old = f"0.0.0.0:{ib['listen'].split(':')[-1]}"
        if old:
            text = text.replace(f'listen = "{old}"', f'listen = "0.0.0.0:{port}"', 1)
    text = text.replace('level = "info"', 'level = "debug"')
    open(conf, "w", encoding="utf-8").write(text)

    cfg = load_toml(conf)
    user = cfg["users"][0]
    nodes = {ib["tag"]: ib for ib in cfg["inbounds"]}

    target = http.server.ThreadingHTTPServer(("127.0.0.1", HTTP_PORT), Target)
    threading.Thread(target=target.serve_forever, daemon=True).start()

    server_log = open(os.path.join(work, "server.log"), "w", encoding="utf-8")
    server = subprocess.Popen([BIN, "run", "-c", conf], stdout=server_log, stderr=server_log)
    procs = [server]
    results: list[tuple[str, bool, str]] = []
    try:
        if not wait_port(ports["vless-tls"]):
            server_log.flush()
            print(open(os.path.join(work, "server.log"), encoding="utf-8",
                       errors="replace").read()[-3000:])
            sys.exit("veil server did not start")
        time.sleep(0.4)

        # ---- Xray-driven cases -------------------------------------------------
        cases = [
            ("vless-tls", "vless", "tcp"),
            ("vless-ws", "vless", "ws"),
            ("trojan-tls", "trojan", "tcp"),
            ("vmess-ws", "vmess", "ws"),
        ]
        for tag, proto, transport in cases:
            label = f"{tag} (xray)"
            if not xray:
                results.append((label, False, "xray not found — skipped"))
                continue
            xconf = os.path.join(work, f"xray-{tag}.json")
            with open(xconf, "w", encoding="utf-8") as fh:
                json.dump(xray_config(proto, transport, nodes[tag], user, ports[tag], cfg["server"]["cert"]), fh)
            xlog = open(os.path.join(work, f"xray-{tag}.log"), "w")
            xp = subprocess.Popen([xray, "run", "-c", xconf], stdout=xlog, stderr=xlog)
            procs.append(xp)
            ok = False
            detail = "socks port never opened"
            if wait_port(SOCKS_PORT, 8):
                body = fetch_through_socks(SOCKS_PORT)
                ok = Target.body in body
                detail = "ok" if ok else f"got {body[:80]!r}"
            xp.terminate()
            procs.remove(xp)
            time.sleep(0.3)
            results.append((label, ok, detail))

        # ---- Our own client, every protocol ------------------------------------
        for tag, ib in nodes.items():
            label = f"{tag} (veil client)"
            lsocks = free_port(21000)
            lproc = subprocess.Popen(
                [BIN, "client", "-c", conf, "--node", tag, "--listen", f"127.0.0.1:{lsocks}"],
                stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
            )
            procs.append(lproc)
            ok = False
            detail = "local proxy never opened"
            if wait_port(lsocks, 8):
                body = fetch_through_socks(lsocks)
                ok = Target.body in body
                detail = "ok" if ok else f"got {body[:80]!r}"
            lproc.terminate()
            procs.remove(lproc)
            time.sleep(0.2)
            results.append((label, ok, detail))

        # ---- API ---------------------------------------------------------------
        try:
            api = cfg["api"]["listen"]
            with urllib.request.urlopen(f"http://{api}/api/health", timeout=5) as r:
                health = json.load(r)
            results.append(("management API", health.get("status") == "ok", json.dumps(health)))
        except Exception as exc:  # noqa: BLE001
            results.append(("management API", False, str(exc)))

        # ---- subscription ------------------------------------------------------
        try:
            token = user["sub_token"]
            api = cfg["api"]["listen"]
            with urllib.request.urlopen(f"http://{api}/sub/{token}", timeout=5) as r:
                blob = r.read().decode()
            decoded = base64.b64decode(blob).decode()
            count = len([l for l in decoded.splitlines() if "://" in l])
            results.append(("subscription", count == len(nodes), f"{count} links"))
        except Exception as exc:  # noqa: BLE001
            results.append(("subscription", False, str(exc)))

    finally:
        for p in procs:
            p.terminate()
        target.shutdown()
        server_log.close()

    print("\n" + "=" * 62)
    print(f"{'case':32} {'result':8} detail")
    print("-" * 62)
    for label, ok, detail in results:
        print(f"{label:32} {'PASS' if ok else 'FAIL':8} {detail}")
    print("=" * 62)
    failed = [r for r in results if not r[1]]
    if failed:
        print(f"{len(failed)} case(s) failed. server log: {os.path.join(work, 'server.log')}")
        return 1
    print(f"all {len(results)} cases passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
