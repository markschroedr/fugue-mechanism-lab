#!/usr/bin/env python3
"""Create, copy, and operate one-directory Fugue experiments."""

from __future__ import annotations

import argparse
import datetime as dt
import fcntl
import ipaddress
import json
import os
import re
import secrets
import shutil
import socket
import stat
import subprocess
import sys
import tarfile
import tempfile
import time
import tomllib
from contextlib import contextmanager
from pathlib import Path
from typing import Any, Literal
from urllib.parse import quote
from urllib.parse import urlsplit
from urllib.request import HTTPRedirectHandler, Request, build_opener


ROOT = Path(__file__).resolve().parent.parent
ID_RE = re.compile(r"^[a-z0-9](?:[a-z0-9-]{0,38}[a-z0-9])?$")
BINDING_KEYS = {"instanceId", "apiHost", "apiPort"}
MAX_OBSERVER_BYTES = 128 * 1024 * 1024
LIFECYCLE_MARKERS = {"paused": ".paused", "prepared": ".prepared", "failed": ".failed"}
COMPUTE_BACKINGS = ("obols", "independent")
ComputeBacking = Literal["obols", "independent"]


def load_toml(path: Path) -> dict[str, Any]:
    with path.open("rb") as source:
        return tomllib.load(source)


def resolved(path: str | Path, relative_to: Path) -> Path:
    value = Path(path).expanduser()
    return value if value.is_absolute() else (relative_to / value).resolve()


class Host:
    def __init__(self) -> None:
        self.template = Path(os.environ.get("FUGUE_CONFIG", ROOT / "fugue.toml")).resolve()
        template = load_toml(self.template)
        data_dir = resolved(template["storage"]["data_dir"], self.template.parent)
        config_path = Path(
            os.environ.get("FUGUE_HOST_CONFIG", Path.home() / ".config/fugue/host.toml")
        ).expanduser()
        config = load_toml(config_path) if config_path.is_file() else {}

        def setting(env: str, key: str, default: str | Path) -> str:
            return str(os.environ.get(env, config.get(key, default)))

        self.experiments = resolved(
            setting("FUGUE_EXPERIMENTS_DIR", "experiments_dir", data_dir.parent / "experiments"),
            config_path.parent,
        )
        self.secrets = resolved(
            setting("FUGUE_EXPERIMENT_SECRETS_DIR", "secrets_dir", Path.home() / ".fugue/secrets"),
            config_path.parent,
        )
        self.logs = resolved(
            setting("FUGUE_EXPERIMENT_LOGS_DIR", "logs_dir", Path.home() / ".fugue/logs"),
            config_path.parent,
        )
        self.gateways = resolved(
            setting(
                "FUGUE_EXPERIMENT_GATEWAYS_DIR",
                "gateways_dir",
                Path.home() / ".fugue/gateways",
            ),
            config_path.parent,
        )
        self.sandbox = setting(
            "FUGUE_SANDBOX_BIN",
            "sandbox_bin",
            template.get("society", {}).get("sandbox_bin", ""),
        )
        self.server = setting("FUGUE_SERVER_BIN", "server_bin", "")
        self.caddy = setting("FUGUE_CADDY_BIN", "caddy_bin", "")
        self.vm_bridge_host = setting("FUGUE_VM_BRIDGE_HOST", "vm_bridge_host", "10.0.0.1")

    def package(self, experiment_id: str) -> Path:
        validate_id(experiment_id)
        return self.experiments / experiment_id

    def secret(self, experiment_id: str) -> Path:
        return self.secrets / experiment_id / "substrate.key"

    def binding(self, experiment_id: str) -> Path:
        return self.gateways / experiment_id / "binding.json"

    @contextmanager
    def lock(self):
        self.experiments.mkdir(parents=True, exist_ok=True)
        with (self.experiments / ".host.lock").open("a+") as lock:
            fcntl.flock(lock, fcntl.LOCK_EX)
            yield


def validate_id(value: str) -> None:
    if not ID_RE.fullmatch(value):
        raise ValueError("experiment id must use 1-40 lowercase letters, numbers, or hyphens")
    if value == "default":
        raise ValueError("experiment id 'default' is reserved for the unscoped authoring runtime")


