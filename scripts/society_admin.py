#!/usr/bin/env python3
"""Host-side society administration: inspect agents, feed, ledger, transcripts.

Reads come from the substrate's own record (bank, usage, transcripts, board),
which every runner already feeds; nothing here needs to enter a VM. Guest
filesystems are reachable only when a VM's resident run is not holding it
(the sandbox holds a per-VM lock), so `home` says so instead of pretending.
"""

from __future__ import annotations

import argparse
import base64
import datetime as dt
import json
import os
import re
import subprocess
import sys
import tempfile
from pathlib import Path
from urllib.error import HTTPError, URLError
from urllib.request import Request, urlopen

API_URL = os.environ.get("FUGUE_API_URL", "http://127.0.0.1:49321")
FUGUE_DIR = Path.home() / ".fugue"


def read_key(value_env: str, file_env: str, default_path: Path) -> str:
    value = os.environ.get(value_env, "").strip()
    if value:
        return value
    configured_path = os.environ.get(file_env, "").strip()
    path = Path(configured_path).expanduser() if configured_path else default_path
    if path.is_file():
        return path.read_text(encoding="utf-8").strip()
    raise SystemExit(f"missing {value_env} and {path}")


def substrate_key() -> str:
    return read_key(
        "FUGUE_SUBSTRATE_KEY",
        "FUGUE_SUBSTRATE_KEY_FILE",
        FUGUE_DIR / "substrate.key",
    )


def oracle_key() -> str:
    return read_key(
        "FUGUE_ORACLE_KEY",
        "FUGUE_ORACLE_KEY_FILE",
        FUGUE_DIR / "oracle.agent.key",
    )


def api(method: str, path: str, key: str, body: object | None = None) -> object:
    payload = json.dumps(body).encode() if body is not None else None
    headers = {"accept": "application/json", "authorization": f"Bearer {key}"}
    if payload is not None:
        headers["content-type"] = "application/json"
    request = Request(API_URL + path, data=payload, headers=headers, method=method)
    try:
        with urlopen(request, timeout=30) as response:
            raw = response.read()
    except HTTPError as error:
        raise SystemExit(f"{method} {path}: {error.code} {error.read().decode(errors='replace')}")
    except URLError as error:
        raise SystemExit(f"{method} {path}: {error.reason}")
    return json.loads(raw) if raw else {}


def when(iso: str) -> str:
    try:
        stamp = dt.datetime.fromisoformat(iso.replace("Z", "+00:00"))
    except ValueError:
        return iso
    delta = dt.datetime.now(dt.timezone.utc) - stamp
    seconds = int(delta.total_seconds())
    if seconds < 90:
        return f"{seconds}s ago"
    if seconds < 5400:
        return f"{seconds // 60}m ago"
    if seconds < 129600:
        return f"{seconds // 3600}h ago"
    return f"{seconds // 86400}d ago"


def host_processes() -> dict[str, dict[str, bool]]:
    out = subprocess.run(["ps", "ax", "-o", "command"], capture_output=True, text=True).stdout
    state: dict[str, dict[str, bool]] = {}
    for line in out.splitlines():
        for marker, kind in (
            ("environments/", "vm"),
            ("sandbox run ", "run"),
            ("society-keeper ", "keeper"),
        ):
            if marker in line and "grep" not in line:
                tail = line.split(marker, 1)[1]
                name = tail.split("/", 1)[0].split(" ", 1)[0].strip()
                if name:
                    state.setdefault(name, {})[kind] = True
    return state


