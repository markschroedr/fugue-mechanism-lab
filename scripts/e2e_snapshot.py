#!/usr/bin/env python3
"""Real microVM snapshot regression: preserve work, omit reproducible files."""
import argparse
import json
from pathlib import Path
import subprocess
import tarfile
import uuid

parser = argparse.ArgumentParser()
parser.add_argument("--sandbox", type=Path, required=True)
parser.add_argument("--output", type=Path, required=True)
args = parser.parse_args()
name = "snapshot-smoke-" + uuid.uuid4().hex[:10]

def sandbox(*arguments):
    return subprocess.run([str(args.sandbox.resolve()), *arguments], check=True, text=True,
                          stdout=subprocess.PIPE, stderr=subprocess.STDOUT).stdout

print(sandbox("copy", "base", name), end="")
print(sandbox("size", name, "2", "1024"), end="")
script = """set -eu
cd /home/coder
mkdir -p research/target research/.venv research/node_modules sessions .local/bin .cache .pi/agent
printf '[package]\nname="smoke"\nversion="0.1.0"\n' > research/Cargo.toml
printf 'version = 4\n' > research/Cargo.lock
printf 'home = /usr/bin\n' > research/.venv/pyvenv.cfg
printf '' > research/requirements.txt
printf '{"dependencies":{}}\n' > research/package.json
printf '{}' > research/bun.lock
printf 'unique work\n' > research/untracked.py
printf 'unique tool\n' > .local/bin/my-research-tool
printf 'session evidence\n' > sessions/session.jsonl
printf 'private session settings\n' > .pi/agent/settings.json
printf 'generated binary\n' > research/target/generated
printf 'generated env\n' > research/.venv/dependency
printf 'generated package\n' > research/node_modules/dependency
printf 'cache\n' > .cache/download
[ -d .git ] || git init -q
 git -c user.name=Smoke -c user.email=smoke@localhost add research/Cargo.toml research/Cargo.lock
 git -c user.name=Smoke -c user.email=smoke@localhost commit -qm 'snapshot oracle'
"""
print(sandbox("run", name, "--", "bash", "-lc", script), end="")
args.output.mkdir(parents=True, exist_ok=True)
archive = args.output / (name + ".tar.gz")
print(sandbox("export-home", name, str(archive)), end="")
with tarfile.open(archive, "r:gz") as source:
    names = {member.name.removeprefix("./") for member in source.getmembers()}
required = {"research/untracked.py", ".local/bin/my-research-tool", "sessions/session.jsonl",
            ".pi/agent/settings.json", ".git/HEAD", "research/Cargo.lock"}
assert required <= names, f"snapshot lost unique work: {sorted(required-names)}"
excluded = {"research/target/generated", "research/.venv/dependency",
            "research/node_modules/dependency", ".cache/download"}
assert not excluded & names, f"snapshot included reproducible payload: {sorted(excluded & names)}"
print(json.dumps({"artifact": str(archive), "bytes": archive.stat().st_size,
                  "preserved": sorted(required), "omitted": sorted(excluded)}))
print("snapshot_smoke=passed environment=" + name)
print("scenario_cost={\"scenario\":\"snapshot\",\"inputTokens\":0,\"outputTokens\":0,\"costUsd\":0}")