def validate_package_boundary_at(package: Path, experiment_id: str) -> tuple[Path, dict[str, Any]]:
    """Validate one package before its files or authority can be used."""
    if not os.path.lexists(package) or package.is_symlink() or not package.is_dir():
        raise RuntimeError(f"experiment package must be a real directory: {package}")

    pending = [package]
    while pending:
        directory = pending.pop()
        try:
            entries = list(os.scandir(directory))
        except OSError as error:
            raise RuntimeError(f"cannot inspect experiment package boundary: {directory}") from error
        for entry in entries:
            path = Path(entry.path)
            if entry.is_symlink():
                raise RuntimeError(f"experiment package must not contain symlinks: {path}")
            if entry.is_dir(follow_symlinks=False):
                pending.append(path)
            elif not entry.is_file(follow_symlinks=False):
                raise RuntimeError(f"experiment package contains an unsupported file type: {path}")

    config_path = package / "fugue.toml"
    if not config_path.is_file():
        raise RuntimeError(f"experiment is missing: {package}")
    config = load_toml(config_path)
    storage = config.get("storage")
    society = config.get("society")
    api = config.get("api")
    if not isinstance(storage, dict) or not isinstance(society, dict) or not isinstance(api, dict):
        raise RuntimeError("experiment config requires storage, api, and society tables")
    runner = society.get("runner")
    if not isinstance(runner, dict):
        raise RuntimeError("experiment config requires a society.runner table")
    if society.get("instance_id") != experiment_id:
        raise RuntimeError(f"experiment config identity does not match package {experiment_id}")

    package_root = package.resolve()
    exact_config = {
        "storage.data_dir": (storage.get("data_dir"), "data"),
        "society.kit_dir": (society.get("kit_dir"), "agents/kits"),
        "society.sandbox_bin": (society.get("sandbox_bin"), ""),
        "society.runner.wake_gate_path": (runner.get("wake_gate_path"), "../../wake-gate.sqlite"),
    }
    for label, (value, expected) in exact_config.items():
        if value != expected:
            raise RuntimeError(f"configured {label} must equal {expected!r}")

    for label, value in {
        "storage.data_dir": exact_config["storage.data_dir"][0],
        "society.kit_dir": exact_config["society.kit_dir"][0],
    }.items():
        candidate = Path(value).expanduser()
        candidate = candidate if candidate.is_absolute() else package / candidate
        try:
            candidate.resolve(strict=False).relative_to(package_root)
        except (OSError, RuntimeError, ValueError) as error:
            raise RuntimeError(f"configured {label} escapes experiment package: {value}") from error

    for label, path in {
        "data": package / "data",
        "kits": package / "agents/kits",
        "homes": package / "agents/homes",
        "evidence": package / "evidence",
    }.items():
        if not path.is_dir():
            raise RuntimeError(f"experiment {label} path must be a real directory: {path}")
    return package, config


def validate_package_boundary(host: Host, experiment_id: str) -> tuple[Path, dict[str, Any]]:
    return validate_package_boundary_at(host.package(experiment_id), experiment_id)


def package_state(package: Path) -> str:
    def marker(name: str) -> bool:
        path = package / name
        if not os.path.lexists(path):
            return False
        if path.is_symlink() or not path.is_file():
            raise RuntimeError(f"experiment lifecycle marker must be a regular file: {path}")
        return True

    present = [state for state, name in LIFECYCLE_MARKERS.items() if marker(name)]
    if len(present) > 1:
        raise RuntimeError(f"experiment has conflicting lifecycle markers: {package}")
    return present[0] if present else "active"


def begin_transition(package: Path, state: str, action: str) -> None:
    failed = package / LIFECYCLE_MARKERS["failed"]
    if state == "active":
        with failed.open("x", encoding="utf-8") as marker:
            marker.write(f"{action}\n")
            marker.flush()
            os.fsync(marker.fileno())
        sync_directory(package)
        return
    os.replace(package / LIFECYCLE_MARKERS[state], failed)
    sync_directory(package)


def finish_transition(package: Path, state: str) -> None:
    failed = package / LIFECYCLE_MARKERS["failed"]
    if state == "active":
        failed.unlink()
    else:
        os.replace(failed, package / LIFECYCLE_MARKERS[state])
    sync_directory(package)


def sync_directory(path: Path) -> None:
    descriptor = os.open(path, os.O_RDONLY)
    try:
        os.fsync(descriptor)
    finally:
        os.close(descriptor)


def toml_value(value: str | int | float) -> str:
    return json.dumps(value) if isinstance(value, str) else str(value)


def rewrite_config(path: Path, updates: dict[tuple[str, str], str | int | float]) -> None:
    section = ""
    found: set[tuple[str, str]] = set()
    output: list[str] = []
    for line in path.read_text(encoding="utf-8").splitlines():
        header = re.fullmatch(r"\s*\[([^]]+)]\s*", line)
        if header:
            section = header.group(1)
        assignment = re.match(r"^(\s*)([A-Za-z0-9_]+)\s*=", line)
        key = (section, assignment.group(2)) if assignment else None
        if key in updates:
            output.append(f"{assignment.group(1)}{key[1]} = {toml_value(updates[key])}")
            found.add(key)
        else:
            output.append(line)
    missing = updates.keys() - found
    if missing:
        names = ", ".join(f"[{section}] {key}" for section, key in sorted(missing))
        raise RuntimeError(f"config is missing required fields: {names}")
    temporary = path.parent / f".{path.name}.{os.getpid()}.{secrets.token_hex(8)}.tmp"
    try:
        with temporary.open("x", encoding="utf-8") as target:
            target.write("\n".join(output) + "\n")
            target.flush()
            os.fsync(target.fileno())
        os.replace(temporary, path)
        sync_directory(path.parent)
    finally:
        temporary.unlink(missing_ok=True)


def used_ports(host: Host, exclude_id: str | None = None) -> set[int]:
    ports: set[int] = set()
    for path in host.experiments.glob("*/fugue.toml"):
        if path.parent.name == exclude_id:
            continue
        _, config = validate_package_boundary(host, path.parent.name)
        ports.add(int(configured_binding(host, config, path.parent.name)["apiPort"]))
    return ports


def ipv4_literal(host: str, label: str) -> str:
    try:
        address = ipaddress.ip_address(host)
    except ValueError as error:
        raise RuntimeError(f"{label} must be an IPv4 address: {host}") from error
    if address.version != 4:
        raise RuntimeError(f"{label} must be an IPv4 address: {host}")
    return str(address)