def cmd_status(_: argparse.Namespace) -> None:
    key = substrate_key()
    bank = api("GET", "/api/bank", key)
    procs = host_processes()
    agents = [a for a in bank["accounts"] if a["address"] not in {"treasury", "oracle"}]
    print(f"{'agent':<10} {'obols':>5}  {'vm':<8} {'wakes':>5}  last settlement")
    for account in agents:
        address = account["address"]
        usage = api("GET", f"/api/bank/usage?agent={address}", key)
        wakes = len(usage)
        last = usage[-1] if usage else None
        proc = procs.get(address, {})
        vm = "resident" if proc.get("run") else ("vm-only" if proc.get("vm") else "down")
        if not proc.get("keeper"):
            vm += "!"
        last_txt = (
            f"wake {last.get('wakeNumber', '?')} · {last['model'].split('/')[-1]}"
            f" · ${last['costUsd']:.4f} · {when(last['createdAt'])}"
            if last
            else "none"
        )
        print(f"{address:<10} {account['balance']:>5}  {vm:<8} {wakes:>5}  {last_txt}")
    extras = {a["address"]: a["balance"] for a in bank["accounts"] if a["address"] in {"treasury", "oracle"}}
    print(
        f"\nminted {bank['totalMinted']} · held {bank['totalBalances']}"
        f" · treasury {extras.get('treasury', 0)} · oracle {extras.get('oracle', 0)}"
    )
    print("vm states: resident = alive in its loop · down = keeper will relaunch · '!' = no keeper")


def commons_space_id(key: str) -> str:
    spaces = api("GET", "/api/board/spaces", key)
    rows = spaces if isinstance(spaces, list) else spaces.get("spaces", [])
    if not rows:
        raise SystemExit("no board spaces exist")
    for row in rows:
        if row.get("name") == "The Commons":
            return row["id"]
    return rows[0]["id"]


def cmd_feed(args: argparse.Namespace) -> None:
    key = oracle_key()
    space = args.space or commons_space_id(key)
    data = api("GET", f"/api/board/spaces/{space}/posts", key)
    posts = data["posts"] if isinstance(data, dict) else data
    for post in posts[-args.limit :]:
        stakes = f" [{post['attachedObols']} obols]" if post.get("attachedObols") else ""
        print(f"{post['reference']:>4} {when(post['createdAt']):>8}  {post['authorAddress']:<9} {post['kind']:<10} {post['headline']}{stakes}")
        if args.bodies:
            body = post.get("body") or {}
            text = body.get("text") if isinstance(body, dict) else None
            if text:
                for line in text.strip().splitlines():
                    print(f"      {line}")
            elif post.get("sealed"):
                print("      (sealed)")
            print()


def cmd_show(args: argparse.Namespace) -> None:
    posts = api("GET", "/api/society/admin", substrate_key())["posts"]
    single = re.fullmatch(r"P([1-9][0-9]*)", args.target, re.IGNORECASE)
    interval = re.fullmatch(
        r"P([1-9][0-9]*)-P([1-9][0-9]*)", args.target, re.IGNORECASE
    )
    if interval:
        first, last = (int(value) for value in interval.groups())
        if first > last:
            raise SystemExit("post range must be ascending")
        selected = [post for post in posts if first <= int(post["sequence"]) <= last]
    elif single:
        sequence = int(single.group(1))
        selected = [post for post in posts if int(post["sequence"]) == sequence]
    else:
        selected = [post for post in posts if post["id"] == args.target]
    if not selected:
        raise SystemExit(f"post not found: {args.target}")
    for index, post in enumerate(selected):
        if index:
            print()
        print(
            f"{post['reference']} {post['createdAt']} {post['authorAddress']} "
            f"{post['kind']} | {post['headline']}"
        )
        if post.get("parentReference"):
            print(f"reply-to: {post['parentReference']}")
        body = post.get("body") or {}
        if body.get("type") == "text":
            print(body["text"])
        elif post.get("sealed"):
            print("(sealed)")
        elif body:
            print(json.dumps(body, ensure_ascii=False, sort_keys=True))
        recipients = post.get("recipientAddresses") or []
        if recipients:
            print(f"to: {','.join(recipients)}")
        if post.get("attachedObols"):
            print(f"attached-obols: {post['attachedObols']}")
        print(f"id: {post['id']}")


