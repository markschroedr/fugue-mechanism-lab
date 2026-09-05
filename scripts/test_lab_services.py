#!/usr/bin/env python3
"""Static launcher contract for the host-owned Lab service boundary."""

from pathlib import Path
import os
import subprocess
import sys
import tempfile


root = Path(__file__).resolve().parents[1]
launcher = (root / "fugue").read_text()
caddyfile = (root / "deploy/society/Caddyfile").read_text()

subprocess.run(["zsh", "-n", str(root / "fugue")], check=True)
help_result = subprocess.run(
    [str(root / "fugue"), "lab-services", "--help"],
    check=True,
    capture_output=True,
    text=True,
)
assert help_result.stdout.strip() == "Usage: ./fugue lab-services <start|stop|status>"

society = launcher.split("  society-services)", 1)[1].split("  lab-services)", 1)[0]
lab = launcher.split("  lab-services)", 1)[1].split("  cargo)", 1)[0]

assert 'write_service_plist "$AGENTS_DIR/$LABEL_PREFIX.api.plist"' in society
assert "$LABEL_PREFIX.ui" not in society
assert "$LABEL_PREFIX.gateway" not in society

for service in ("api", "ui", "gateway"):
    assert f'$LABEL_PREFIX.{service}.plist' in lab
assert "lab-services requires FUGUE_HOST_CONFIG" in lab
assert "FUGUE_HOST_CONFIG='$HOST_CONFIG'" in lab
assert "FUGUE_LAB_HOST=127.0.0.1" in lab
assert "FUGUE_UI_HOST=127.0.0.1" in lab
assert "FUGUE_LAB_GATEWAY_PORT:-49324" in lab

assert "http://:{$FUGUE_LAB_GATEWAY_PORT:49324}" in caddyfile
assert "bind 127.0.0.1" in caddyfile
assert "{$FUGUE_LAB_UPSTREAM:127.0.0.1:49320}" in caddyfile
assert "{$FUGUE_LAB_UI_UPSTREAM:127.0.0.1:49323}" in caddyfile
assert "{$FUGUE_DIRECT_API_UPSTREAM:127.0.0.1:49321}" in caddyfile


def write_lines(path: Path, values: list[str]) -> None:
    path.write_text("".join(f"{value}\n" for value in values), encoding="utf-8")


def read_lines(path: Path) -> list[str]:
    return path.read_text(encoding="utf-8").splitlines() if path.exists() else []