def vm_bridge_host(host: Host) -> str:
    trusted = ipv4_literal(host.vm_bridge_host, "host vm_bridge_host")
    trusted_address = ipaddress.ip_address(trusted)
    private_networks = (
        ipaddress.ip_network("10.0.0.0/8"),
        ipaddress.ip_network("172.16.0.0/12"),
        ipaddress.ip_network("192.168.0.0/16"),
    )
    if trusted != "127.0.0.1" and not any(trusted_address in network for network in private_networks):
        raise RuntimeError("host vm_bridge_host must be 127.0.0.1 or an RFC1918 IPv4 address")
    return trusted


def controller_api_host(configured: str) -> str:
    actual = ipv4_literal(configured, "configured API host")
    if actual != "127.0.0.1":
        raise RuntimeError(f"configured API host must equal 127.0.0.1: {actual}")
    return actual


def bindable(host: str, port: int) -> bool:
    try:
        with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as probe:
            probe.bind((host, port))
    except OSError:
        return False
    return True


def configured_binding(host: Host, config: dict[str, Any], experiment_id: str) -> dict[str, str | int]:
    def port(section: str, key: str) -> int:
        value = config[section][key]
        if isinstance(value, bool) or not isinstance(value, int) or not 1 <= value <= 65535:
            raise RuntimeError(f"configured [{section}] {key} must be an integer from 1 to 65535")
        return value

    trusted_api_host = controller_api_host(str(config["api"]["host"]))
    trusted_vm_bridge = vm_bridge_host(host)
    api_port = port("api", "port")
    guest_url = str(config["society"]["guest_api_url"])
    parsed_guest_url = urlsplit(guest_url)
    expected_guest_url = f"http://{trusted_vm_bridge}:{api_port}"
    if (
        guest_url != expected_guest_url
        or parsed_guest_url.scheme != "http"
        or parsed_guest_url.hostname != trusted_vm_bridge
        or parsed_guest_url.port != api_port
        or parsed_guest_url.username is not None
        or parsed_guest_url.password is not None
        or parsed_guest_url.path
        or parsed_guest_url.query
        or parsed_guest_url.fragment
    ):
        raise RuntimeError(f"society.guest_api_url must equal {expected_guest_url}")
    return {
        "instanceId": experiment_id,
        "apiHost": trusted_api_host,
        "apiPort": api_port,
    }


def read_binding(path: Path) -> dict[str, str | int]:
    if path.is_symlink() or not path.is_file():
        raise RuntimeError(f"endpoint binding must be a regular non-symlink file: {path}")
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, json.JSONDecodeError) as error:
        raise RuntimeError(f"cannot read endpoint binding {path}: {error}") from error
    if not isinstance(value, dict) or set(value) != BINDING_KEYS:
        raise RuntimeError(f"endpoint binding has invalid fields: {path}")
    if not isinstance(value["instanceId"], str) or not isinstance(value["apiHost"], str):
        raise RuntimeError(f"endpoint binding has invalid identity or host: {path}")
    ipv4_literal(value["apiHost"], "endpoint binding apiHost")
    for key in ("apiPort",):
        port = value[key]
        if isinstance(port, bool) or not isinstance(port, int) or not 1 <= port <= 65535:
            raise RuntimeError(f"endpoint binding has invalid {key}: {path}")
    return value


def write_binding(host: Host, experiment_id: str, binding: dict[str, str | int]) -> None:
    path = host.binding(experiment_id)
    directory = path.parent
    if directory.is_symlink():
        raise RuntimeError(f"endpoint binding directory must not be a symlink: {directory}")
    directory.mkdir(parents=True, exist_ok=True)
    if not directory.is_dir() or directory.is_symlink():
        raise RuntimeError(f"endpoint binding directory is invalid: {directory}")
    temporary = directory / f".binding.{os.getpid()}.{secrets.token_hex(8)}.tmp"
    try:
        with temporary.open("x", encoding="utf-8") as target:
            json.dump(binding, target, separators=(",", ":"))
            target.write("\n")
            target.flush()
            os.fsync(target.fileno())
        os.replace(temporary, path)
        directory_fd = os.open(directory, os.O_RDONLY)
        try:
            os.fsync(directory_fd)
        finally:
            os.close(directory_fd)
    finally:
        temporary.unlink(missing_ok=True)


def ensure_binding(host: Host, experiment_id: str, create: bool = False) -> dict[str, str | int]:
    _, config = validate_package_boundary(host, experiment_id)
    expected = configured_binding(host, config, experiment_id)
    path = host.binding(experiment_id)
    if not path.exists():
        if not create:
            raise RuntimeError(f"endpoint binding is missing: {path}; run experiment prepare {experiment_id}")
        write_binding(host, experiment_id, expected)
        return expected
    actual = read_binding(path)
    if actual != expected:
        raise RuntimeError(
            f"endpoint binding does not match package config: {path}; "
            f"run experiment rebind {experiment_id}"
        )
    return actual


def create_binding(host: Host, experiment_id: str) -> None:
    path = host.binding(experiment_id)
    if path.exists():
        raise RuntimeError(f"endpoint binding already exists: {path}")
    _, config = validate_package_boundary(host, experiment_id)
    write_binding(host, experiment_id, configured_binding(host, config, experiment_id))


def remove_binding(host: Host, experiment_id: str) -> None:
    path = host.binding(experiment_id)
    path.unlink(missing_ok=True)
    try:
        path.parent.rmdir()
    except OSError:
        pass


class NoRedirects(HTTPRedirectHandler):
    def redirect_request(self, request, file_pointer, code, message, headers, new_url):
        return None