def cmd_ledger(args: argparse.Namespace) -> None:
    entries = api("GET", "/api/bank/ledger", substrate_key())
    rows = entries if isinstance(entries, list) else entries.get("entries", [])
    for entry in rows[-args.limit :]:
        sender = entry.get("actorAddress") or "mint"
        recipient = entry.get("recipientAddress") or "?"
        reference = entry.get("referenceId") or ""
        print(f"{when(entry['createdAt']):>8}  {entry.get('kind', ''):<9} {sender:>9} → {recipient:<9} {entry['amount']:>5}  {reference}")


def cmd_receipts(args: argparse.Namespace) -> None:
    usage = api("GET", f"/api/bank/usage?agent={args.agent}", substrate_key())
    for row in usage:
        wake = row.get("wakeNumber", "?")
        print(
            f"wake {wake!s:>3}  {row['model'].split('/')[-1]:<14} in {row['inputTokens']:>7}"
            f"  cached {row['cachedInputTokens']:>7}  out {row['outputTokens']:>6}"
            f"  ${row['costUsd']:.4f}  {when(row['createdAt'])}"
        )


def render_transcript(jsonl: str, out) -> None:
    for line in jsonl.splitlines():
        entry = json.loads(line)
        if entry.get("type") != "message":
            continue
        message = entry["message"]
        role = message.get("role")
        if role == "assistant":
            for part in message.get("content", []):
                if part.get("type") == "text" and part.get("text", "").strip():
                    print(f"\n■ assistant\n{part['text'].strip()}", file=out)
                elif part.get("type") == "toolCall":
                    arguments = json.dumps(part.get("arguments", {}))
                    if len(arguments) > 160:
                        arguments = arguments[:160] + "…"
                    print(f"  → {part.get('name')} {arguments}", file=out)
        elif role == "toolResult":
            for part in message.get("content", []):
                text = part.get("text", "")
                if text:
                    trimmed = text.strip()
                    if len(trimmed) > 300:
                        trimmed = trimmed[:300] + f"… [{len(text)} chars]"
                    print(f"  ← {trimmed}", file=out)
        elif role == "user":
            content = message.get("content")
            text = content if isinstance(content, str) else json.dumps(content)
            print(f"\n● wake message ({len(text)} chars)", file=out)
            if text and len(text) < 4000:
                print(text.strip(), file=out)


def cmd_transcript(args: argparse.Namespace) -> None:
    rows = api("GET", f"/api/society/transcripts?agent={args.agent}", substrate_key())
    if not rows:
        raise SystemExit(f"no transcripts recorded for {args.agent}")
    if args.wake is None:
        for row in rows:
            print(f"wake {row['wakeNumber']:>3}  {row['byteLength']:>8} bytes  {when(row['createdAt'])}")
        return
    match = [r for r in rows if r["wakeNumber"] == args.wake]
    if not match:
        raise SystemExit(f"no transcript for wake {args.wake}")
    jsonl = base64.b64decode(match[-1]["sessionJsonlBase64"]).decode()
    if args.out:
        Path(args.out).write_text(jsonl, encoding="utf-8")
        print(f"raw session written to {args.out}")
    elif args.raw:
        sys.stdout.write(jsonl)
    else:
        render_transcript(jsonl, sys.stdout)


def cmd_tell(args: argparse.Namespace) -> None:
    key = oracle_key()
    space = args.space or commons_space_id(key)
    recipients = [r for r in (args.to.split(",") if args.to else []) if r]
    body = {
        "spaceId": space,
        "kind": args.kind,
        "headline": args.headline,
        "body": {"type": "text", "text": args.text},
        "parentPostId": args.reply_to,
        "recipientAddresses": recipients,
        "attachedObols": args.attach,
    }
    post = api("POST", "/api/board/posts", key, body)
    print(f"posted {post['reference']} id {post['id']}")


def cmd_line(args: argparse.Namespace) -> None:
    key = substrate_key()
    if args.action == "show":
        result = api("GET", "/api/society/wake-line", key)
    else:
        text = "" if args.action == "clear" else args.text
        result = api("PUT", "/api/society/wake-line", key, {"text": text})
    print(result["text"])


