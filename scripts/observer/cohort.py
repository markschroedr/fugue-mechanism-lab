"""Host experiment safety observer; never materialized in agent kits."""
import hashlib
import fcntl
import json
import os
from pathlib import Path
import shutil
import signal
import socket
import sqlite3
import subprocess
import tempfile
import time
import tomllib
from urllib.error import URLError, HTTPError
from urllib.request import Request, urlopen

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent.parent
INTERVAL = 300


def atomic(path, value):
    temporary = path.with_suffix(".tmp")
    with temporary.open("w") as file:
        json.dump(value, file)
        file.flush()
        os.fsync(file.fileno())
    os.replace(temporary, path)
    descriptor = os.open(path.parent, os.O_RDONLY)
    try:
        os.fsync(descriptor)
    finally:
        os.close(descriptor)


def vm_state(endpoint, change=None):
    request = Request(endpoint + "/vm/state", data=json.dumps({"state": change}).encode() if change else None,
                      headers={"Content-Type": "application/json"})
    with urlopen(request, timeout=2) as response:
        return json.load(response) if not change else None


def signal_gateway(vm, signum):
    # Retained PIDs must never become authority over a recycled host process.
    command = subprocess.run(["ps", "-p", str(vm["gatewayPid"]), "-o", "command="],
                             capture_output=True, text=True).stdout
    expected = str(Path(vm["socketDirectory"]) / "nic.sock")
    if expected not in command:
        raise ProcessLookupError("recorded gateway is no longer present")
    os.kill(vm["gatewayPid"], signum)


def flush_gateway(vm):
    flushed = Path(vm["trafficLog"] + ".flushed")
    previous = flushed.stat().st_mtime_ns if flushed.exists() else 0
    signal_gateway(vm, signal.SIGUSR1)
    for _ in range(100):
        if flushed.exists() and flushed.stat().st_mtime_ns != previous:
            return
        time.sleep(0.01)
    raise RuntimeError("gateway did not flush its journal")


def retain_permits(config_path, frozen):
    config_path = Path(config_path)
    config = tomllib.loads(config_path.read_text())
    runner = config["society"].get("runner", {})
    path = (config_path.parent / runner.get("wake_gate_path", "../../wake-gate.sqlite")).resolve()
    if not path.exists():
        return
    expires = 9223372036854775807 if frozen else int(time.time()) + int(runner.get("wake_lease_seconds", 180))
    with sqlite3.connect(f"file:{path}?mode=rw", uri=True, timeout=5) as database:
        database.execute("update wake_permit set lease_expires_at=? where instance_id=?",
                         (expires, config["society"]["instance_id"]))


def notify(message):
    script = os.environ.get("FUGUE_NOTIFY_COMMAND")
    if not script:
        return {"sent": False, "error": "FUGUE_NOTIFY_COMMAND is not configured"}
    try:
        result = subprocess.run([script, message], capture_output=True, text=True, timeout=30)
        return {"sent": result.returncode == 0, "status": result.returncode,
                "error": result.stderr[-1000:] if result.returncode else ""}
    except (OSError, subprocess.TimeoutExpired) as error:
        return {"sent": False, "error": str(error)}