def fetch_json(url: str, key: str, limit: int) -> tuple[Any, int]:
    if limit <= 0:
        raise RuntimeError("observer snapshot exceeds its size limit")
    request = Request(
        url,
        headers={"accept": "application/json", "authorization": f"Bearer {key}"},
    )
    with build_opener(NoRedirects).open(request, timeout=10) as response:
        raw = response.read(limit + 1)
    if len(raw) > limit:
        raise RuntimeError("observer snapshot exceeds its size limit")
    try:
        return json.loads(raw), len(raw)
    except json.JSONDecodeError as error:
        raise RuntimeError(f"observer API returned invalid JSON: {url}") from error


def read_observer_snapshot(path: Path, experiment_id: str) -> dict[str, Any]:
    try:
        descriptor = os.open(path, os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0))
        with os.fdopen(descriptor, "rb") as source:
            if not stat.S_ISREG(os.fstat(source.fileno()).st_mode):
                raise RuntimeError(f"observer snapshot must be a regular non-symlink file: {path}")
            raw = source.read(MAX_OBSERVER_BYTES + 1)
        if len(raw) > MAX_OBSERVER_BYTES:
            raise RuntimeError(f"observer snapshot exceeds its size limit: {path}")
        snapshot = json.loads(raw)
    except (OSError, UnicodeError, json.JSONDecodeError) as error:
        raise RuntimeError(f"cannot read observer snapshot {path}: {error}") from error
    if not isinstance(snapshot, dict) or set(snapshot) != {
        "instanceId",
        "capturedAt",
        "overview",
        "agentsByAddress",
    }:
        raise RuntimeError(f"observer snapshot has an invalid envelope: {path}")
    if snapshot["instanceId"] != experiment_id:
        raise RuntimeError(f"observer snapshot identity does not match experiment {experiment_id}")
    if not isinstance(snapshot["capturedAt"], str) or not snapshot["capturedAt"]:
        raise RuntimeError(f"observer snapshot has an invalid capture time: {path}")
    if not isinstance(snapshot["overview"], dict) or not isinstance(snapshot["agentsByAddress"], dict):
        raise RuntimeError(f"observer snapshot has invalid projections: {path}")
    return snapshot


def write_observer_snapshot(path: Path, snapshot: dict[str, Any]) -> None:
    payload = (json.dumps(snapshot, separators=(",", ":"), ensure_ascii=False) + "\n").encode()
    if len(payload) > MAX_OBSERVER_BYTES:
        raise RuntimeError("observer snapshot exceeds its size limit")
    directory = path.parent
    if directory.is_symlink() or not directory.is_dir():
        raise RuntimeError(f"observer evidence directory is invalid: {directory}")
    temporary = directory / f".observer.{os.getpid()}.{secrets.token_hex(8)}.tmp"
    try:
        with temporary.open("xb") as target:
            target.write(payload)
            target.flush()
            os.fsync(target.fileno())
        os.replace(temporary, path)
        directory_fd = os.open(directory, os.O_RDONLY)
        try:
            os.fsync(directory_fd)
        finally:
            os.close(directory_fd)
    finally:
        temporary.unlink(missing_ok=True)


def capture_observer(host: Host, experiment_id: str) -> Path:
    binding = ensure_binding(host, experiment_id)
    secret_path = ensure_secret(host, experiment_id)
    try:
        key = secret_path.read_text(encoding="utf-8").strip()
    except OSError as error:
        raise RuntimeError(f"cannot read substrate authority for observer capture: {secret_path}") from error
    if not key:
        raise RuntimeError(f"substrate authority is empty: {secret_path}")
    base_url = f"http://{binding['apiHost']}:{binding['apiPort']}"
    remaining = MAX_OBSERVER_BYTES
    health, size = fetch_json(f"{base_url}/api/health", key, remaining)
    remaining -= size
    if not isinstance(health, dict) or health.get("instanceId") != experiment_id:
        raise RuntimeError(f"observer API identity does not match experiment {experiment_id}")
    overview, size = fetch_json(f"{base_url}/api/society/admin", key, remaining)
    remaining -= size
    if not isinstance(overview, dict) or not isinstance(overview.get("agents"), list):
        raise RuntimeError("observer overview has no agents list")
    agents_by_address: dict[str, Any] = {}
    for agent in overview["agents"]:
        address = agent.get("address") if isinstance(agent, dict) else None
        if not isinstance(address, str) or not address or address in agents_by_address:
            raise RuntimeError("observer overview contains an invalid or duplicate agent address")
        detail, size = fetch_json(
            f"{base_url}/api/society/admin/agents/{quote(address, safe='')}",
            key,
            remaining,
        )
        remaining -= size
        detail_address = detail.get("agent", {}).get("address") if isinstance(detail, dict) else None
        if detail_address != address:
            raise RuntimeError(f"observer detail identity does not match agent {address}")
        agents_by_address[address] = detail
    snapshot = {
        "instanceId": experiment_id,
        "capturedAt": dt.datetime.now(dt.timezone.utc).isoformat().replace("+00:00", "Z"),
        "overview": overview,
        "agentsByAddress": agents_by_address,
    }
    path = host.package(experiment_id) / "evidence/observer.json"
    write_observer_snapshot(path, snapshot)
    print(f"observer={path}")
    return path


def allocate_port(host: Host, exclude_id: str | None = None) -> int:
    occupied = used_ports(host, exclude_id)
    vm_bridge_host(host)
    for candidate in range(49325, 65536):
        if candidate not in occupied and bindable("127.0.0.1", candidate):
            return candidate
    raise RuntimeError("no experiment API port is available")