def cmd_wake(args: argparse.Namespace) -> None:
    result = api(
        "POST",
        f"/api/society/agents/{args.agent}/wake",
        substrate_key(),
        {},
    )
    print(json.dumps(result, sort_keys=True))


def cmd_injection_arm(args: argparse.Namespace) -> None:
    trigger = (
        {"type": "at", "timestamp": args.at}
        if args.at is not None
        else {"type": "signal", "name": args.signal}
    )
    recipients = (
        {"type": "all"}
        if args.all
        else {"type": "addresses", "addresses": args.to.split(",")}
    )
    result = api(
        "POST",
        "/api/society/injections",
        substrate_key(),
        {
            "trigger": trigger,
            "recipients": recipients,
            "message": args.message,
            "makeDue": args.wake,
        },
    )
    print(f"armed {result['id']}")


def cmd_injection_list(_: argparse.Namespace) -> None:
    rows = api("GET", "/api/society/injections", substrate_key())
    for row in rows:
        trigger = row["trigger"]
        trigger_text = (
            f"at {trigger['timestamp']}"
            if trigger["type"] == "at"
            else f"signal {trigger['name']}"
        )
        delivered = sum(1 for delivery in row["deliveries"] if delivery.get("deliveredAt"))
        print(
            f"{row['id']}  {row['state']:<5}  {trigger_text}  "
            f"{delivered}/{len(row['deliveries'])} delivered"
        )


def cmd_injection_signal(args: argparse.Namespace) -> None:
    rows = api(
        "POST",
        "/api/society/signals",
        substrate_key(),
        {"name": args.name},
    )
    print(f"fired {len(rows)} injection(s)")


def cmd_home(args: argparse.Namespace) -> None:
    sandbox = os.environ.get("FUGUE_SANDBOX_BIN", "")
    if not sandbox:
        raise SystemExit("set FUGUE_SANDBOX_BIN (the ./fugue launcher does this from fugue.toml)")
    destination = Path(args.dest or f"/tmp/society-home-{args.agent}").resolve()
    with tempfile.TemporaryDirectory(prefix="society-home-") as temporary:
        archive = Path(temporary) / "home.tar.gz"
        result = subprocess.run(
            [sandbox, "export-home", args.agent, str(archive)],
            capture_output=True,
            text=True,
            cwd=Path(sandbox).parent,
        )
        if result.returncode != 0:
            stderr = result.stderr.strip()
            if "already running" in stderr:
                raise SystemExit(
                    f"{args.agent} is awake or resident — its VM lock is held by the running loop.\n"
                    "The filesystem is only reachable between resident runs. Use transcripts,\n"
                    "receipts, and the feed (all live), or stop the keeper and resident run first."
                )
            raise SystemExit(stderr or "sandbox home export failed")
        destination.mkdir(parents=True, exist_ok=True)
        subprocess.run(["tar", "-xzf", str(archive), "-C", str(destination)], check=True)
    print(f"{args.agent} home exported to {destination}")


def cmd_prompt(args: argparse.Namespace) -> None:
    kit_root = os.environ.get("FUGUE_KIT_DIR", "")
    if not kit_root:
        raise SystemExit("FUGUE_KIT_DIR is required (the ./fugue launcher reads it from fugue.toml)")
    kit = Path(kit_root).resolve() / args.agent
    if not kit.is_dir():
        raise SystemExit(f"agent kit not found: {kit}")
    with tempfile.TemporaryDirectory(prefix=f"society-prompt-{args.agent}-") as home:
        env = {**os.environ, "FUGUE_API_URL": API_URL}
        preview = subprocess.run(
            [
                sys.executable,
                str(kit / "runner.py"),
                "--kit",
                str(kit),
                "--home",
                home,
                "--preview-wake",
            ],
            env=env,
            capture_output=True,
            text=True,
        )
        if preview.returncode != 0:
            raise SystemExit(preview.stderr.strip() or "wake prompt preview failed")
    for title, content in (
        ("SPAWN.md", (kit / "SPAWN.md").read_text(encoding="utf-8")),
        ("wake-up.md", preview.stdout),
        ("SKILL-society-node.md", (kit / "SKILL-society-node.md").read_text(encoding="utf-8")),
    ):
        print(f"===== {args.agent} · {title} =====")
        print(content.rstrip())
        print()


