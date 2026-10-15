"""Host-only world bundles. Only the honest world ships in public source."""
from __future__ import annotations
import hashlib
import json
import os
import re
import shutil
from pathlib import Path


def world_directory(root: Path, society: dict, config_dir: Path | None = None) -> Path:
    selector = society.get("world", "honest")
    if selector == "honest":
        return root / "resources/society"
    directory = Path(selector)
    base = config_dir or Path(os.environ.get("FUGUE_CONFIG", root / "fugue.toml")).resolve().parent
    return directory if directory.is_absolute() else base / directory


def definition(root: Path, society: dict, config_dir: Path | None = None) -> dict:
    directory = world_directory(root, society, config_dir)
    filename = "worlds/honest.json" if society.get("world", "honest") == "honest" else "world.json"
    world = json.loads((directory / filename).read_text(encoding="utf-8"))
    if not world.get("id") or not isinstance(world.get("features"), dict):
        raise ValueError("world requires id and features")
    return world


def resource_files(world: dict) -> set[str]:
    files = {"SPAWN.md", "SKILL-society-node.md", *world.get("guestFiles", [])}
    files.update(world[field] for field in ("wakeRenderer", "scorer", "supportFile") if world.get(field))
    if any(not isinstance(name, str) or Path(name).name != name or name.startswith(".") for name in files):
        raise ValueError("world resources must use simple filenames")
    return files


def snapshot_world(root: Path, society: dict, package: Path) -> str | None:
    """Vendor only declared resources into the package, making paths machine-independent."""
    if society.get("world", "honest") == "honest":
        return None
    directory = world_directory(root, society, package)
    world = definition(root, society, package)
    files = resource_files(world) | {"world.json"}
    contents = {}
    for name in sorted(files):
        source = directory / name
        if source.is_symlink() or not source.is_file():
            raise ValueError(f"world resource must be a regular file: {source}")
        contents[name] = source.read_bytes()
    digest = hashlib.sha256()
    for name, content in contents.items():
        digest.update(name.encode() + b"\0" + content + b"\0")
    relative = Path("worlds") / digest.hexdigest()
    target = package / relative
    target.mkdir(parents=True, exist_ok=True)
    for name, content in contents.items():
        output = target / name
        if output.exists():
            if output.is_symlink() or output.read_bytes() != content:
                raise RuntimeError("world snapshot content mismatch")
        else:
            with output.open("xb") as handle:
                handle.write(content)
    return relative.as_posix()


def refresh_kit(root: Path, kit: Path, society: dict, *, name: str | None = None, role: str | None = None, force: bool = False, config_dir: Path | None = None) -> bool:
    world = definition(root, society, config_dir)
    marker = kit / "onboarding.json"
    old = json.loads(marker.read_text(encoding="utf-8")) if marker.exists() else {}
    if not force and not old and society.get("world", "honest") == "honest":
        return False  # Preserve historical custom honest kits.
    node = json.loads((kit / "node.json").read_text(encoding="utf-8"))
    address = node["address"]
    old_spawn = (kit / "SPAWN.md").read_text(encoding="utf-8") if (kit / "SPAWN.md").exists() else ""
    if name is None:
        identity = re.search(r"Your name is `([^`]+)`", old_spawn)
        name = old.get("name", identity.group(1) if identity else address)
    if role is None:
        legacy_role = re.search(r"\n## Role\n\n(.*?)(?=\n## |\Z)", old_spawn, re.S)
        role = old.get("role", legacy_role.group(1).strip() if legacy_role else "")
    directory = world_directory(root, society, config_dir)
    sources = {"runner.py": root / "scripts/robinson_runner.py", "society_node.py": root / "scripts/society_node.py", "sleep.ts": root / "scripts/robinson_sleep.ts"}
    for filename in ("SPAWN.md", "SKILL-society-node.md", *world.get("guestFiles", [])):
        if filename not in resource_files(world):
            raise ValueError("invalid guest resource")
        sources[filename] = directory / filename
    if world.get("wakeRenderer"):
        sources["world-wake.py"] = directory / world["wakeRenderer"]
    inputs = {"world": world["id"], "version": world["version"], "name": name, "role": role,
              "mission": society.get("mission", "You are here. Choose what to do. Stay active."),
              "models": society["models"], "runner": society.get("runner", {}), "guestApiUrl": society.get("guest_api_url"),
              "sources": {key: hashlib.sha256(path.read_bytes()).hexdigest() for key, path in sources.items()}}
    if not force and old == inputs:
        return False
    replacements = {"{NAME}": name, "{ADDRESS}": address, "{MISSION}": inputs["mission"].strip(),
        "{ROLE_SECTION}": f"\n## Role\n\n{role.strip()}\n" if role else "", "{WORK_POINTERS}": "none yet.",
        "{LIVE_SPEND_FILE}": "~/.robinson-live-spend.json", "{RATES_FILE}": "~/society/rates.json", "{NOTE_FILE}": "~/note-to-self.md"}
    spawn = sources.pop("SPAWN.md").read_text(encoding="utf-8")
    for key, value in replacements.items():
        spawn = spawn.replace(key, value)
    if re.search(r"\{[A-Z_]+\}", spawn):
        raise RuntimeError("unresolved world spawn placeholder")
    for filename, source in sources.items():
        if source.is_symlink() or not source.is_file():
            raise ValueError("world resource must be a regular file")
        shutil.copyfile(source, kit / filename)
    # Tell the runner which renderer to use; old guest files can remain as historical work.
    node["worldWakeRenderer"] = "world-wake.py" if world.get("wakeRenderer") else None
    for filename in ("runner.py", "society_node.py"):
        (kit / filename).chmod(0o755)
    (kit / "SPAWN.md").write_text(spawn, encoding="utf-8")
    node["apiBaseUrl"] = society.get("guest_api_url", node["apiBaseUrl"])
    for key, value in society.get("runner", {}).items():
        camel = re.sub(r"_([a-z])", lambda match: match.group(1).upper(), key)
        if camel in node["runner"]:
            node["runner"][camel] = value
    (kit / "node.json").write_text(json.dumps(node, sort_keys=True) + "\n", encoding="utf-8")
    (kit / "rates.json").write_text(json.dumps({"obolUsd": 0.01, "models": society["models"]}) + "\n", encoding="utf-8")
    marker.write_text(json.dumps(inputs, sort_keys=True) + "\n", encoding="utf-8")
    return True