def portable_config(host: Host, target: Path, experiment_id: str) -> None:
    if not target.exists():
        shutil.copy2(host.template, target)
    api_port = allocate_port(host, experiment_id)
    trusted_vm_bridge = vm_bridge_host(host)
    rewrite_config(
        target,
        {
            ("storage", "data_dir"): "data",
            ("api", "host"): "127.0.0.1",
            ("api", "port"): api_port,
            ("society", "instance_id"): experiment_id,
            ("society", "guest_api_url"): f"http://{trusted_vm_bridge}:{api_port}",
            ("society", "kit_dir"): "agents/kits",
            ("society", "sandbox_bin"): "",
            ("society.runner", "wake_gate_path"): "../../wake-gate.sqlite",
        },
    )


def write_secret(host: Host, experiment_id: str, fresh: bool = False) -> Path:
    path = host.secret(experiment_id)
    directory = path.parent
    if os.path.lexists(directory) and directory.is_symlink():
        if not fresh:
            raise RuntimeError(f"substrate authority directory must not be a symlink: {directory}")
        directory.unlink()
    directory.mkdir(parents=True, exist_ok=True)
    flags = os.O_RDONLY | getattr(os, "O_DIRECTORY", 0) | getattr(os, "O_NOFOLLOW", 0)
    try:
        directory_fd = os.open(directory, flags)
    except OSError as error:
        raise RuntimeError(f"substrate authority directory is invalid: {directory}") from error
    try:
        try:
            existing = os.stat(path.name, dir_fd=directory_fd, follow_symlinks=False)
        except FileNotFoundError:
            existing = None
        if existing is not None:
            if fresh:
                if stat.S_ISDIR(existing.st_mode):
                    raise RuntimeError(f"substrate authority path must not be a directory: {path}")
                os.unlink(path.name, dir_fd=directory_fd)
            else:
                if stat.S_ISLNK(existing.st_mode) or not stat.S_ISREG(existing.st_mode):
                    raise RuntimeError(f"substrate authority must be a regular non-symlink file: {path}")
                return path
        descriptor = os.open(
            path.name,
            os.O_WRONLY | os.O_CREAT | os.O_EXCL | getattr(os, "O_NOFOLLOW", 0),
            0o600,
            dir_fd=directory_fd,
        )
        with os.fdopen(descriptor, "w", encoding="utf-8") as target:
            target.write(secrets.token_hex(32) + "\n")
            target.flush()
            os.fsync(target.fileno())
        os.fsync(directory_fd)
    finally:
        os.close(directory_fd)
    return path


def ensure_secret(host: Host, experiment_id: str) -> Path:
    return write_secret(host, experiment_id)


def fresh_secret(host: Host, experiment_id: str) -> Path:
    return write_secret(host, experiment_id, fresh=True)


def remove_secret(host: Host, experiment_id: str) -> None:
    path = host.secret(experiment_id)
    directory = path.parent
    if directory.is_symlink() or not directory.is_dir():
        return
    if path.is_symlink() or path.is_file():
        path.unlink()
    try:
        directory.rmdir()
    except OSError:
        pass


def service_env(host: Host, experiment_id: str, require_secret: bool = True) -> dict[str, str]:
    package = host.package(experiment_id)
    binding = ensure_binding(host, experiment_id)
    env = dict(os.environ)
    for name in (
        "FUGUE_API_HOST",
        "FUGUE_API_PORT",
        "FUGUE_API_URL",
        "FUGUE_CADDY_BIN",
        "FUGUE_CADDY_CONFIG",
        "FUGUE_DATA_DIR",
        "FUGUE_KIT_DIR",
        "FUGUE_PREVIEW_PORT",
        "FUGUE_SUBSTRATE_KEY",
        "FUGUE_SUBSTRATE_KEY_FILE",
        "FUGUE_UI_HOST",
        "FUGUE_UI_PORT",
    ):
        env.pop(name, None)
    env.update(
        FUGUE_CONFIG=str(package / "fugue.toml"),
        FUGUE_API_URL=f"http://{binding['apiHost']}:{binding['apiPort']}",
        FUGUE_SERVICE_LOG_DIR=str(host.logs / experiment_id),
    )
    secret = host.secret(experiment_id)
    if require_secret:
        ensure_secret(host, experiment_id)
    elif os.path.lexists(secret):
        ensure_secret(host, experiment_id)
    if secret.is_file():
        env["FUGUE_SUBSTRATE_KEY_FILE"] = str(secret)
    if host.sandbox:
        env["FUGUE_SANDBOX_BIN"] = host.sandbox
    if host.server:
        env["FUGUE_SERVER_BIN"] = host.server
    caddyfile = host.gateways / experiment_id / "Caddyfile"
    if host.caddy and caddyfile.is_file():
        env["FUGUE_CADDY_BIN"] = host.caddy
        env["FUGUE_CADDY_CONFIG"] = str(caddyfile)
    return env


def run_fugue(
    host: Host,
    experiment_id: str,
    *arguments: str,
    require_secret: bool = True,
    direct_secret: bool = False,
) -> None:
    env = service_env(host, experiment_id, require_secret)
    if direct_secret:
        key = host.secret(experiment_id).read_text(encoding="utf-8").strip()
        if not key:
            raise RuntimeError(f"substrate authority is empty: {host.secret(experiment_id)}")
        env["FUGUE_SUBSTRATE_KEY"] = key
    subprocess.run(
        [str(ROOT / "fugue"), *arguments],
        cwd=ROOT,
        env=env,
        check=True,
    )


