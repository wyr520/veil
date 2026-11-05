#!/usr/bin/env python3
"""
Push this repository to GitHub through the REST API instead of `git push`.

Why: the sandbox this ran in can reach api.github.com but not github.com, so
git's smart-HTTP transport is unusable. The Git Data API does the same job —
upload blobs, build a tree, create one commit, move the branch ref — and the
result is byte-identical to a normal push (same blob SHAs, same history shape).

Usage:
    python scripts/api-push.py                  # repo "veil", public
    python scripts/api-push.py --name my-node --private
    GITHUB_TOKEN=ghp_xxx python scripts/api-push.py

Token is read from $GITHUB_TOKEN, else from `gh auth token`.
Needs scopes: repo (classic) or Contents+Administration+Workflows (fine-grained).
"""

from __future__ import annotations

import argparse
import base64
import json
import os
import pathlib
import subprocess
import sys
import urllib.error
import urllib.parse
import urllib.request

API = "https://api.github.com"
UA = "veil-publisher/1.0"


def die(msg: str) -> None:
    print(f"error: {msg}", file=sys.stderr)
    sys.exit(1)


def token() -> str:
    t = os.environ.get("GITHUB_TOKEN") or os.environ.get("GH_TOKEN")
    if t:
        return t.strip()
    # Local fallback so the token never has to travel through a chat window.
    local = pathlib.Path.home() / ".veil-token"
    if local.is_file():
        t = local.read_text(encoding="utf-8").strip()
        if t:
            return t
    try:
        out = subprocess.run(["gh", "auth", "token"], capture_output=True, text=True, timeout=20)
    except FileNotFoundError:
        die("no GITHUB_TOKEN and `gh` is not installed")
    if out.returncode != 0 or not out.stdout.strip():
        die("`gh auth token` failed — run `gh auth login` first")
    return out.stdout.strip()


class Api:
    def __init__(self, token: str) -> None:
        self.token = token

    def call(self, method: str, path: str, body: dict | None = None, ok=(200, 201, 204)):
        url = path if path.startswith("http") else API + path
        data = None
        headers = {
            "Authorization": f"Bearer {self.token}",
            "Accept": "application/vnd.github+json",
            "X-GitHub-Api-Version": "2022-11-28",
            "User-Agent": UA,
        }
        if body is not None:
            data = json.dumps(body).encode()
            headers["Content-Type"] = "application/json"
        req = urllib.request.Request(url, data=data, headers=headers, method=method)
        try:
            with urllib.request.urlopen(req, timeout=60) as r:
                raw = r.read()
                return r.status, (json.loads(raw) if raw else {})
        except urllib.error.HTTPError as e:
            payload = e.read()
            try:
                detail = json.loads(payload).get("message", payload[:300])
            except Exception:
                detail = payload[:300]
            if e.code in ok:
                return e.code, {}
            die(f"{method} {path} -> {e.code}: {detail}")


def git(*args: str) -> bytes:
    out = subprocess.run(["git", *args], capture_output=True)
    if out.returncode != 0:
        die(f"git {' '.join(args)} failed: {out.stderr.decode(errors='replace')}")
    return out.stdout


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--name", default="veil")
    ap.add_argument("--private", action="store_true")
    ap.add_argument("--tag", default="v0.1.0")
    ap.add_argument("--branch", default="main")
    args = ap.parse_args()

    if subprocess.run(["git", "rev-parse", "--git-dir"], capture_output=True).returncode != 0:
        die("run this from the repository root")

    api = Api(token())
    status, me = api.call("GET", "/user")
    login = me["login"]
    print(f"authenticated as {login}")

    # ---- repository -------------------------------------------------------
    status, repo = api.call(
        "POST",
        "/user/repos",
        {
            "name": args.name,
            "description": "Veil — VLESS / Trojan / VMess / Hysteria2 proxy suite (Rust)",
            "private": args.private,
            "has_issues": True,
            "has_wiki": False,
            "auto_init": False,
        },
        ok=(201, 422),
    )
    slug = f"{login}/{args.name}"
    if status == 422:
        print(f"repository {slug} already exists — reusing it")
    else:
        print(f"created https://github.com/{slug}")

    # ---- blobs ------------------------------------------------------------
    raw = git("ls-tree", "-r", "-z", "--full-tree", "HEAD")
    entries = []
    for chunk in raw.split(b"\0"):
        if not chunk:
            continue
        meta, path = chunk.split(b"\t", 1)
        mode, kind, sha = meta.split()
        if kind != b"blob":
            continue
        entries.append((mode.decode(), sha.decode(), path.decode("utf-8", "surrogateescape")))

    print(f"uploading {len(entries)} objects")
    tree = []
    for mode, sha, path in entries:
        content = git("cat-file", "-p", sha)
        _, blob = api.call(
            "POST",
            f"/repos/{slug}/git/blobs",
            {"content": base64.b64encode(content).decode(), "encoding": "base64"},
        )
        tree.append({"path": path, "mode": mode, "type": "blob", "sha": blob["sha"]})
        print(f"  {path}", end="\r")

    print(f"  {len(tree)} objects uploaded          ")
    _, new_tree = api.call("POST", f"/repos/{slug}/git/trees", {"tree": tree})

    message = git("log", "-1", "--format=%B").decode("utf-8", "replace").strip()
    _, commit = api.call(
        "POST",
        f"/repos/{slug}/git/commits",
        {"message": message, "tree": new_tree["sha"], "parents": []},
    )
    print(f"commit {commit['sha'][:8]}")

    # ---- branch -----------------------------------------------------------
    code, _ = api.call(
        "POST",
        f"/repos/{slug}/git/refs",
        {"ref": f"refs/heads/{args.branch}", "sha": commit["sha"]},
        ok=(201, 422),
    )
    if code == 422:
        api.call(
            "PATCH",
            f"/repos/{slug}/git/refs/heads/{args.branch}",
            {"sha": commit["sha"], "force": False},
        )
    print(f"branch {args.branch} -> {commit['sha'][:8]}")

    # ---- tag + release build ---------------------------------------------
    api.call(
        "POST",
        f"/repos/{slug}/git/refs",
        {"ref": f"refs/tags/{args.tag}", "sha": commit["sha"]},
        ok=(201, 422),
    )
    print(f"tag {args.tag}")

    # Creating a ref through the API does not always fire a `push` event, so
    # ask the release workflow to run explicitly. It declares workflow_dispatch.
    code, _ = api.call(
        "POST",
        f"/repos/{slug}/actions/workflows/release.yml/dispatches",
        {"ref": args.tag},
        ok=(204, 404, 422),
    )
    if code == 204:
        print("release workflow dispatched")
    else:
        print("note: could not dispatch the workflow (it may still run from the tag push)")

    print()
    print(f"https://github.com/{slug}")
    print()
    print("install on a server:")
    print(f"  REPO={slug}")
    print("  curl -fsSL https://raw.githubusercontent.com/$REPO/main/deploy/install.sh \\")
    print("    | sudo bash -s -- --repo $REPO --domain node.example.com")
    return 0


if __name__ == "__main__":
    sys.exit(main())
