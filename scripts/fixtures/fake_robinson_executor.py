#!/usr/bin/env python3
"""Deterministic pi substitute for the Robinson end-to-end scenario."""

import json
import os
import signal
import subprocess
import sys
import time
from pathlib import Path


def line(value: object) -> str:
    return json.dumps(value, separators=(",", ":")) + "\n"


def main() -> int:
    args = sys.argv[1:]
    try:
        session = Path(args[args.index("--session") + 1])
    except (ValueError, IndexError):
        print("fake executor requires --session PATH", file=sys.stderr)
        return 2
    wake = int(session.stem.split("-")[-1])
    session.parent.mkdir(parents=True, exist_ok=True)
    session.write_text(line({"type": "session", "version": 3, "id": f"fake-{wake}", "timestamp": "2026-08-27T00:00:00.000Z", "cwd": os.getcwd(), "computeBacking": os.environ.get("ROBINSON_COMPUTE_BACKING")}), encoding="utf-8")
    if wake == 1:
        action = subprocess.run(
            ["society-node", "space", "create", "--slug", "runner-proof", "--name", "Runner Proof"],
            capture_output=True,
            text=True,
            check=True,
        )
        with session.open("a", encoding="utf-8") as handle:
            handle.write(line({"type": "message", "id": "user-1", "message": {"role": "user", "content": [{"type": "text", "text": "wake"}]}}))
            handle.write(line({"type": "message", "id": "assistant-1", "message": {
                "role": "assistant", "provider": "openai-codex", "model": "fake-luna",
                "content": [{"type": "text", "text": action.stdout.strip()}],
                "usage": {"input": 1001, "output": 0, "cacheRead": 0, "cacheWrite": 0, "reasoning": 0,
                          "totalTokens": 1001, "cost": {"total": 9999.0}}
            }}))
        write_sleep("0.02s", "luna", "high", 5)
        return 0
    if wake == 2:
        with session.open("a", encoding="utf-8") as handle:
            handle.write(line({"type": "message", "id": "user-2", "message": {"role": "user", "content": [{"type": "text", "text": "no usage"}]}}))
        write_sleep("6h", "luna", "high", 1)
        return 0
    if wake == 3:
        subprocess.run(
            ["society-node", "transfer", "treasury", "1"],
            capture_output=True,
            text=True,
            check=True,
        )
        killed = Path(os.environ["FAKE_EXECUTOR_KILLED_FILE"])
        def stop(_signum, _frame):
            killed.write_text("killed\n", encoding="utf-8")
            raise SystemExit(143)
        signal.signal(signal.SIGTERM, stop)
        with session.open("a", encoding="utf-8") as handle:
            handle.write(line({"type": "message", "id": "assistant-3", "message": {
                "role": "assistant", "provider": "openai-codex", "model": "fake-luna",
                "content": [{"type": "toolCall", "id": "tool-1", "name": "bash", "arguments": {"command": "sleep"}}],
                "usage": {"input": 1001, "output": 0, "cacheRead": 0, "cacheWrite": 0, "reasoning": 0,
                          "totalTokens": 1001, "cost": {"total": 9999.0}}, "stopReason": "toolUse"
            }}))
            handle.flush()
        while True:
            time.sleep(1)
    return 2


def write_sleep(duration: str, model: str, effort: str, cap: int) -> None:
    path = Path(os.environ["ROBINSON_SLEEP_FILE"])
    path.write_text(json.dumps({"duration": duration, "model": model, "effort": effort, "capObols": cap}) + "\n", encoding="utf-8")


if __name__ == "__main__":
    raise SystemExit(main())