def create(host: Host, experiment_id: str) -> None:
    with host.lock():
        package = host.package(experiment_id)
        if os.path.lexists(package):
            raise RuntimeError(f"experiment already exists: {package}")
        binding_created = False
        secret_created = False
        try:
            (package / "agents/kits").mkdir(parents=True)
            (package / "agents/homes").mkdir(parents=True)
            (package / "data").mkdir()
            (package / "evidence").mkdir()
            portable_config(host, package / "fugue.toml", experiment_id)
            validate_package_boundary(host, experiment_id)
            create_binding(host, experiment_id)
            binding_created = True
            fresh_secret(host, experiment_id)
            secret_created = True
            run_fugue(host, experiment_id, "init")
            (package / ".paused").write_text("paused\n", encoding="utf-8")
        except Exception:
            shutil.rmtree(package, ignore_errors=True)
            if secret_created:
                remove_secret(host, experiment_id)
            if binding_created:
                remove_binding(host, experiment_id)
            raise
    print(f"created={package}")


def copy_experiment(host: Host, source_id: str, target_id: str) -> None:
    with host.lock():
        source, _ = validate_package_boundary(host, source_id)
        target = host.package(target_id)
        if package_state(source) != "paused":
            raise RuntimeError("source experiment must be stopped before it can be copied")
        require_no_pending_births(source, "copy")
        ensure_binding(host, source_id)
        if os.path.lexists(target):
            raise RuntimeError(f"target experiment already exists: {target}")
        source_observer_path = source / "evidence/observer.json"
        source_observer = None
        if source_observer_path.exists() or source_observer_path.is_symlink():
            source_observer = read_observer_snapshot(source_observer_path, source_id)
        staging = target.parent.parent / (
            f".{target.parent.name}-{target.name}.copy.{os.getpid()}.{secrets.token_hex(8)}"
        )
        binding_created = False
        secret_created = False
        try:
            shutil.copytree(source, staging, symlinks=True)
            (staging / "Caddyfile").unlink(missing_ok=True)
            portable_config(host, staging / "fugue.toml", target_id)
            if source_observer is not None:
                target_observer = dict(source_observer)
                target_observer["instanceId"] = target_id
                write_observer_snapshot(staging / "evidence/observer.json", target_observer)
            _, target_config = validate_package_boundary_at(staging, target_id)
            binding = configured_binding(host, target_config, target_id)
            write_binding(host, target_id, binding)
            binding_created = True
            fresh_secret(host, target_id)
            secret_created = True
            staging.replace(target)
        except Exception:
            shutil.rmtree(staging, ignore_errors=True)
            shutil.rmtree(target, ignore_errors=True)
            if secret_created:
                remove_secret(host, target_id)
            if binding_created:
                remove_binding(host, target_id)
            raise
    print(f"copied={source}->{target}")


def agent_names(package: Path) -> list[str]:
    kits = package / "agents/kits"
    return (
        sorted(
            path.name
            for path in kits.iterdir()
            if not path.name.startswith(".") and path.is_dir() and not path.is_symlink()
        )
        if kits.is_dir()
        else []
    )


def pending_birth_addresses(package: Path) -> list[str]:
    kits = package / "agents/kits"
    if not kits.is_dir():
        return []
    addresses: list[str] = []
    for path in kits.iterdir():
        if not path.name.startswith(".") or not path.name.endswith(".pending"):
            continue
        address = path.name[1:-8]
        if len(address) >= 2 and ID_RE.fullmatch(address) and path.is_dir() and not path.is_symlink():
            addresses.append(address)
    return sorted(addresses)


def require_no_pending_births(package: Path, operation: str) -> None:
    addresses = pending_birth_addresses(package)
    if addresses:
        joined = ", ".join(addresses)
        raise RuntimeError(
            f"experiment has pending births: {joined}; run experiment prepare, then rerun "
            f"experiment birth before {operation}"
        )


def materialization_marker(package: Path, agent: str) -> Path:
    return package / "agents/homes/.materializing" / agent


def begin_materialization(package: Path, agent: str) -> None:
    markers = materialization_marker(package, agent).parent
    markers.mkdir(parents=True, exist_ok=True)
    sync_directory(markers.parent)
    marker = markers / agent
    if os.path.lexists(marker):
        if marker.is_symlink() or not marker.is_file():
            raise RuntimeError(f"materialization marker must be a regular file: {marker}")
        return
    with marker.open("x", encoding="utf-8") as target:
        target.write("incomplete\n")
        target.flush()
        os.fsync(target.fileno())
    sync_directory(markers)


def finish_materialization(package: Path, agent: str) -> None:
    marker = materialization_marker(package, agent)
    marker.unlink()
    sync_directory(marker.parent)


def sandbox(host: Host, *arguments: str, capture: bool = False) -> str:
    if not host.sandbox or not Path(host.sandbox).is_file():
        raise RuntimeError("host sandbox_bin is not configured")
    result = subprocess.run(
        [host.sandbox, *arguments],
        cwd=Path(host.sandbox).parent,
        check=True,
        text=True,
        capture_output=capture,
    )
    return result.stdout if capture else ""


def environments(host: Host) -> set[str]:
    return {line.split()[0] for line in sandbox(host, "list", capture=True).splitlines() if line.strip()}


