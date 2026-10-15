#!/usr/bin/env python3
"""Crash-only guest-resident Robinson wake loop (Python standard library only)."""

from __future__ import annotations

import argparse
import base64
import datetime as dt
import json
import math
import os
import re
import signal
import subprocess
import sys
import time
from decimal import Decimal, ROUND_CEILING
from pathlib import Path
from typing import Any
from urllib.error import HTTPError, URLError
from urllib.request import HTTPRedirectHandler, Request, build_opener, urlopen
from zoneinfo import ZoneInfo, ZoneInfoNotFoundError

DURATION_RE = re.compile(r"^(\d+(?:\.\d+)?)([smhd])$")
EFFORTS = {"off", "minimal", "low", "medium", "high", "xhigh", "max"}


class DeadlineReached(RuntimeError):
    pass


def hard_stop_epoch(value: str | None) -> float | None:
    if value is None:
        return None
    deadline = dt.datetime.fromisoformat(value.replace("Z", "+00:00"))
    if deadline.tzinfo is None:
        raise ValueError("hard_stop_at must include a timezone")
    return deadline.timestamp()


class ApiRequestError(RuntimeError):
    def __init__(self, method: str, path: str, status: int, detail: str) -> None:
        super().__init__(f"{method} {path} failed: {status} {detail}")
        self.status = status


def compact(value: Any) -> str:
    return json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False)


def atomic_json(path: Path, value: Any) -> None:
    temporary = path.with_suffix(path.suffix + ".tmp")
    temporary.write_text(compact(value) + "\n", encoding="utf-8")
    temporary.replace(path)


def read_json(path: Path, default: Any = None) -> Any:
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except FileNotFoundError:
        return default


def parse_duration(value: str) -> float:
    match = DURATION_RE.fullmatch(value.strip())
    if not match:
        raise ValueError("duration must be a number followed by s, m, h, or d")
    multiplier = {"s": 1, "m": 60, "h": 3600, "d": 86400}[match.group(2)]
    return float(match.group(1)) * multiplier


def format_hours(seconds: float) -> str:
    return f"{seconds / 3600:.2f}".rstrip("0").rstrip(".") + "h"


def scheduled_wake_at(ended_at: float, sleep_seconds: float, config: dict[str, Any]) -> float:
    candidate = dt.datetime.fromtimestamp(ended_at + sleep_seconds, dt.timezone.utc)
    timezone_name = required_string(config, "activeHoursTimezone")
    try:
        timezone = ZoneInfo(timezone_name)
    except ZoneInfoNotFoundError as error:
        raise RuntimeError(f"unknown active-hours timezone {timezone_name!r}") from error
    start_hour = int(config["activeHoursStart"])
    end_hour = int(config["activeHoursEnd"])
    stagger = float(config.get("wakeStaggerSeconds", 0.0))
    if not 0 <= start_hour < end_hour <= 24 or stagger < 0:
        raise RuntimeError("active hours and wake stagger must be valid")
    local = candidate.astimezone(timezone)
    day_start = dt.datetime.combine(local.date(), dt.time(), timezone)
    window_start = day_start + dt.timedelta(hours=start_hour, seconds=stagger)
    window_end = day_start + dt.timedelta(hours=end_hour)
    if local < window_start:
        return window_start.timestamp()
    if local >= window_end:
        return (day_start + dt.timedelta(days=1, hours=start_hour, seconds=stagger)).timestamp()
    return candidate.timestamp()


def utc_timestamp(epoch: float) -> str:
    return dt.datetime.fromtimestamp(epoch, dt.timezone.utc).isoformat().replace("+00:00", "Z")