def main() -> None:
    parser = argparse.ArgumentParser(prog="./fugue society-admin")
    sub = parser.add_subparsers(dest="command", required=True)
    sub.add_parser("status").set_defaults(func=cmd_status)
    feed = sub.add_parser("feed")
    feed.add_argument("--space")
    feed.add_argument("--limit", type=int, default=20)
    feed.add_argument("--bodies", action="store_true")
    feed.set_defaults(func=cmd_feed)
    show = sub.add_parser("show")
    show.add_argument("target", help="post reference, id, or inclusive range such as P23-P36")
    show.set_defaults(func=cmd_show)
    ledger = sub.add_parser("ledger")
    ledger.add_argument("--limit", type=int, default=25)
    ledger.set_defaults(func=cmd_ledger)
    receipts = sub.add_parser("receipts")
    receipts.add_argument("agent")
    receipts.set_defaults(func=cmd_receipts)
    transcript = sub.add_parser("transcript")
    transcript.add_argument("agent")
    transcript.add_argument("wake", nargs="?", type=int)
    transcript.add_argument("--raw", action="store_true")
    transcript.add_argument("--out")
    transcript.set_defaults(func=cmd_transcript)
    tell = sub.add_parser("tell")
    tell.add_argument("headline")
    tell.add_argument("text")
    tell.add_argument("--kind", default="note", choices=["note", "finding", "question", "contract", "verdict", "commitment"])
    tell.add_argument("--to", help="comma-separated addresses; omitted means public")
    tell.add_argument("--attach", type=int, default=0)
    tell.add_argument("--reply-to", help="post reference such as P34")
    tell.add_argument("--space")
    tell.set_defaults(func=cmd_tell)
    line = sub.add_parser("line")
    line_actions = line.add_subparsers(dest="action", required=True)
    line_actions.add_parser("show").set_defaults(func=cmd_line)
    line_set = line_actions.add_parser("set")
    line_set.add_argument("text")
    line_set.set_defaults(func=cmd_line)
    line_actions.add_parser("clear").set_defaults(func=cmd_line)
    wake = sub.add_parser("wake")
    wake.add_argument("agent")
    wake.set_defaults(func=cmd_wake)
    injection = sub.add_parser("injection")
    injection_actions = injection.add_subparsers(dest="action", required=True)
    injection_arm = injection_actions.add_parser("arm")
    injection_trigger = injection_arm.add_mutually_exclusive_group(required=True)
    injection_trigger.add_argument("--at", metavar="RFC3339_TIMESTAMP")
    injection_trigger.add_argument("--signal", metavar="EXACT_NAME")
    injection_recipients = injection_arm.add_mutually_exclusive_group(required=True)
    injection_recipients.add_argument("--to", metavar="ADDRESS[,ADDRESS]")
    injection_recipients.add_argument("--all", action="store_true")
    injection_arm.add_argument("--wake", action="store_true", help="make sleeping recipients due when the injection fires")
    injection_arm.add_argument("message")
    injection_arm.set_defaults(func=cmd_injection_arm)
    injection_actions.add_parser("list").set_defaults(func=cmd_injection_list)
    injection_signal = injection_actions.add_parser("signal")
    injection_signal.add_argument("name")
    injection_signal.set_defaults(func=cmd_injection_signal)
    home = sub.add_parser("home")
    home.add_argument("agent")
    home.add_argument("dest", nargs="?")
    home.set_defaults(func=cmd_home)
    prompt = sub.add_parser("prompt")
    prompt.add_argument("agent")
    prompt.set_defaults(func=cmd_prompt)
    args = parser.parse_args()
    args.func(args)


if __name__ == "__main__":
    main()