def materialize(host: Host, experiment_id: str) -> None:
    package = host.package(experiment_id)
    agents = agent_names(package)
    if not agents:
        return
    config = load_toml(package / "fugue.toml")
    endpoint = urlsplit(config["society"]["guest_api_url"])
    existing = environments(host)
    for agent in agents:
        begin_materialization(package, agent)
        name = f"{experiment_id}-{agent}"
        if name in existing:
            created = False
        else:
            sandbox(host, "copy", "base", name)
            sandbox(host, "restrict-network", name, str(endpoint.hostname), str(endpoint.port))
            created = True
        archive = package / "agents/homes" / f"{agent}.tar.gz"
        if archive.is_file():
            sandbox(host, "import-home", name, str(archive))
        else:
            descriptor, temporary_name = tempfile.mkstemp(
                prefix=f"fugue-{experiment_id}-{agent}-",
                suffix=".tar.gz",
            )
            os.close(descriptor)
            temporary = Path(temporary_name)
            try:
                with tarfile.open(temporary, "w:gz") as coder_home:
                    coder_home.add(package / "agents/kits" / agent, arcname="society")
                sandbox(host, "import-home", name, str(temporary))
            finally:
                temporary.unlink(missing_ok=True)
        sandbox(host, "sync-openai-auth", name)
        sandbox(host, "setup-subagents", name)
        finish_materialization(package, agent)
        print(f"{'materialized' if created else 'restored'}={name}")


def wait_for_vms_quiescent(host: Host, experiment_id: str, timeout_seconds: float = 20) -> None:
    agents = agent_names(host.package(experiment_id))
    if not agents:
        return
    if not host.sandbox:
        raise RuntimeError("host sandbox_bin is not configured")
    state = Path(host.sandbox).parent / "state"
    pidfiles = {
        f"{experiment_id}-{agent}": state / f"{experiment_id}-{agent}.pid"
        for agent in agents
    }
    deadline = time.monotonic() + timeout_seconds
    while True:
        running: list[str] = []
        for name, pidfile in pidfiles.items():
            if not pidfile.exists():
                continue
            try:
                pid_text = pidfile.read_text(encoding="utf-8").strip()
            except FileNotFoundError:
                continue
            except OSError as error:
                raise RuntimeError(f"cannot inspect environment state: {name}") from error
            try:
                pid = int(pid_text)
            except ValueError:
                running.append(name)
                continue
            if pid <= 0:
                running.append(name)
                continue
            try:
                os.kill(pid, 0)
            except ProcessLookupError:
                continue
            except PermissionError:
                running.append(name)
            except OSError as error:
                raise RuntimeError(f"cannot inspect environment process: {name}") from error
            else:
                running.append(name)
        if not running:
            return
        if time.monotonic() >= deadline:
            raise RuntimeError(f"environments did not stop: {', '.join(running)}")
        time.sleep(0.1)


def export_homes(host: Host, experiment_id: str) -> None:
    package = host.package(experiment_id)
    agents = agent_names(package)
    if not agents:
        return
    existing = environments(host)
    homes = package / "agents/homes"
    homes.mkdir(parents=True, exist_ok=True)
    for agent in agents:
        if materialization_marker(package, agent).is_file():
            print(f"skipped-incomplete={experiment_id}-{agent}")
            continue
        name = f"{experiment_id}-{agent}"
        if name not in existing:
            continue
        destination = homes / f"{agent}.tar.gz"
        temporary = homes / f".{agent}.tar.gz"
        temporary.unlink(missing_ok=True)
        sandbox(host, "export-home", name, str(temporary))
        temporary.replace(destination)
        print(f"exported={destination}")


def operate(host: Host, experiment_id: str, action: str) -> None:
    with host.lock():
        package, _ = validate_package_boundary(host, experiment_id)
        state = package_state(package)
        if action == "prepare":
            if state != "paused":
                raise RuntimeError("experiment must be paused before it can be prepared")
            ensure_binding(host, experiment_id, create=True)
            begin_transition(package, state, action)
            materialize(host, experiment_id)
            run_fugue(host, experiment_id, "society-services", "prepare")
            finish_transition(package, "prepared")
        elif action == "start":
            if state not in {"paused", "prepared"}:
                raise RuntimeError("experiment must be paused or prepared before it can be started")
            require_no_pending_births(package, "start")
            ensure_binding(host, experiment_id)
            begin_transition(package, state, action)
            if state == "paused":
                materialize(host, experiment_id)
                run_fugue(host, experiment_id, "society-services", "install")
            else:
                run_fugue(host, experiment_id, "society-services", "start")
            finish_transition(package, "active")
        elif action == "stop":
            if state == "paused":
                raise RuntimeError("experiment is already paused")
            if state == "failed":
                raise RuntimeError("experiment has failed; run experiment recover before another action")
            ensure_binding(host, experiment_id)
            begin_transition(package, state, action)
            run_fugue(host, experiment_id, "society-services", "stop")
            wait_for_vms_quiescent(host, experiment_id)
            capture_observer(host, experiment_id)
            export_homes(host, experiment_id)
            run_fugue(host, experiment_id, "society-services", "uninstall")
            finish_transition(package, "paused")
        else:
            raise AssertionError(action)