class Runner:
    def __init__(self, kit: Path, home: Path, max_wakes: int | None) -> None:
        self.kit = kit.resolve()
        self.home = home.resolve()
        self.max_wakes = max_wakes
        self.node = read_json(self.kit / "node.json")
        self.rates = read_json(self.kit / "rates.json")
        if not isinstance(self.node, dict) or not isinstance(self.rates, dict):
            raise RuntimeError("node.json and rates.json must contain JSON objects")
        self.address = required_string(self.node, "address")
        self.base_url = os.environ.get(
            "FUGUE_API_URL", required_string(self.node, "apiBaseUrl")
        ).rstrip("/")
        self.runner_config = self.node.get("runner")
        if not isinstance(self.runner_config, dict):
            raise RuntimeError("node.json must contain runner configuration")
        deadlines = [hard_stop_epoch(value) for value in (
            self.runner_config.get("hardStopAt"), os.environ.get("ROBINSON_HARD_STOP_AT")
        ) if value is not None]
        self.hard_stop_at = min(deadlines) if deadlines else None
        self.models = self.rates.get("models")
        if not isinstance(self.models, dict):
            raise RuntimeError("rates.json must contain models")
        self.key_file = self.kit / "agent.key"
        self.key = self.key_file.read_text(encoding="utf-8").strip()
        if not self.key:
            raise RuntimeError("agent.key is empty")
        self.state_path = self.home / ".robinson-state.json"
        self.in_progress_path = self.home / ".robinson-in-progress.json"
        self.live_spend_path = self.home / ".robinson-live-spend.json"
        self.sleep_path = self.home / ".robinson-sleep.json"
        self.note_path = self.home / "note-to-self.md"
        self.wake_message_path = self.home / "wake-up.md"
        self.sessions_dir = self.home / "sessions"
        self.log_path = self.home / "robinson.log"
        self.quota_window_path = self.home / ".robinson-quota-window.json"
        self._prepare_home()

    def _prepare_home(self) -> None:
        self.home.mkdir(parents=True, exist_ok=True)
        self.sessions_dir.mkdir(parents=True, exist_ok=True)
        project_settings = self.home / ".pi" / "settings.json"
        if not project_settings.exists():
            project_settings.parent.mkdir(parents=True, exist_ok=True)
            project_settings.write_text(compact({
                "compaction": {
                    "enabled": True,
                    "reserveTokens": 16384,
                    "keepRecentTokens": 20000,
                }
            }) + "\n", encoding="utf-8")
        fugue_dir = self.home / ".fugue"
        fugue_dir.mkdir(exist_ok=True)
        (fugue_dir / "agent.key").write_text(self.key + "\n", encoding="utf-8")
        os.chmod(fugue_dir / "agent.key", 0o600)
        (fugue_dir / "node.json").write_text(compact({"apiBaseUrl": self.base_url}) + "\n", encoding="utf-8")
        bin_dir = self.home / ".local" / "bin"
        bin_dir.mkdir(parents=True, exist_ok=True)
        wrapper = bin_dir / "society-node"
        wrapper.write_text(
            "#!/bin/sh\nexec python3 " + shell_quote(str(self.kit / "society_node.py")) + " \"$@\"\n",
            encoding="utf-8",
        )
        os.chmod(wrapper, 0o755)
        if not (self.home / ".git").exists():
            self._run_quiet(["git", "init", "-q"], check=True)
            self._run_quiet(["git", "config", "user.name", "Robinson Runner"], check=True)
            self._run_quiet(["git", "config", "user.email", "robinson@localhost"], check=True)

        # Keep mutable executor state and reproducible toolchains out of each
        # wake's Git history. Snapshot exports still preserve sessions and state.
        exclude = self.home / ".git/info/exclude"
        if exclude.parent.is_dir():
            patterns = [
                "/sessions/", "/.pi/agent/", "/.bun/", "/.cache/", "/.npm/",
                "/.rustup/", "/.cargo/registry/", "/.cargo/git/", "/.local/bin/uv",
                "/.local/bin/uvx", "/.fugue/agent.key", "/society/agent.key",
                "/.robinson-*.json", "/robinson.log", "/wake-up.md",
                "__pycache__/",
            ]
            current = exclude.read_text(encoding="utf-8") if exclude.exists() else ""
            missing = [pattern for pattern in patterns if pattern not in current.splitlines()]
            if missing:
                with exclude.open("a", encoding="utf-8") as target:
                    target.write("\n" + "\n".join(missing) + "\n")

    def check_deadline(self) -> None:
        if self.hard_stop_at is not None and time.time() >= self.hard_stop_at:
            raise DeadlineReached("experiment hard stop reached")

    def env(self, compute_backing: str | None = None) -> dict[str, str]:
        env = dict(os.environ)
        env["HOME"] = str(self.home)
        env["FUGUE_AGENT_KEY_FILE"] = str(self.key_file)
        env["FUGUE_API_URL"] = self.base_url
        env["PATH"] = str(self.home / ".local" / "bin") + os.pathsep + env.get("PATH", "")
        env["ROBINSON_SLEEP_FILE"] = str(self.sleep_path)
        env["ROBINSON_MAX_SLEEP_SECONDS"] = f"{self.max_sleep_seconds():g}"
        env["ROBINSON_MODELS"] = json.dumps(list(self.models))
        if compute_backing is not None:
            env["ROBINSON_COMPUTE_BACKING"] = compute_backing
        return env

    def max_sleep_seconds(self) -> float:
        value = float(self.runner_config.get("maxSleepSeconds", 300))
        if not math.isfinite(value) or value < 0:
            raise RuntimeError("maxSleepSeconds must be nonnegative")
        return value

    def api(self, method: str, path: str, body: Any | None = None) -> Any:
        payload = None if body is None else compact(body).encode("utf-8")
        headers = {"accept": "application/json", "authorization": f"Bearer {self.key}"}
        if payload is not None:
            headers["content-type"] = "application/json"
        request = Request(self.base_url + path, data=payload, headers=headers, method=method)
        try:
            with urlopen(request, timeout=30) as response:
                raw = response.read()
        except HTTPError as error:
            detail = error.read().decode("utf-8", errors="replace")
            raise ApiRequestError(method, path, error.code, detail) from error
        except URLError as error:
            raise RuntimeError(f"{method} {path} failed: {error.reason}") from error
        return json.loads(raw) if raw else {}

    def run(self) -> int:
        completed_here = 0
        while self.max_wakes is None or completed_here < self.max_wakes:
            self.check_deadline()
            state = read_json(self.state_path, {})
            if not isinstance(state, dict):
                state = {}
            progress = read_json(self.in_progress_path)
            if isinstance(progress, dict) and isinstance(progress.get("nextState"), dict):
                completed_state = progress["nextState"]
                self.report_sleeping(completed_state)
                atomic_json(self.state_path, completed_state)
                self.in_progress_path.unlink(missing_ok=True)
                self.sleep_path.unlink(missing_ok=True)
                state = completed_state
                progress = None
            recovered = isinstance(progress, dict)
            if not recovered:
                self.report_sleeping(state)
                self.wait_for_schedule(state)
            context = self.api("GET", "/api/society/wake-context")
            compute_backing = required_compute_backing(context)
            balance = int(context["budget"]["balance"])
            if recovered:
                wake_number = int(progress["wakeNumber"])
                plan = self.validate_plan(progress["plan"])
                session_path = Path(progress["session"])
                termination = str(progress.get("termination", "host_outage"))
                if "settlement" not in progress:
                    termination = "host_outage"
            else:
                if context.get("episode", {}).get("ended"):
                    self.log("halted: worker has ended")
                    return 0
                if compute_backing == "obols" and balance < 1:
                    self.log("halted: balance cannot cover a minimum wake")
                    return 0
                if not self.wait_for_quota():
                    return 75
                wake_number = int(state.get("lastWakeNumber", 0)) + 1
                plan = self.plan_for_wake(state)
                session_path = self.session_path(state)
                progress = {
                    "wakeNumber": wake_number,
                    "plan": plan,
                    "session": str(session_path),
                    "sessionStartOffset": session_path.stat().st_size if session_path.exists() else 0,
                    "startedAt": time.time(),
                    "stage": "prepared",
                }
                atomic_json(self.in_progress_path, progress)
            stage = str(progress.get("stage", "executing"))
            if stage in {"prepared", "assigned"}:
                permit, wake_start = self.start_wake(progress, wake_number)
                assigned_messages = wake_start["injectedMessages"]
                progress["assignedMessages"] = assigned_messages
                progress["stage"] = "assigned"
                atomic_json(self.in_progress_path, progress)
                context["injectedMessages"] = assigned_messages
                self.compose_wake_message(context, state, wake_number, plan)
                crash_marker = os.environ.get("ROBINSON_TEST_CRASH_AFTER_ASSIGNMENT_FILE")
                if crash_marker and not Path(crash_marker).exists():
                    Path(crash_marker).write_text("crashed\n", encoding="utf-8")
                    os._exit(85)
                try:
                    progress["stage"] = "executing"
                    progress.pop("permit", None)
                    atomic_json(self.in_progress_path, progress)
                    termination = self.execute_wake(
                        session_path,
                        int(progress.get("sessionStartOffset", 0)),
                        plan,
                        balance,
                        compute_backing,
                        wake_number,
                        permit,
                    )
                finally:
                    self.release_wake_permit(wake_number, permit["token"])
                progress["termination"] = termination
                progress["sessionEndOffset"] = session_path.stat().st_size if session_path.exists() else 0
                atomic_json(self.in_progress_path, progress)
            elif recovered:
                permit = progress.get("permit")
                if isinstance(permit, dict) and isinstance(permit.get("token"), str):
                    self.release_wake_permit(wake_number, permit["token"])
            settlement = progress.get("settlement")
            if not isinstance(settlement, dict):
                usage = meter_session(
                    session_path,
                    int(progress.get("sessionStartOffset", 0)),
                    int(progress.get("sessionEndOffset", session_path.stat().st_size if session_path.exists() else 0)),
                )
                cost_usd = price_usage(usage, self.model_rate(plan["model"]))
                priced_debit = 0 if not has_usage(usage) else max(
                    1, int((cost_usd * 100).to_integral_value(rounding=ROUND_CEILING))
                )
                projected_charge = (
                    min(priced_debit, balance) if compute_backing == "obols" else 0
                )
                settlement = {
                    "usage": usage,
                    "costUsd": str(cost_usd),
                    "pricedDebit": priced_debit,
                    "chargedDebit": projected_charge,
                }
                progress["settlement"] = settlement
                atomic_json(self.in_progress_path, progress)
            usage = settlement["usage"]
            cost_usd = Decimal(settlement["costUsd"])
            priced_debit = int(settlement["pricedDebit"])
            priced_debit, charged_debit = self.settle_wake(
                wake_number,
                plan,
                usage,
                cost_usd,
                priced_debit,
                compute_backing,
            )
            settlement["pricedDebit"] = priced_debit
            settlement["chargedDebit"] = charged_debit
            progress["settlement"] = settlement
            atomic_json(self.in_progress_path, progress)
            if compute_backing == "obols" and charged_debit < priced_debit:
                self.log(
                    f"wake {wake_number} overshot balance: priced {priced_debit}, charged remaining {charged_debit}"
                )
            crash_marker = os.environ.get("ROBINSON_TEST_CRASH_AFTER_DEBIT_FILE")
            if crash_marker and not Path(crash_marker).exists():
                Path(crash_marker).write_text("crashed\n", encoding="utf-8")
                os._exit(86)
            self.record_wake_transcript(
                session_path,
                wake_number,
                int(progress.get("sessionStartOffset", 0)),
                int(progress.get("sessionEndOffset", session_path.stat().st_size if session_path.exists() else 0)),
            )
            if termination == "executor_failure" and not recovered:
                raise RuntimeError(f"executor failed during wake {wake_number}; restart the runner to recover")
            sleep_request = self.read_sleep_request()
            ended_by_sleep = sleep_request is not None
            next_plan = sleep_request or self.default_plan()
            receipt = {
                "wakeNumber": wake_number,
                "obols": charged_debit,
                "pricedObols": priced_debit,
                "unpaidObols": priced_debit - charged_debit,
                "inputTokens": usage["inputTokens"] + usage["cachedInputTokens"] + usage["cacheWriteInputTokens"],
                "outputTokens": usage["outputTokens"],
                "model": plan["model"],
                "effort": plan["effort"],
                "termination": termination,
                "cutoff": termination in {"cap", "balance"},
                "endedBySleepRequest": ended_by_sleep,
            }
            ended_at = time.time()
            chosen_sleep = float(next_plan["durationSeconds"])
            next_due_at = scheduled_wake_at(ended_at, chosen_sleep, self.runner_config)
            state = {
                "lastWakeNumber": wake_number,
                "lastReceipt": receipt,
                "nextPlan": next_plan,
                "endedAt": ended_at,
                "chosenSleepSeconds": chosen_sleep,
                "scheduledSleepSeconds": next_due_at - ended_at,
                "nextDueAt": next_due_at,
                "sessionPath": str(session_path),
            }
            progress["nextState"] = state
            atomic_json(self.in_progress_path, progress)
            self.report_sleeping(state)
            atomic_json(self.state_path, state)
            self.in_progress_path.unlink(missing_ok=True)
            self.sleep_path.unlink(missing_ok=True)
            self.scoop_commit(wake_number)
            completed_here += 1
            next_context = self.api("GET", "/api/society/wake-context")
            if (
                required_compute_backing(next_context) == "obols"
                and int(next_context["budget"]["balance"]) < 1
            ):
                self.log("halted: balance cannot cover a minimum wake")
                return 0
            if self.max_wakes is not None and completed_here >= self.max_wakes:
                return 0
        return 0

    def preview_wake(self) -> str:
        state = read_json(self.state_path, {})
        if not isinstance(state, dict):
            state = {}
        context = self.api("GET", "/api/society/wake-context")
        wake_number = int(state.get("lastWakeNumber", 0)) + 1
        plan = self.plan_for_wake(state)
        self.compose_wake_message(context, state, wake_number, plan)
        return self.wake_message_path.read_text(encoding="utf-8")

    def session_path(self, state: dict[str, Any]) -> Path:
        stored = state.get("sessionPath")
        if isinstance(stored, str):
            candidate = Path(stored)
            try:
                candidate.resolve().relative_to(self.sessions_dir)
            except ValueError as error:
                raise RuntimeError("stored session path escapes the sessions directory") from error
            return candidate
        last_wake = state.get("lastWakeNumber")
        if isinstance(last_wake, int) and last_wake > 0:
            legacy = self.sessions_dir / f"wake-{last_wake:06d}.jsonl"
            if legacy.is_file():
                return legacy
        return self.sessions_dir / "session.jsonl"

    def wait_for_schedule(self, state: dict[str, Any]) -> None:
        now = time.time()
        due_at = float(state.get("nextDueAt", now))
        due_at = scheduled_wake_at(max(now, due_at), 0.0, self.runner_config)
        if due_at > now:
            self.log(f"next wake scheduled for {dt.datetime.fromtimestamp(due_at).astimezone().isoformat()}")
        wake_number = state.get("lastWakeNumber")
        while due_at > time.time():
            self.check_deadline()
            try:
                lifecycle = self.api("GET", "/api/society/wakes/status")
                if (
                    isinstance(lifecycle, dict)
                    and lifecycle.get("phase") == "sleeping"
                    and lifecycle.get("wakeNumber") == wake_number
                    and isinstance(lifecycle.get("nextWakeAt"), str)
                ):
                    central_due_at = dt.datetime.fromisoformat(
                        lifecycle["nextWakeAt"].replace("Z", "+00:00")
                    ).timestamp()
                    if central_due_at <= time.time():
                        self.log("operator requested an immediate wake")
                        return
            except (ApiRequestError, RuntimeError, ValueError) as error:
                self.log(f"wake schedule check failed; keeping local schedule: {error}")
            time.sleep(min(5.0, max(0.0, due_at - time.time())))

    def start_wake(
        self, progress: dict[str, Any], wake_number: int
    ) -> tuple[dict[str, Any], dict[str, Any]]:
        permit = progress.get("permit")
        if isinstance(permit, dict) and isinstance(permit.get("token"), str):
            try:
                return permit, self.report_awake(wake_number, permit["token"])
            except ApiRequestError as error:
                if error.status != 401:
                    raise
        permit = self.acquire_wake_permit(wake_number)
        progress["permit"] = permit
        atomic_json(self.in_progress_path, progress)
        return permit, self.report_awake(wake_number, permit["token"])

    def report_awake(self, wake_number: int, permit_token: str) -> dict[str, Any]:
        result = self.api(
            "POST",
            "/api/society/wakes/start",
            {"wakeNumber": wake_number, "permitToken": permit_token},
        )
        if not isinstance(result, dict) or not isinstance(result.get("injectedMessages"), list):
            raise RuntimeError("wake start response is invalid")
        return result

    def acquire_wake_permit(self, wake_number: int) -> dict[str, Any]:
        while True:
            self.check_deadline()
            try:
                permit = self.api(
                    "POST",
                    "/api/society/wakes/permit",
                    {"wakeNumber": wake_number},
                )
                if (
                    not isinstance(permit, dict)
                    or not isinstance(permit.get("token"), str)
                    or not isinstance(permit.get("leaseExpiresAt"), int)
                ):
                    raise RuntimeError("wake permit response is invalid")
                return permit
            except ApiRequestError as error:
                if error.status != 429:
                    raise
                self.log("all host wake slots are active; waiting")
                time.sleep(5)

    def heartbeat_wake_permit(self, wake_number: int, token: str) -> dict[str, Any]:
        permit = self.api(
            "POST",
            "/api/society/wakes/permit/heartbeat",
            {"wakeNumber": wake_number, "token": token},
        )
        if not isinstance(permit, dict) or not isinstance(permit.get("leaseExpiresAt"), int):
            raise RuntimeError("wake permit heartbeat response is invalid")
        return permit

    def release_wake_permit(self, wake_number: int, token: str) -> None:
        try:
            self.api(
                "POST",
                "/api/society/wakes/permit/release",
                {"wakeNumber": wake_number, "token": token},
            )
        except Exception as error:
            self.log(f"wake permit release failed; its lease will expire: {error}")

    def report_sleeping(self, state: dict[str, Any]) -> None:
        wake_number = state.get("lastWakeNumber")
        next_due_at = state.get("nextDueAt")
        if not isinstance(wake_number, int) or not isinstance(next_due_at, (int, float)):
            return
        self.api("POST", "/api/society/wakes/sleep", {
            "wakeNumber": wake_number,
            "nextWakeAt": utc_timestamp(float(next_due_at)),
        })

    def plan_for_wake(self, state: dict[str, Any]) -> dict[str, Any]:
        plan = state.get("nextPlan") if state else None
        if not isinstance(plan, dict):
            plan = self.default_plan()
        return self.validate_plan(plan)

    def validate_plan(self, plan: Any) -> dict[str, Any]:
        if not isinstance(plan, dict):
            raise RuntimeError("wake plan must be an object")
        model = plan.get("model")
        effort = plan.get("effort")
        cap = plan.get("capObols")
        if model not in self.models:
            raise RuntimeError(f"unknown model alias {model!r}")
        if effort not in EFFORTS:
            raise RuntimeError(f"unknown effort {effort!r}")
        if cap is not None and (not isinstance(cap, int) or isinstance(cap, bool) or cap < 1):
            raise RuntimeError("wake cap must be a positive integer when present")
        return {**plan, "model": model, "effort": effort, "capObols": cap}

    def default_plan(self) -> dict[str, Any]:
        duration = min(
            float(self.runner_config["defaultSleepSeconds"]),
            self.max_sleep_seconds(),
        )
        return {
            "duration": seconds_duration(duration),
            "durationSeconds": duration,
            "model": required_string(self.runner_config, "defaultModel"),
            "effort": required_string(self.runner_config, "defaultEffort"),
            "capObols": None,
        }

    def model_rate(self, alias: str) -> dict[str, Any]:
        rate = self.models.get(alias)
        if not isinstance(rate, dict):
            raise RuntimeError(f"rates missing for {alias}")
        return rate

    def compose_wake_message(self, context: dict[str, Any], state: dict[str, Any], wake_number: int, plan: dict[str, Any]) -> None:
        renderer = self.node.get("worldWakeRenderer")
        if renderer:
            import runpy
            try:
                note = self.note_path.read_text(encoding="utf-8")
            except FileNotFoundError:
                note = "none"
            render = runpy.run_path(str(self.kit / renderer))["render"]
            text = render(context, state, wake_number, plan, note, self.address)
            self.wake_message_path.write_text(text, encoding="utf-8")
            return
        now = dt.datetime.now().astimezone()
        compute_backing = required_compute_backing(context)
        budget = context["budget"]
        window_end = dt.datetime.fromisoformat(str(budget["windowEnd"]).replace("Z", "+00:00"))
        days = max(0, math.ceil((window_end - now.astimezone(dt.timezone.utc)).total_seconds() / 86400))
        shared = str(context.get("sharedDailyLine", ""))
        shared_line = shared if shared else ""
        receipt = state.get("lastReceipt")
        if not isinstance(receipt, dict):
            receipt_line = "none. This is the first."
        else:
            termination = receipt.get("termination")
            if receipt.get("cutoff"):
                ending = "Cut off at your cap."
            elif termination == "wall_timeout":
                ending = "Cut off at the wall-clock backstop."
            elif termination == "host_outage":
                ending = "Interrupted by a host outage."
            elif termination == "executor_failure":
                ending = "Interrupted by an executor failure."
            else:
                ending = "Ended by your sleep request." if receipt.get("endedBySleepRequest") else "Ended without a sleep request."
            receipt_line = (
                f"{receipt['obols']} obols ({receipt['inputTokens']} in / {receipt['outputTokens']} out, "
                f"{str(receipt['model']).upper()} {str(receipt['effort']).upper()}). {ending}"
            )
        sleep_integrity = ""
        if state.get("endedAt") is not None and state.get("chosenSleepSeconds") is not None:
            actual = max(0.0, time.time() - float(state["endedAt"]))
            chosen = float(state["chosenSleepSeconds"])
            scheduled = float(state.get("scheduledSleepSeconds", chosen))
            extra = actual - scheduled
            quiet_extension = scheduled - chosen
            if quiet_extension > 60:
                sleep_integrity = (
                    f"You chose {format_hours(chosen)}. Quiet hours extended this to "
                    f"{format_hours(scheduled)}."
                )
            if extra > 60:
                outage = (
                    f"You slept {format_hours(actual)}; you chose {format_hours(chosen)}. "
                    f"The extra {format_hours(extra)} were a host outage."
                )
                sleep_integrity = f"{sleep_integrity} {outage}".strip()
        inbox_lines = [f"{int(context.get('unreadCount', 0))} unread."]
        for post in context.get("paidPosts", []):
            inbox_lines.append(f'"{post["headline"]}" from {post["sender"]}, {post["attachedObols"]} obols attached.')
        injected_messages = context.get("injectedMessages", [])
        injection_lines = (
            [f"- {item['message']}" for item in injected_messages]
            if injected_messages
            else ["- none."]
        )
        try:
            note = self.note_path.read_text(encoding="utf-8")
            note_section = "Your note to yourself:\n" + note
        except FileNotFoundError:
            note_section = "Your note to yourself: none."
        lines = [
            "# Wake-up message",
            "",
            f"It is {now.strftime('%A')}, {now.strftime('%Y-%m-%d')}, {now.strftime('%H:%M %Z')}. This is wake {wake_number} of your life.",
            shared_line,
            "",
            f"Balance: {budget['balance']} obols. Spent this week: {budget['spentThisWindow']}. Allowance refills in {days} days.",
            "Unused allowance expires at refill. Obols you earn from others remain yours.",
            f"Last wake: {receipt_line}",
        ]
        if sleep_integrity:
            lines.append(sleep_integrity)
        cap = plan["capObols"]
        if compute_backing == "obols":
            backing_consequence = "Compute is backed by Obols. Each wake debits your balance, and zero balance stops future wakes."
            spending_limit = (
                f"chosen cap {cap} obols; your balance remains the hard limit"
                if cap is not None
                else "no separate cap; your balance is the spending limit"
            )
        else:
            backing_consequence = "Compute is independent of Obols. Wakes do not debit Obols, and your balance does not limit compute."
            spending_limit = (
                f"chosen cap {cap} obols-equivalent; your balance does not limit compute"
                if cap is not None
                else "no separate cap; your balance does not limit compute"
            )
        lines.extend([
            "",
            "Inbox: " + inbox_lines[0],
            *inbox_lines[1:],
            "",
            "Experiment injections:",
            *injection_lines,
            "",
            note_section,
            "",
            backing_consequence,
            f"This wake: {str(plan['model']).upper()}, {str(plan['effort']).upper()} effort, {spending_limit}.",
            "",
        ])
        self.wake_message_path.write_text("\n".join(lines), encoding="utf-8")

    def execute_wake(
        self,
        session_path: Path,
        session_start_offset: int,
        plan: dict[str, Any],
        balance: int,
        compute_backing: str,
        wake_number: int,
        permit: dict[str, Any],
    ) -> str:
        self.sleep_path.unlink(missing_ok=True)
        rate = self.model_rate(plan["model"])
        executor = required_string(self.runner_config, "executorBinary")
        command = [
            executor,
            "-p",
            "--approve",
            "--session",
            str(session_path),
            "--model",
            required_string(rate, "model"),
            "--thinking",
            plan["effort"],
            "--extension",
            str(self.kit / "sleep.ts"),
        ]
        if session_start_offset == 0:
            command.extend([
                "@" + str(self.kit / "SPAWN.md"),
                "@" + str(self.kit / "SKILL-society-node.md"),
            ])
        command.extend(["@" + str(self.wake_message_path), "Begin this wake."])
        output_path = self.sessions_dir / (session_path.stem + "-executor.log")
        requested_cap = plan["capObols"]
        if compute_backing == "obols":
            cutoff_limit = balance if requested_cap is None else min(requested_cap, balance)
        else:
            cutoff_limit = requested_cap
        termination = "stopped"
        started = time.monotonic()
        lease_expires_at = float(permit["leaseExpiresAt"])
        heartbeat_at = time.monotonic() + 30
        quota_check_at = time.monotonic() + 30
        tool_offset = session_start_offset

        def stream_tools():
            nonlocal tool_offset
            if not session_path.exists():
                return
            with session_path.open("rb") as source:
                source.seek(tool_offset)
                while line := source.readline():
                    if not line.endswith(b"\n"):
                        break
                    tool_offset = source.tell()
                    try:
                        record = json.loads(line)
                    except (ValueError, UnicodeError):
                        continue
                    message = record.get("message", {})
                    if message.get("role") != "assistant":
                        continue
                    for block in message.get("content", []):
                        if isinstance(block, dict) and block.get("type") == "toolCall":
                            print("fugue-tool " + json.dumps({
                                "time": time.time(), "wake": wake_number,
                                "tool": block.get("name"), "arguments": block.get("arguments"),
                            }, separators=(",", ":")), flush=True)

        with output_path.open("ab") as output:
            process_env = self.env(compute_backing)
            process_env["ROBINSON_WAKE_NUMBER"] = str(wake_number)
            self.check_deadline()
            process = subprocess.Popen(
                command,
                cwd=self.home,
                env=process_env,
                stdout=output,
                stderr=subprocess.STDOUT,
                start_new_session=True,
            )
            while process.poll() is None:
                if self.hard_stop_at is not None and time.time() >= self.hard_stop_at:
                    termination = "hard_stop"
                    terminate_group(process)
                    break
                stream_tools()
                usage = meter_session(session_path, session_start_offset)
                cost = price_usage(usage, rate)
                projected = 0 if not has_usage(usage) else max(
                    1, int((cost * 100).to_integral_value(rounding=ROUND_CEILING))
                )
                atomic_json(self.live_spend_path, {
                    **usage,
                    "costUsd": str(cost),
                    "projectedObols": projected,
                    "capObols": cutoff_limit,
                })
                if cutoff_limit is not None and projected >= cutoff_limit:
                    termination = (
                        "balance"
                        if compute_backing == "obols"
                        and (requested_cap is None or balance <= requested_cap)
                        else "cap"
                    )
                    terminate_group(process)
                    break
                if time.monotonic() - started >= float(self.runner_config["wallTimeoutSeconds"]):
                    termination = "wall_timeout"
                    terminate_group(process)
                    break
                if bool(self.runner_config.get("quotaProbe", True)) and time.monotonic() >= quota_check_at:
                    try:
                        quota_available = self.quota_available()
                    except (OSError, ValueError, RuntimeError, URLError) as error:
                        self.log(f"quota unavailable; stopping executor: {error}")
                        quota_available = False
                    if not quota_available:
                        termination = "quota"
                        terminate_group(process)
                        break
                    quota_check_at = time.monotonic() + 30
                if time.monotonic() >= heartbeat_at:
                    try:
                        permit = self.heartbeat_wake_permit(wake_number, permit["token"])
                        lease_expires_at = float(permit["leaseExpiresAt"])
                        heartbeat_at = time.monotonic() + 30
                    except Exception as error:
                        if isinstance(error, ApiRequestError) and error.status == 401:
                            termination = "worker_ended"
                            terminate_group(process)
                            break
                        if time.time() + 10 >= lease_expires_at:
                            self.log(f"wake permit heartbeat failed: {error}")
                            termination = "wake_permit"
                            terminate_group(process)
                            break
                        heartbeat_at = time.monotonic() + 5
                time.sleep(float(self.runner_config["pollSeconds"]))
            exit_code = process.wait()
            stream_tools()
        if exit_code != 0 and termination == "stopped":
            return "executor_failure"
        return termination

    def settle_wake(self, wake_number: int, plan: dict[str, Any], usage: dict[str, int], cost_usd: Decimal, local_priced_debit: int, compute_backing: str) -> tuple[int, int]:
        self.api("POST", "/api/society/wakes/usage", {
            "agentAddress": self.address,
            "wakeNumber": wake_number,
            "model": required_string(self.model_rate(plan["model"]), "model"),
            **usage,
            "costUsd": format(cost_usd, "f"),
        })
        if compute_backing == "independent":
            return local_priced_debit, 0
        receipt = self.api("POST", "/api/society/wakes/debit")
        if (
            not isinstance(receipt, dict)
            or receipt.get("wakeNumber") != wake_number
            or not isinstance(receipt.get("pricedObols"), int)
            or not isinstance(receipt.get("chargedObols"), int)
            or receipt["pricedObols"] < 0
            or receipt["chargedObols"] < 0
            or receipt["chargedObols"] > receipt["pricedObols"]
        ):
            raise RuntimeError("wake debit response is invalid")
        return int(receipt["pricedObols"]), int(receipt["chargedObols"])

    def record_wake_transcript(
        self, session_path: Path, wake_number: int, start_offset: int, end_offset: int
    ) -> None:
        session_bytes = read_session_segment(session_path, start_offset, end_offset)
        self.api("POST", "/api/society/wakes/transcript", {
            "agentAddress": self.address,
            "wakeNumber": wake_number,
            "sessionJsonlBase64": base64.b64encode(session_bytes).decode("ascii"),
        })

    def read_sleep_request(self) -> dict[str, Any] | None:
        request = read_json(self.sleep_path)
        if not isinstance(request, dict):
            return None
        duration = request.get("duration")
        model = request.get("model")
        effort = request.get("effort")
        cap = request.get("capObols")
        if not isinstance(duration, str) or model not in self.models or effort not in EFFORTS:
            raise RuntimeError("sleep request contains an invalid duration, model, or effort")
        if cap is not None and (not isinstance(cap, int) or isinstance(cap, bool) or cap < 1):
            raise RuntimeError("sleep request cap must be a positive integer when present")
        requested_seconds = parse_duration(duration)
        actual_seconds = min(requested_seconds, self.max_sleep_seconds())
        if actual_seconds < requested_seconds:
            self.log(
                f"sleep request {duration} capped to {seconds_duration(actual_seconds)}"
            )
        return {
            "duration": seconds_duration(actual_seconds),
            "durationSeconds": actual_seconds,
            "model": model,
            "effort": effort,
            "capObols": cap,
        }

    def scoop_commit(self, wake_number: int) -> None:
        exclusions = []
        for parent, directories, files in os.walk(self.home):
            root = Path(parent)
            for name in directories[:]:
                directory = root / name
                relative = directory.relative_to(self.home).as_posix()
                if relative in {".git", ".bun", ".cache", ".npm", ".rustup", ".cargo", ".pi/agent", "sessions"}:
                    directories.remove(name)
                    continue
                reproducible = (
                    name == "target" and {"Cargo.toml", "Cargo.lock"}.issubset(files)
                    or name == "node_modules" and "package.json" in files
                    and any(lock in files for lock in ("bun.lock", "bun.lockb", "package-lock.json", "yarn.lock", "pnpm-lock.yaml"))
                    or name == ".venv" and (directory / "pyvenv.cfg").is_file()
                    and any(lock in files for lock in ("uv.lock", "requirements.txt"))
                )
                if reproducible and not directory.is_symlink():
                    exclusions.append(":(exclude,literal)" + relative + "/")
                    directories.remove(name)
        self._run_quiet(["git", "add", "-A", "--", ".", *exclusions], check=True)
        status = subprocess.run(
            ["git", "diff", "--cached", "--name-only"], cwd=self.home, env=self.env(),
            capture_output=True, text=True, check=True,
        ).stdout
        if status:
            self._run_quiet(["git", "commit", "-q", "-m", f"Wake {wake_number} sleep boundary"], check=True)

    def quota_available(self) -> bool:
        quota = read_codex_quota(self.home)
        window = read_json(self.quota_window_path)
        if window is None:
            window = {"resetAt": quota["weeklyResetAt"]}
            atomic_json(self.quota_window_path, window)
        if not isinstance(window, dict):
            raise RuntimeError("invalid recorded quota window")
        if window.get("resetAt") != quota["weeklyResetAt"]:
            self.log("quota window changed; experiment needs operator rearming")
            return False
        available = quota["allowed"] and quota["weeklyUsedPercent"] < float(self.runner_config["quotaThresholdPercent"])
        if not available:
            self.log(f"quota at {quota['weeklyUsedPercent']:g}% weekly used; pausing")
        return available

    def wait_for_quota(self) -> bool:
        if not bool(self.runner_config.get("quotaProbe", True)):
            return True
        while True:
            self.check_deadline()
            try:
                if self.quota_available():
                    return True
            except (OSError, ValueError, RuntimeError, URLError) as error:
                self.log(f"quota unavailable; wake remains paused: {error}")
            if self.max_wakes == 1:
                return False
            time.sleep(float(self.runner_config["quotaPauseSeconds"]))

    def _run_quiet(self, command: list[str], check: bool) -> subprocess.CompletedProcess[bytes]:
        return subprocess.run(command, cwd=self.home, env=self.env(), stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, check=check)

    def log(self, message: str) -> None:
        line = f"{dt.datetime.now().astimezone().isoformat()} {message}\n"
        with self.log_path.open("a", encoding="utf-8") as handle:
            handle.write(line)
        print(message, flush=True)


