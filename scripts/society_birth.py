#!/usr/bin/env python3
"""Provision one society agent and emit its self-contained Robinson kit."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import secrets
import shutil
import sys
import tomllib
from pathlib import Path
from urllib.error import HTTPError, URLError
from urllib.request import Request, urlopen

if __package__:
    from .worlds import definition, refresh_kit
else:
    from worlds import definition, refresh_kit


def compact(value: object) -> str:
    return json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False)


def address_for(name: str) -> str:
    address = re.sub(r"[^a-z0-9]+", "-", name.strip().lower()).strip("-")
    if not 2 <= len(address) <= 40:
        raise ValueError("name must produce an address of 2-40 lowercase letters, numbers, or hyphens")
    return address


def wake_stagger_seconds(address: str, default_sleep_seconds: float) -> float:
    interval = int(default_sleep_seconds)
    if interval < 1:
        return 0.0
    digest = hashlib.sha256(address.encode("utf-8")).digest()
    return float(int.from_bytes(digest[:8], "big") % interval)


def post(base_url: str, key: str, path: str, body: object) -> dict[str, object]:
    request = Request(
        base_url.rstrip("/") + path,
        data=compact(body).encode("utf-8"),
        headers={"content-type": "application/json", "authorization": f"Bearer {key}"},
        method="POST",
    )
    try:
        with urlopen(request, timeout=30) as response:
            return json.loads(response.read())
    except HTTPError as error:
        detail = error.read().decode("utf-8", errors="replace")
        raise RuntimeError(f"birth API failed: {error.code} {detail}") from error
    except URLError as error:
        raise RuntimeError(f"birth API failed: {error.reason}") from error


def sync_directory(path: Path) -> None:
    descriptor = os.open(path, os.O_RDONLY)
    try:
        os.fsync(descriptor)
    finally:
        os.close(descriptor)


def write_durable(path: Path, content: str, mode: int | None = None) -> None:
    temporary = path.parent / f".{path.name}.{os.getpid()}.{secrets.token_hex(8)}.tmp"
    try:
        with temporary.open("x", encoding="utf-8") as target:
            if mode is not None:
                os.fchmod(target.fileno(), mode)
            target.write(content)
            target.flush()
            os.fsync(target.fileno())
        os.link(temporary, path, follow_symlinks=False)
        sync_directory(path.parent)
    finally:
        if os.path.lexists(temporary):
            temporary.unlink()
            sync_directory(path.parent)


def finalize_pending(pending: Path, destination: Path, kit_root: Path) -> None:
    pending.rename(destination)
    sync_directory(kit_root)


def main() -> int:
    parser = argparse.ArgumentParser(prog="./fugue society-birth")
    parser.add_argument("name")
    parser.add_argument("--kit-dir", type=Path)
    parser.add_argument("--role", help="unique role added to this agent's spawn context")
    args = parser.parse_args()

    root = Path(__file__).resolve().parent.parent
    config_path = Path(os.environ.get("FUGUE_CONFIG", root / "fugue.toml")).resolve()
    with config_path.open("rb") as handle:
        config = tomllib.load(handle)
    society = config.get("society", {})
    definition(root, society)  # Reject unknown worlds before creating identity or authority.
    runner = society.get("runner", {})
    models = society.get("models", {})
    if not models or runner.get("default_model", "luna") not in models:
        raise RuntimeError("society.models must include the configured default model")
    address = address_for(args.name)
    configured_kit_dir = Path(society.get("kit_dir", "data/kits"))
    if not configured_kit_dir.is_absolute():
        configured_kit_dir = config_path.parent / configured_kit_dir
    kit_root = (args.kit_dir or configured_kit_dir).resolve()
    destination = kit_root / address
    if destination.exists():
        raise RuntimeError(f"kit already exists: {destination}")
    pending = kit_root / f".{address}.pending"

    substrate_key = os.environ.get("FUGUE_SUBSTRATE_KEY", "").strip()
    if not substrate_key:
        raise RuntimeError("FUGUE_SUBSTRATE_KEY is required")
    host = os.environ.get("FUGUE_API_HOST", str(config["api"]["host"]))
    if host == "0.0.0.0":
        host = "127.0.0.1"
    port = os.environ.get("FUGUE_API_PORT", str(config["api"]["port"]))
    host_api_url = os.environ.get("FUGUE_API_URL", f"http://{host}:{port}")

    kit_root.mkdir(parents=True, exist_ok=True)
    identity = {"address": address, "name": args.name.strip()}
    identity_path = pending / ".birth.json"
    key_path = pending / "agent.key"

    def create_pending() -> str:
        pending.mkdir(mode=0o700)
        sync_directory(kit_root)
        write_durable(identity_path, compact(identity) + "\n")
        key = secrets.token_hex(32)
        write_durable(key_path, key + "\n", 0o600)
        return key

    if os.path.lexists(pending):
        if pending.is_symlink() or not pending.is_dir():
            raise RuntimeError(f"pending kit must be a real directory: {pending}")
        if not os.path.lexists(key_path):
            shutil.rmtree(pending)
            agent_key = create_pending()
        else:
            try:
                pending_identity = json.loads(identity_path.read_text(encoding="utf-8"))
                agent_key = key_path.read_text(encoding="utf-8").strip()
            except (OSError, UnicodeError, json.JSONDecodeError) as error:
                raise RuntimeError(f"cannot resume pending kit: {pending}") from error
            if pending_identity != identity or not agent_key:
                raise RuntimeError(f"pending kit identity does not match this birth: {pending}")
    else:
        agent_key = create_pending()

    credential: dict[str, object] | None = None
    try:
        runner_config = {
            "executorBinary": runner.get("executor_binary", "pi"),
            "defaultModel": runner.get("default_model", "luna"),
            "defaultEffort": runner.get("default_effort", "high"),
            "defaultSleepSeconds": runner.get("default_sleep_seconds", 300),
            "maxSleepSeconds": runner.get("max_sleep_seconds", 300),
            "activeHoursTimezone": runner.get("active_hours_timezone", "Europe/Vienna"),
            "activeHoursStart": runner.get("active_hours_start", 8),
            "activeHoursEnd": runner.get("active_hours_end", 22),
            "wakeStaggerSeconds": wake_stagger_seconds(
                address, float(runner.get("default_sleep_seconds", 300))
            ),
            "wallTimeoutSeconds": runner.get("wall_timeout_seconds", 86400),
            "hardStopAt": runner.get("hard_stop_at"),
            "pollSeconds": runner.get("poll_seconds", 0.2),
            "quotaThresholdPercent": runner.get("quota_threshold_percent", 85),
            "quotaPauseSeconds": runner.get("quota_pause_seconds", 600),
            "quotaProbe": runner.get("quota_probe", True),
        }
        node = {
            "apiBaseUrl": society.get("guest_api_url", "http://192.168.64.1:49321"),
            "address": address,
            "runner": runner_config,
        }
        (pending / "node.json").write_text(compact(node) + "\n", encoding="utf-8")
        rates = {"obolUsd": 0.01, "models": models}
        (pending / "rates.json").write_text(compact(rates) + "\n", encoding="utf-8")
        refresh_kit(root, pending, society, name=args.name.strip(), role=args.role or "", force=True)

        credential = post(host_api_url, substrate_key, "/api/society/birth", {
            "address": address,
            "name": args.name.strip(),
            "key": agent_key,
        })
        key = credential.get("key")
        if key != agent_key:
            raise RuntimeError("birth API did not return the pending agent key")
        finalize_pending(pending, destination, kit_root)
    except BaseException:
        print(f"society-birth: recovery kit retained at {pending}", file=sys.stderr)
        raise

    print(f"address={address}")
    print(f"actor_id={credential.get('actorId')}")
    print(f"kit={destination}")
    print(f"weekly_allowance={society.get('weekly_allowance', 100)}")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except Exception as error:
        print(f"society-birth: {error}", file=sys.stderr)
        raise SystemExit(1)
