#!/usr/bin/env python3
"""One host owner admits a fair cohort; at most configured VMs exist at once."""
import argparse
import collections
import datetime as dt
import fcntl
import json
import importlib.util
import os
from pathlib import Path
import shutil
import signal
import subprocess
import time
import tomllib
from urllib.request import Request, urlopen

if __package__:
    from .robinson_runner import hard_stop_epoch, scheduled_wake_at
else:
    from robinson_runner import hard_stop_epoch, scheduled_wake_at


def main():
    argparse.ArgumentParser(description=__doc__).parse_args()
    config_path = Path(os.environ["FUGUE_CONFIG"])
    config = tomllib.loads(config_path.read_text())
    society = config["society"]
    kit_root = Path(os.environ.get("FUGUE_KIT_DIR", society["kit_dir"]))
    if not kit_root.is_absolute():
        kit_root = config_path.parent / kit_root
    sandbox = Path(os.environ["FUGUE_SANDBOX_BIN"])
    state_root = Path(os.environ.get("SANDBOX_STATE_DIR", sandbox.parent / "state"))
    # One admission owner across every cohort sharing these VM disks.
    lock = (state_root / "society-coordinator.lock").open("a")
    try:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
    except BlockingIOError:
        print("keeper: another admission owner exists", flush=True)
        return 0
    for pid_file in state_root.glob("*.pid"):
        try:
            pid = int(pid_file.read_text().strip())
            if pid <= 1:
                raise ValueError("invalid VM pid")
            os.kill(pid, 0)
        except (ProcessLookupError, FileNotFoundError):
            continue
        print("keeper: an earlier VM is still running; refusing admission", flush=True)
        return 75
    agents = {}
    for kit in sorted(kit_root.iterdir()):
        if not kit.is_dir():
            continue
        node = json.loads((kit / "node.json").read_text())
        vm = kit.name if society["instance_id"] == "default" else society["instance_id"] + "-" + kit.name
        agents[kit.name] = {"key": (kit / "agent.key").read_text().strip(),
                            "runner": node["runner"], "vm": vm, "retryAt": 0}
    driver = os.environ.get("FUGUE_COHORT_HOOK")
    if not driver:
        raise RuntimeError("host cohort driver is required before admission")
    spec = importlib.util.spec_from_file_location("cohort_hook", driver)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    controller = module.Controller(config_path, state_root, agents)
    waiting = collections.deque(agents)
    active = {}
    maximum = int(society["runner"].get("max_active_wakes", 3))
    deadline = hard_stop_epoch(society["runner"].get("hard_stop_at"))
    base_url = f"http://{config['api']['host']}:{config['api']['port']}"
    reserve = int(os.environ.get("FUGUE_HOST_FREE_RESERVE_BYTES", "0"))
    reserve_path = os.environ.get("FUGUE_HOST_FREE_PATH", str(sandbox.parent))
    state_reserve = int(os.environ.get("FUGUE_STATE_FREE_RESERVE_BYTES", "0"))
    if maximum < 1 or reserve < 0 or state_reserve < 0:
        raise ValueError("invalid admission or storage limit")
    stopped = False
    quota_retry_at = 0

    def stop(signum, frame):
        nonlocal stopped
        stopped = True

    signal.signal(signal.SIGTERM, stop)
    signal.signal(signal.SIGINT, stop)
    # A transient assertion, not a global power-management setting.
    awake = subprocess.Popen(["/usr/bin/caffeinate", "-i", "-w", str(os.getpid())]) if Path("/usr/bin/caffeinate").is_file() else None

    def expired():
        return deadline is not None and time.time() >= deadline

    def api(agent, path):
        remaining = max(0.01, deadline - time.time()) if deadline else 1
        request = Request(base_url + path, headers={"Authorization": "Bearer " + agent["key"]})
        with urlopen(request, timeout=min(1, remaining)) as response:
            return json.load(response)

    def storage_room():
        return ((not reserve or shutil.disk_usage(reserve_path).free >= reserve)
                and (not state_reserve or shutil.disk_usage(state_root).free >= state_reserve))

    try:
        while not stopped:
            if not controller.tick():
                time.sleep(0.1)
                continue
            if expired():
                break
            if not storage_room():
                print("keeper: storage reserve crossed; stopping cohort", flush=True)
                break
            for name, process in list(active.items()):
                result = process.poll()
                if result is None:
                    continue
                process.wait()
                del active[name]
                waiting.append(name)
                if result == 75:
                    quota_retry_at = time.time() + float(agents[name]["runner"]["quotaPauseSeconds"])
                elif result:
                    agents[name]["retryAt"] = time.time() + 30
            if time.time() >= quota_retry_at:
                for _ in range(len(waiting)):
                    if len(active) >= maximum or stopped or expired():
                        break
                    name = waiting.popleft()
                    agent = agents[name]
                    if time.time() < agent["retryAt"]:
                        waiting.append(name)
                        continue
                    try:
                        status = api(agent, "/api/society/wakes/status")
                        context = api(agent, "/api/society/wake-context")
                        if context["computeBacking"] == "obols" and context["budget"]["balance"] < 1:
                            agent["retryAt"] = time.time() + 30
                            waiting.append(name)
                            continue
                        due = time.time()
                        if status and status.get("phase") == "sleeping" and status.get("nextWakeAt"):
                            due = dt.datetime.fromisoformat(status["nextWakeAt"].replace("Z", "+00:00")).timestamp()
                        due = scheduled_wake_at(max(time.time(), due), 0, agent["runner"])
                        if due > time.time():
                            waiting.append(name)
                            continue
                        if stopped or expired() or not storage_room():
                            stopped = True
                            break
                        command = [str(sandbox), "run", agent["vm"], "--", "env",
                                   "FUGUE_API_URL=" + society["guest_api_url"]]
                        if deadline is not None:
                            early_stop = dt.datetime.fromtimestamp(deadline - 5, dt.timezone.utc).isoformat()
                            command.append("ROBINSON_HARD_STOP_AT=" + early_stop)
                        command.extend(["python3", "/home/coder/society/runner.py", "--kit",
                                        "/home/coder/society", "--home", "/home/coder", "--max-wakes", "1"])
                        environment = dict(os.environ)
                        environment.update(controller.environment(name, status))
                        if not controller.tick():
                            waiting.appendleft(name)
                            break
                        active[name] = subprocess.Popen(command, cwd=sandbox.parent,
                                                        env=environment, start_new_session=True)
                    except (OSError, ValueError, KeyError) as error:
                        print(f"keeper: unavailable for {name}: {error}", flush=True)
                        agent["retryAt"] = time.time() + 30
                        waiting.append(name)
            time.sleep(0.1)
        return 0
    finally:
        # Retained cohorts belong to an explicit operator transition, not normal teardown.
        retained = controller.frozen
        if not retained:
            # Terminate all groups together; escalation has one shared two-second grace.
            for process in active.values():
                try:
                    os.killpg(process.pid, signal.SIGTERM)
                except ProcessLookupError:
                    pass
                except PermissionError:
                    # macOS can reject killpg for an exited, unreaped group leader.
                    if process.poll() is None:
                        raise
            grace = time.monotonic() + 2
            while any(process.poll() is None for process in active.values()) and time.monotonic() < grace:
                time.sleep(0.02)
            for process in active.values():
                if process.poll() is None:
                    try:
                        os.killpg(process.pid, signal.SIGKILL)
                    except ProcessLookupError:
                        pass
                    except PermissionError:
                        if process.poll() is None:
                            raise
                process.wait()
            controller.close()
        if awake is not None:
            awake.terminate()
            awake.wait()
        print("keeper: cohort retained" if retained else "keeper: stopped; no more VMs", flush=True)


if __name__ == "__main__":
    raise SystemExit(main())