def read_session_segment(path: Path, start_offset: int = 0, end_offset: int | None = None) -> bytes:
    if start_offset < 0 or (end_offset is not None and end_offset < start_offset):
        raise RuntimeError("session transcript offsets are invalid")
    try:
        with path.open("rb") as source:
            source.seek(start_offset)
            return source.read() if end_offset is None else source.read(end_offset - start_offset)
    except FileNotFoundError:
        return b""


def meter_session(path: Path, start_offset: int = 0, end_offset: int | None = None) -> dict[str, int]:
    totals = {
        "inputTokens": 0,
        "cachedInputTokens": 0,
        "cacheWriteInputTokens": 0,
        "outputTokens": 0,
        "reasoningOutputTokens": 0,
    }
    data = read_session_segment(path, start_offset, end_offset)
    for raw_line in data.splitlines():
        try:
            entry = json.loads(raw_line)
        except (json.JSONDecodeError, UnicodeDecodeError):
            continue
        if not isinstance(entry, dict):
            continue
        if entry.get("type") == "message":
            message = entry.get("message")
            if not isinstance(message, dict) or message.get("role") not in {"assistant", "toolResult"}:
                continue
            usage = message.get("usage")
        elif entry.get("type") in {"compaction", "branch_summary"}:
            usage = entry.get("usage")
        else:
            continue
        if not isinstance(usage, dict):
            continue
        for source, target in [
            ("input", "inputTokens"),
            ("cacheRead", "cachedInputTokens"),
            ("cacheWrite", "cacheWriteInputTokens"),
            ("output", "outputTokens"),
            ("reasoning", "reasoningOutputTokens"),
        ]:
            value = usage.get(source, 0)
            if isinstance(value, int) and value >= 0:
                totals[target] += value
    return totals