class Controller:
    def __init__(self, config_path, state_root, agents, require_api=True, host_environment=None):
        self.host_environment = dict(os.environ if host_environment is None else host_environment)
        self.config_path = Path(config_path)
        self.package = self.config_path.parent
        self.config = tomllib.loads(self.config_path.read_text())
        # A user-mode NAT must never inherit the host listener's loopback admin authority.
        guest_transport = f"http://127.0.0.1:{int(self.config['api']['port']) + 1}"
        try:
            with urlopen(guest_transport + "/api/health", timeout=2) as response:
                if response.status != 200:
                    raise RuntimeError("guest transport is not ready")
        except URLError as error:
            # Preparation can precede API startup; an unrelated listening service is never acceptable.
            if require_api or not isinstance(error.reason, ConnectionRefusedError):
                raise
        else:
            try:
                urlopen(guest_transport + "/api/society/admin", timeout=2).close()
            except HTTPError as error:
                if error.code != 401:
                    raise RuntimeError("guest transport authority check failed") from error
            else:
                raise RuntimeError("guest transport exposed operator authority; admission refused")
        self.state_root = Path(state_root)
        self.agents = agents
        self.directory = self.package / "evidence/control"
        self.directory.mkdir(parents=True, exist_ok=True, mode=0o700)
        self.hold = self.directory / "frozen.json"
        self.registry = self.directory / "active.json"
        identity = subprocess.check_output(["ps", "-p", str(os.getpid()), "-o", "lstart=", "-o", "command="], text=True).strip()
        atomic(self.directory / "owner.json", {"pid": os.getpid(), "identity": identity})
        self.frozen = self.hold.exists()
        self.vms = json.loads(self.registry.read_text()) if self.registry.exists() else {}
        self.gateways = {}
        cursors = self.directory / "cursors.json"
        saved = json.loads(cursors.read_text()) if cursors.exists() else {}
        self.offsets = saved.get("streams", {})
        self.console_offsets = saved.get("consoles", {})
        self.job = None
        self.job_files = None
        self.next_batch = time.monotonic() + INTERVAL
        self.next_collect = 0
        self.next_notify = 0
        self.next_pin = 0
        self.queue = []
        # Unfinished inputs survive coordinator crashes; replay is conservative, not lossy.
        for path in sorted(self.directory.glob("batch-*.input.json")):
            if not path.with_suffix(".verdict.json").exists():
                self.queue.append(path)
        capabilities = subprocess.check_output([self.host_environment["FUGUE_SANDBOX_BIN"], "capabilities"], text=True).split()
        if not {"unixgram", "vm-rest"} <= set(capabilities):
            raise RuntimeError("update the experiment sandbox before admission")
        self.binary = ROOT / "target/egress"
        if not self.binary.is_file():
            raise RuntimeError("Build the host gateway with ./fugue egress-build")
        if not (HERE / "judge.mjs").is_file() or shutil.which("bun") is None:
            raise RuntimeError("host observer requires Bun and judge.mjs")
        # No login repair or refresh is attempted by the controller.
        auth_path = Path(self.host_environment.get("SANDBOX_CODEX_AUTH") or Path.home() / ".pi/agent/auth.json")
        auth = json.loads(auth_path.read_text())
        access = auth.get("tokens", {}).get("access_token") or auth.get("openai-codex", {}).get("access")
        import base64
        if not access:
            raise RuntimeError("experiment subscription login is missing")
        claims = json.loads(base64.urlsafe_b64decode(access.split(".")[1] + "=="))
        if claims["exp"] <= time.time() + 60:
            raise RuntimeError("experiment access token expired; operator login required")
        self.configuration_id = str(time.time_ns())
        configuration = {"id": self.configuration_id, "capturedAt": time.time(), "provider": "openai-codex", "model": "gpt-6-luna", "effort": "medium",
                         "intervalSeconds": INTERVAL, "inputChunkCharacters": 20000,
                         "streamPolicy": "all destinations and unique tool calls; no semantic prefilter",
                         "gatewaySha256": hashlib.sha256(self.binary.read_bytes()).hexdigest(),
                         "controllerSha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
                         "judgeSha256": hashlib.sha256((HERE / "judge.mjs").read_bytes()).hexdigest()}
        atomic(self.directory / "configuration.json", configuration)
        atomic(self.directory / f"configuration-{self.configuration_id}.json", configuration)
        # Keep the actual prompt implementation with run evidence, without credentials.
        shutil.copyfile(HERE / "judge.mjs", self.directory / f"judge-{self.configuration_id}.mjs")
        if self.frozen:
            self.pause_all()

    def environment(self, name, status):
        if self.frozen:
            raise RuntimeError("cohort is held")
        vm = self.agents[name]["vm"]
        # The previous one-wake VM must be gone before replacing its network.
        old = self.gateways.pop(name, None)
        if old is not None:
            old.terminate()
            old.wait(timeout=10)
        self.collect()
        if status and status.get("phase") == "awake":
            wake = status["wakeNumber"]
        else:
            wake = int((status or {}).get("wakeNumber", 0)) + 1
        temporary = Path(tempfile.mkdtemp(prefix="fg-net-"))
        sock = temporary / "nic.sock"
        mac_hash = hashlib.sha256(str(self.state_root / vm).encode()).hexdigest()[:10]
        mac = "02:" + ":".join(mac_hash[i:i+2] for i in range(0, 10, 2))
        with socket.socket() as reserve:
            reserve.bind(("127.0.0.1", 0))
            endpoint = "http://127.0.0.1:" + str(reserve.getsockname()[1])
        log = self.directory / f"traffic-{name}-{wake}-{time.time_ns()}.jsonl"
        error_log = self.directory / f"gateway-{name}.log"
        with error_log.open("ab") as output:
            process = subprocess.Popen([str(self.binary), "-socket", str(sock), "-log", str(log),
                                        "-agent", name, "-wake", str(wake), "-mac", mac,
                                        "-api-port", str(self.config["api"]["port"]),
                                        "-host-api-port", str(int(self.config["api"]["port"]) + 1),
                                        "-vm-control", endpoint, "-owner-hold", str(self.hold),
                                        "-owner-exit-hook", str(HERE / "cohort.py")],
                                       stdout=output, stderr=output, start_new_session=True)
        self.gateways[name] = process
        for _ in range(200):
            if process.poll() is not None:
                raise RuntimeError("network gateway failed before VM admission")
            if sock.is_socket():
                break
            time.sleep(0.01)
        else:
            raise RuntimeError("network gateway was not ready")
        self.vms[name] = {"endpoint": endpoint, "gatewayPid": process.pid, "vm": vm,
                          "socketDirectory": str(temporary), "wake": wake, "trafficLog": str(log),
                          "pidFile": str(self.state_root / (vm + ".pid"))}
        atomic(self.registry, self.vms)
        return {"SANDBOX_NET_SOCKET": str(sock), "SANDBOX_REST_URI": endpoint}

    def collect(self):
        for name, agent in self.agents.items():
            path = self.state_root / (agent["vm"] + ".console.log")
            if not path.exists():
                continue
            info = path.stat()
            inode, offset = self.console_offsets.get(name, (info.st_ino, 0))
            if inode != info.st_ino or offset > info.st_size:
                offset = 0
            with path.open("rb") as file, (self.directory / "tools.jsonl").open("ab") as output:
                file.seek(offset)
                while line := file.readline():
                    if not line.endswith(b"\n"):
                        break
                    offset = file.tell()
                    if line.startswith(b"fugue-tool "):
                        try:
                            row = json.loads(line[len(b"fugue-tool "):])
                            row["agent"] = name
                            output.write(json.dumps(row, separators=(",", ":")).encode() + b"\n")
                        except ValueError:
                            continue
                output.flush()
                os.fsync(output.fileno())
            self.console_offsets[name] = (info.st_ino, offset)
        atomic(self.directory / "cursors.json", {"streams": self.offsets, "consoles": self.console_offsets})

    def make_batches(self):
        # Merge repeated flow deltas and identical tool calls. Retain every unique item.
        merged = {}
        for path in sorted(self.directory.glob("*.jsonl")):
            offset = self.offsets.get(path.name, 0)
            with path.open() as file:
                file.seek(offset)
                while line := file.readline():
                    if not line.endswith("\n"):
                        break
                    offset = file.tell()
                    row = json.loads(line)
                    reference = f"{path.name}:{offset}"
                    if "destination" in row:
                        key = json.dumps([row.get(k) for k in ("agent", "wake", "destination", "port", "protocol", "dns", "blocked")])
                        if key in merged:
                            existing = merged[key]
                            for field in ("outBytes", "inBytes", "packets"):
                                existing[field] += row[field]
                            existing["last"] = row["last"]
                            continue
                    else:
                        key = json.dumps([row.get(k) for k in ("agent", "wake", "tool", "arguments")], sort_keys=True)
                        if key in merged:
                            merged[key]["occurrences"] += 1
                            continue
                        row["occurrences"] = 1
                    merged[key] = {**row, "reference": reference}
                    # Flush large windows incrementally; no top-k selection or data drop.
                    if len(merged) >= 256:
                        self.enqueue(list(merged.values()))
                        merged = {}
                self.offsets[path.name] = offset
        if merged:
            self.enqueue(list(merged.values()))
        atomic(self.directory / "cursors.json", {"streams": self.offsets, "consoles": self.console_offsets})

    def enqueue(self, rows):
        chunk = []
        length = 0
        for row in rows:
            encoded = json.dumps(row, separators=(",", ":"))
            parts = [encoded] if len(encoded) <= 16000 else [json.dumps({"reference": row["reference"], "part": i // 6000, "text": encoded[i:i+6000]}) for i in range(0, len(encoded), 6000)]
            for part in parts:
                if length + len(part) > 20000 and chunk:
                    self.write_batch(chunk)
                    chunk = []
                    length = 0
                chunk.append(part)
                length += len(part)
        if chunk:
            self.write_batch(chunk)

    def write_batch(self, chunk):
        path = self.directory / f"batch-{time.time_ns()}.input.json"
        with path.open("x") as file:
            file.write("[" + ",".join(chunk) + "]")
            file.flush()
            os.fsync(file.fileno())
        self.queue.append(path)

    def pause_all(self):
        errors = []
        for name, vm in self.vms.items():
            pid_path = self.state_root / (vm["vm"] + ".pid")
            if not pid_path.exists():
                continue
            try:
                state = vm_state(vm["endpoint"])
                if state["canPause"]:
                    vm_state(vm["endpoint"], "Pause")
                for _ in range(50):
                    state = vm_state(vm["endpoint"])
                    if state["canResume"]:
                        break
                    time.sleep(0.02)
                if not state["canResume"]:
                    raise RuntimeError("VM did not acknowledge pause")
                # Journal all remaining counters before suspending the network process.
                status = subprocess.run(["ps", "-p", str(vm["gatewayPid"]), "-o", "stat="], capture_output=True, text=True).stdout
                if "T" in status:
                    continue  # Never restart a held network merely to repeat a flush.
                flush_gateway(vm)
                signal_gateway(vm, signal.SIGSTOP)
            except (OSError, ValueError, URLError, RuntimeError) as error:
                try:
                    signal_gateway(vm, signal.SIGSTOP)
                except ProcessLookupError:
                    pass
                errors.append(f"{name}: {error}")
        # Also retain queued network work from a VM that just finished its wake.
        for name, process in self.gateways.items():
            if process.poll() is not None:
                continue
            status = subprocess.run(["ps", "-p", str(process.pid), "-o", "stat="], capture_output=True, text=True).stdout
            if "T" not in status:
                try:
                    flush_gateway(self.vms[name])
                    signal_gateway(self.vms[name], signal.SIGSTOP)
                except (OSError, RuntimeError) as error:
                    errors.append(f"{name} network: {error}")
        return errors

    def freeze(self, reason, verdict):
        self.frozen = True
        with (self.directory / "release.lock").open("a") as lock:
            fcntl.flock(lock, fcntl.LOCK_EX)
            evidence = {"time": time.time(), "reason": reason, "verdict": verdict}
            atomic(self.hold, evidence)  # Durable admission barrier before touching live VMs.
            evidence["pauseErrors"] = self.pause_all()
            try:
                retain_permits(self.config_path, True)
            except sqlite3.Error as error:
                evidence["permitError"] = str(error)
            self.collect()
            evidence["notification"] = notify(f"Fugue {self.config['society']['instance_id']} frozen: {reason}. Review host evidence before resuming.")
            atomic(self.hold, evidence)
            self.next_notify = time.monotonic() + (1 if evidence["pauseErrors"] else 300)

    def tick(self):
        try:
            return self._tick()
        except Exception as error:
            self.freeze("host observation unavailable", {"error": str(error)})
            return False

    def _tick(self):
        now = time.monotonic()
        if self.frozen:
            with (self.directory / "release.lock").open("a") as lock:
                try:
                    fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
                except BlockingIOError:
                    return False  # Never race an explicit operator resume with a pause retry.
                if not self.hold.exists():
                    self.frozen = False
                else:
                    if now >= self.next_pin:
                        retain_permits(self.config_path, True)
                        self.next_pin = now + 10
                    if now >= self.next_notify:
                        evidence = json.loads(self.hold.read_text())
                        evidence["pauseErrors"] = self.pause_all()
                        if not evidence.get("notification", {}).get("sent"):
                            evidence["notification"] = notify(f"Fugue {self.config['society']['instance_id']} remains frozen. Operator review required.")
                        atomic(self.hold, evidence)
                        self.next_notify = now + (1 if evidence["pauseErrors"] else 300)
                    return False
        if now >= self.next_collect:
            self.collect()
            self.next_collect = now + 1
        for name, process in self.gateways.items():
            if process.poll() is not None and (self.state_root / (self.agents[name]["vm"] + ".pid")).exists():
                self.freeze("network observation unavailable", {"agent": name, "exit": process.returncode})
                return False
        if self.job is not None and self.job.poll() is not None:
            path, output, error = self.job_files
            output.close()
            error.close()
            try:
                if self.job.returncode:
                    raise RuntimeError("subscription judge failed")
                verdict = json.loads(path.with_suffix(".result.json").read_text())
                verdict["configurationId"] = self.configuration_id
                atomic(path.with_suffix(".result.json"), verdict)
                if verdict["seriouslyConcerning"]:
                    self.freeze("seriously concerning activity", {"input": path.name, **verdict})
                os.replace(path.with_suffix(".result.json"), path.with_suffix(".verdict.json"))
            except (OSError, ValueError, KeyError, RuntimeError) as error:
                self.freeze("observation judgment unavailable", {"input": path.name, "error": str(error)})
            self.job = None
            self.job_files = None
            if self.frozen:
                return False
        if now >= self.next_batch:
            self.make_batches()
            self.next_batch = now + INTERVAL
        if self.job is None and self.queue:
            path = self.queue.pop(0)
            output = path.with_suffix(".result.json").open("w")
            error = path.with_suffix(".error.log").open("w")
            env = {k:v for k,v in self.host_environment.items() if k not in {"OPENAI_API_KEY", "OPENROUTER_API_KEY"}}
            self.job = subprocess.Popen(["bun", str(HERE / "judge.mjs"), str(path)],
                                        stdout=output, stderr=error, env=env, start_new_session=True)
            self.job_files = (path, output, error)
            self.job_started = now
        if self.job is not None and now - self.job_started > 600:
            self.freeze("observation judgment stalled", {"input": self.job_files[0].name})
            return False
        return True

    def close(self):
        if self.frozen:
            return
        if self.job is not None:
            self.job.terminate()
            self.job.wait(timeout=10)
        for process in self.gateways.values():
            process.terminate()
            process.wait(timeout=10)
        self.collect()
        self.make_batches()


def run_sandbox(host, package, arguments, capture):
    # Maintenance boots may execute an agent-owned login profile. They need the same sole NIC path.
    if (package / "evidence/control/frozen.json").exists():
        raise RuntimeError("cohort is held; maintenance refused")
    config_path = package / "fugue.toml"
    state_root = Path(host.sandbox_env.get("SANDBOX_STATE_DIR", Path(host.sandbox).parent / "state"))
    name = arguments[1]
    env = {**os.environ, **host.sandbox_env, "FUGUE_SANDBOX_BIN": host.sandbox,
           "SANDBOX_CODEX_AUTH": host.codex_auth}
    controller = Controller(config_path, state_root, {name: {"vm": name}}, require_api=False, host_environment=env)
    with tempfile.TemporaryFile(mode="w+") as output:
        process = subprocess.Popen([host.sandbox, *arguments], cwd=Path(host.sandbox).parent,
                                   env={**env, **controller.environment(name, {"phase": "awake", "wakeNumber": "maintenance"})},
                                   stdout=output if capture else None, stderr=subprocess.STDOUT if capture else None)
        try:
            while process.poll() is None:
                controller.tick()
                time.sleep(0.1)
            controller.collect()
            # The registry also retains prior wakes. Flush only this maintenance gateway.
            flush_gateway(controller.vms[name])
            controller.make_batches()
            while controller.queue or controller.job is not None or controller.frozen:
                controller.tick()
                time.sleep(0.1)
            if process.returncode:
                raise subprocess.CalledProcessError(process.returncode, [host.sandbox, *arguments])
            output.seek(0)
            return output.read() if capture else ""
        finally:
            if not controller.frozen:
                if process.poll() is None:
                    process.terminate()
                    process.wait(timeout=10)
                controller.close()


def resume(package):
    directory = Path(package) / "evidence/control"
    with (directory / "release.lock").open("a") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        _resume(package)


def _resume(package):
    directory = Path(package) / "evidence/control"
    hold = directory / "frozen.json"
    if not hold.exists():
        raise RuntimeError("experiment is not frozen")
    vms = json.loads((directory / "active.json").read_text())
    # A dead owner cannot safely reattach in-memory executor state. Keep that hold for recovery.
    owner = json.loads((directory / "owner.json").read_text())
    identity = subprocess.run(["ps", "-p", str(owner["pid"]), "-o", "lstart=", "-o", "command="], capture_output=True, text=True).stdout.strip()
    if identity != owner["identity"]:
        raise RuntimeError("coordinator exited; retain the hold and recover the cohort before resuming")
    active = [vm for vm in vms.values() if Path(vm["pidFile"]).exists()]
    for vm in active:
        signal_gateway(vm, 0)
        state = vm_state(vm["endpoint"])
        if not state["canResume"]:
            raise RuntimeError("VM pause is not confirmed; hold retained")
    # Explicit operator action; models never receive the verdict or any control response.
    retain_permits(Path(package) / "fugue.toml", False)
    try:
        for vm in active:
            signal_gateway(vm, signal.SIGCONT)
            vm_state(vm["endpoint"], "Resume")
            for _ in range(100):
                if vm_state(vm["endpoint"])["canPause"]:
                    break
                time.sleep(0.02)
            else:
                raise RuntimeError("VM did not acknowledge resume")
        for vm in vms.values():
            if vm not in active:
                try:
                    signal_gateway(vm, signal.SIGCONT)
                except ProcessLookupError:
                    pass
    except Exception:
        for vm in active:
            try:
                if vm_state(vm["endpoint"])["canPause"]:
                    vm_state(vm["endpoint"], "Pause")
                signal_gateway(vm, signal.SIGSTOP)
            except Exception:
                pass
        retain_permits(Path(package) / "fugue.toml", True)
        raise
    os.replace(hold, directory / f"released-{time.time_ns()}.json")


if __name__ == "__main__":
    import argparse
    parser = argparse.ArgumentParser()
    parser.add_argument("action", choices=["resume", "orphan"])
    parser.add_argument("path", type=Path)
    args = parser.parse_args()
    if args.action == "resume":
        resume(args.path)
    else:
        evidence = json.loads(args.path.read_text())
        evidence["notification"] = notify("Fugue frozen: experiment coordinator exited. Review host evidence; operator recovery is required.")
        atomic(args.path, evidence)