def recover(host: Host, experiment_id: str) -> None:
    with host.lock():
        package, _ = validate_package_boundary(host, experiment_id)
        if package_state(package) != "failed":
            raise RuntimeError("experiment recovery requires failed state")
        ensure_binding(host, experiment_id)
        run_fugue(host, experiment_id, "society-services", "stop")
        wait_for_vms_quiescent(host, experiment_id)
        try:
            capture_observer(host, experiment_id)
        except (OSError, RuntimeError) as error:
            print(f"observer warning: recovery could not refresh snapshot: {error}", file=sys.stderr)
        export_homes(host, experiment_id)
        run_fugue(host, experiment_id, "society-services", "uninstall")
        run_fugue(host, experiment_id, "society-services", "verify-stopped")
        wait_for_vms_quiescent(host, experiment_id)
        finish_transition(package, "paused")
    print(f"recovered={package}")


def rebind(host: Host, experiment_id: str) -> None:
    with host.lock():
        package, config = validate_package_boundary(host, experiment_id)
        if package_state(package) != "paused":
            raise RuntimeError("experiment must be stopped before it can be rebound")
        binding = configured_binding(host, config, experiment_id)
        port = int(binding["apiPort"])
        if port in used_ports(host, exclude_id=experiment_id):
            raise RuntimeError("configured experiment API port conflicts with another package")
        if not bindable("127.0.0.1", port):
            raise RuntimeError("configured experiment API port is not available")
        write_binding(host, experiment_id, binding)
    print(f"rebound={host.binding(experiment_id)}")


def configure_compute(
    host: Host,
    experiment_id: str,
    compute_backing: ComputeBacking,
) -> None:
    if compute_backing not in COMPUTE_BACKINGS:
        raise ValueError("compute backing must be 'obols' or 'independent'")
    with host.lock():
        package, _ = validate_package_boundary(host, experiment_id)
        if package_state(package) != "paused":
            raise RuntimeError("experiment must be paused before compute backing can be configured")
        config_path = package / "fugue.toml"
        rewrite_config(config_path, {("society", "compute_backing"): compute_backing})
        validate_package_boundary(host, experiment_id)
    print(f"configured={package} compute_backing={compute_backing}")


def birth(host: Host, experiment_id: str, name: str, role: str | None) -> None:
    with host.lock():
        package, _ = validate_package_boundary(host, experiment_id)
        if package_state(package) != "prepared":
            raise RuntimeError("experiment must be prepared before an agent can be born")
        begin_transition(package, "prepared", "birth")
        arguments = ["society-birth", name]
        if role is not None:
            arguments.extend(("--role", role))
        run_fugue(host, experiment_id, *arguments, direct_secret=True)
        materialize(host, experiment_id)
        finish_transition(package, "prepared")


def status(host: Host, experiment_id: str | None) -> None:
    ids = [experiment_id] if experiment_id else sorted(
        path.parent.name for path in host.experiments.glob("*/fugue.toml")
    )
    for item in ids:
        package, _ = validate_package_boundary(host, item)
        state = package_state(package)
        pending = pending_birth_addresses(package)
        pending_field = f" pending_births={','.join(pending)}" if pending else ""
        print(f"experiment={item} state={state}{pending_field} package={package}")
        run_fugue(host, item, "society-services", "status", require_secret=False)


def society(host: Host, experiment_id: str, arguments: list[str]) -> None:
    validate_package_boundary(host, experiment_id)
    run_fugue(host, experiment_id, "society", *arguments)


def parser() -> argparse.ArgumentParser:
    result = argparse.ArgumentParser(prog="./fugue experiment")
    commands = result.add_subparsers(dest="command", required=True)
    create_parser = commands.add_parser("create")
    create_parser.add_argument("experiment_id")
    copy_parser = commands.add_parser("copy")
    copy_parser.add_argument("source_id")
    copy_parser.add_argument("target_id")
    configure_compute_parser = commands.add_parser("configure-compute")
    configure_compute_parser.add_argument("experiment_id")
    configure_compute_parser.add_argument("compute_backing", choices=COMPUTE_BACKINGS)
    birth_parser = commands.add_parser("birth")
    birth_parser.add_argument("experiment_id")
    birth_parser.add_argument("name")
    birth_parser.add_argument("--role")
    for name in ("prepare", "start", "stop", "recover", "rebind"):
        action = commands.add_parser(name)
        action.add_argument("experiment_id")
    status_parser = commands.add_parser("status")
    status_parser.add_argument("experiment_id", nargs="?")
    society_parser = commands.add_parser("society", add_help=False)
    society_parser.add_argument("experiment_id")
    society_parser.add_argument("arguments", nargs=argparse.REMAINDER)
    return result


def main() -> int:
    try:
        args = parser().parse_args()
        host = Host()
        if args.command == "create":
            create(host, args.experiment_id)
        elif args.command == "copy":
            copy_experiment(host, args.source_id, args.target_id)
        elif args.command in {"prepare", "start", "stop"}:
            operate(host, args.experiment_id, args.command)
        elif args.command == "recover":
            recover(host, args.experiment_id)
        elif args.command == "rebind":
            rebind(host, args.experiment_id)
        elif args.command == "configure-compute":
            configure_compute(host, args.experiment_id, args.compute_backing)
        elif args.command == "birth":
            birth(host, args.experiment_id, args.name, args.role)
        elif args.command == "society":
            society(host, args.experiment_id, args.arguments)
        else:
            status(host, args.experiment_id)
        return 0
    except (OSError, RuntimeError, ValueError, subprocess.CalledProcessError) as error:
        print(f"experiment error: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