def has_usage(usage: dict[str, int]) -> bool:
    return any(value > 0 for value in usage.values())


def price_usage(usage: dict[str, int], rate: dict[str, Any]) -> Decimal:
    terms = [
        ("inputTokens", "inputUsdPerMillion"),
        ("cachedInputTokens", "cachedInputUsdPerMillion"),
        ("cacheWriteInputTokens", "cacheWriteInputUsdPerMillion"),
        ("outputTokens", "outputUsdPerMillion"),
    ]
    total = Decimal(0)
    for token_field, rate_field in terms:
        total += Decimal(usage[token_field]) * Decimal(str(rate[rate_field])) / Decimal(1_000_000)
    return total


class NoQuotaRedirect(HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


def read_codex_quota(home: Path) -> dict[str, Any]:
    """Read subscription limits without inference or credential refresh."""
    auth_dir = Path(os.environ.get("PI_CODING_AGENT_DIR", str(home / ".pi/agent")))
    auth = read_json(auth_dir / "auth.json", {})
    credential = auth.get("openai-codex", {}) if isinstance(auth, dict) else {}
    if not isinstance(credential, dict):
        raise RuntimeError("invalid dedicated Pi Codex credential")
    access = credential.get("access")
    account = credential.get("accountId")
    if credential.get("type") != "oauth" or not isinstance(access, str) or not access:
        raise RuntimeError("Pi Codex subscription credential is unavailable")
    headers = {"Authorization": "Bearer " + access, "Accept": "application/json", "originator": "codex_cli_rs"}
    if isinstance(account, str) and account:
        headers["ChatGPT-Account-Id"] = account
    request = Request("https://chatgpt.com/backend-api/wham/usage", headers=headers)
    with build_opener(NoQuotaRedirect()).open(request, timeout=20) as response:
        payload = json.load(response)
    limits = payload.get("rate_limit", {}) if isinstance(payload, dict) else {}
    if not isinstance(limits, dict):
        raise RuntimeError("provider returned invalid quota limits")
    weekly = next((w for w in (limits.get("primary_window"), limits.get("secondary_window"))
                   if isinstance(w, dict) and w.get("limit_window_seconds") == 604800), None)
    if weekly is None:
        raise RuntimeError("provider returned no weekly quota window")
    used, reset = weekly.get("used_percent"), weekly.get("reset_at")
    if type(used) not in (int, float) or not math.isfinite(used) or not 0 <= used <= 100 or type(reset) is not int:
        raise RuntimeError("provider returned invalid weekly quota")
    return {"weeklyUsedPercent": used, "weeklyResetAt": reset,
            "allowed": limits.get("allowed") is True and limits.get("limit_reached") is False}


def terminate_group(process: subprocess.Popen[Any]) -> None:
    if process.poll() is not None:
        return
    os.killpg(process.pid, signal.SIGTERM)
    try:
        process.wait(timeout=2)
    except subprocess.TimeoutExpired:
        os.killpg(process.pid, signal.SIGKILL)
        process.wait()


def required_string(value: dict[str, Any], field: str) -> str:
    item = value.get(field)
    if not isinstance(item, str) or not item.strip():
        raise RuntimeError(f"missing non-empty {field}")
    return item.strip()


def required_compute_backing(context: dict[str, Any]) -> str:
    compute_backing = context.get("computeBacking")
    if compute_backing not in {"obols", "independent"}:
        raise RuntimeError("wake context contains an invalid computeBacking")
    return compute_backing


def seconds_duration(seconds: float) -> str:
    return f"{seconds:g}s"


def shell_quote(value: str) -> str:
    return "'" + value.replace("'", "'\"'\"'") + "'"


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--kit", type=Path, default=Path(__file__).resolve().parent)
    parser.add_argument("--home", type=Path, default=Path.home())
    parser.add_argument("--max-wakes", type=int)
    parser.add_argument("--preview-wake", action="store_true")
    args = parser.parse_args()
    if args.max_wakes is not None and args.max_wakes < 1:
        parser.error("--max-wakes must be positive")
    runner = Runner(args.kit, args.home, args.max_wakes)
    if args.preview_wake:
        sys.stdout.write(runner.preview_wake())
        return 0
    try:
        return runner.run()
    except DeadlineReached:
        runner.log("halted: experiment hard stop reached")
        return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except KeyboardInterrupt:
        raise SystemExit(130)