with tempfile.TemporaryDirectory() as temporary:
    temp = Path(temporary)
    home = temp / "home"
    agents = home / "Library/LaunchAgents"
    agents.mkdir(parents=True)
    fake_bin = temp / "bin"
    fake_bin.mkdir()
    state = temp / "launchctl-state"
    calls = temp / "launchctl-calls"
    config = temp / "experiment.toml"
    config.write_text(
        """[storage]
data_dir = "data"

[society]
instance_id = "alpha"
kit_dir = "agents/kits"
sandbox_bin = ""
""",
        encoding="utf-8",
    )
    launchctl = fake_bin / "launchctl"
    launchctl.write_text(
        f"""#!{sys.executable}
import os
from pathlib import Path
import sys

state = Path(os.environ["FAKE_LAUNCHCTL_STATE"])
calls = Path(os.environ["FAKE_LAUNCHCTL_CALLS"])
labels = state.read_text(encoding="utf-8").splitlines() if state.exists() else []
command = sys.argv[1]
if command == "list":
    if os.environ.get("FAKE_LAUNCHCTL_FAIL_LIST") == "1":
        raise SystemExit(4)
    print("PID\\tStatus\\tLabel")
    for label in labels:
        print(f"-\\t0\\t{{label}}")
    raise SystemExit(0)
if command == "bootout":
    label = sys.argv[2].rsplit("/", 1)[-1]
    with calls.open("a", encoding="utf-8") as destination:
        destination.write(f"{{label}}\\n")
    if label not in labels:
        raise SystemExit(3)
    labels.remove(label)
    state.write_text("".join(f"{{item}}\\n" for item in labels), encoding="utf-8")
    raise SystemExit(0)
if command == "print":
    label = sys.argv[2].rsplit("/", 1)[-1]
    raise SystemExit(0 if label in labels else 3)
raise SystemExit(f"unexpected launchctl command: {{command}}")
""",
        encoding="utf-8",
    )
    launchctl.chmod(0o755)

    prefix = "com.fugue.society.alpha"
    target_api = f"{prefix}.api"
    target_installed = f"{prefix}.keeper-installed"
    target_orphan = f"{prefix}.keeper-orphan"
    target_legacy = f"{prefix}.ui"
    other_api = "com.fugue.society.alpha-two.api"
    other_keeper = "com.fugue.society.alpha-two.keeper-other"
    default_api = "com.fugue.society.api"
    default_keeper = "com.fugue.society.keeper-default"
    initial = [
        target_api,
        target_installed,
        target_orphan,
        target_legacy,
        other_api,
        other_keeper,
        default_api,
        default_keeper,
    ]
    write_lines(state, initial)
    for label in (
        target_api,
        target_installed,
        target_legacy,
        f"{prefix}.stale",
        other_api,
        other_keeper,
        default_api,
        default_keeper,
    ):
        (agents / f"{label}.plist").write_text(label, encoding="utf-8")

    environment = os.environ.copy()
    environment.update(
        {
            "FAKE_LAUNCHCTL_CALLS": str(calls),
            "FAKE_LAUNCHCTL_STATE": str(state),
            "FUGUE_CONFIG": str(config),
            "HOME": str(home),
            "PATH": f"{fake_bin}:{environment['PATH']}",
        }
    )

    def society_services(
        action: str, *, check: bool = True
    ) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            [str(root / "fugue"), "society-services", action],
            check=check,
            capture_output=True,
            text=True,
            env=environment,
        )

    stop_result = society_services("stop")
    assert read_lines(calls) == [
        target_installed,
        target_orphan,
    ], (stop_result.stdout, stop_result.stderr, read_lines(state))
    assert not (agents / f"{target_installed}.plist").exists()
    for label in (target_api, target_legacy, other_api, other_keeper, default_api, default_keeper):
        assert (agents / f"{label}.plist").exists()

    society_services("stop")
    assert read_lines(calls) == [
        target_installed,
        target_orphan,
    ]
    status_result = society_services("status")
    assert status_result.stdout.splitlines() == [
        f"service={target_api}",
        f"service={target_legacy}",
    ]
    verify_result = society_services("verify-stopped", check=False)
    assert verify_result.returncode == 1
    assert verify_result.stdout == status_result.stdout

    target_reappeared = f"{prefix}.keeper-reappeared"
    write_lines(state, [*read_lines(state), target_reappeared])
    (agents / f"{target_api}.plist").unlink()
    society_services("uninstall")
    assert read_lines(calls) == [
        target_installed,
        target_orphan,
        target_api,
        target_reappeared,
        target_legacy,
    ]
    assert not list(agents.glob(f"{prefix}.*.plist"))
    for label in (other_api, other_keeper, default_api, default_keeper):
        assert (agents / f"{label}.plist").exists()
        assert label in read_lines(state)

    society_services("uninstall")
    assert read_lines(calls) == [
        target_installed,
        target_orphan,
        target_api,
        target_reappeared,
        target_legacy,
    ]
    assert society_services("status").stdout.strip() == "services=none"
    verify_result = society_services("verify-stopped")
    assert verify_result.stdout.strip() == "services=none"
    environment["FAKE_LAUNCHCTL_FAIL_LIST"] = "1"
    verify_result = society_services("verify-stopped", check=False)
    assert verify_result.returncode != 0
    assert verify_result.stdout == ""

print("lab_services_static=ok")
