#!/usr/bin/env python3
"""Portable home state; omit only recognized reproducible runtime payloads."""
import argparse
from pathlib import Path, PurePosixPath
import tarfile
import sys


def reproducible(path: Path, relative: PurePosixPath) -> bool:
    parts = relative.parts
    if ".git" in parts:
        return False
    root_caches = {".bun", ".cache", ".npm", ".rustup"}
    if parts and parts[0] in root_caches:
        return True
    if relative.as_posix() in {".pi/agent/auth.json", ".codex/auth.json",
                               ".local/bin/uv", ".local/bin/uvx", ".pi/agent/bin",
                               ".cargo/registry", ".cargo/git", ".codex/tmp"}:
        return True
    if path.name == "__pycache__" and path.is_dir():
        return True
    parent = path.parent
    if path.name == "target" and path.is_dir():
        return (parent / "Cargo.toml").is_file() and (parent / "Cargo.lock").is_file()
    if path.name == ".venv" and path.is_dir():
        return (path / "pyvenv.cfg").is_file() and any(
            (parent / manifest).is_file() for manifest in ("uv.lock", "requirements.txt")
        )
    if path.name == "node_modules" and path.is_dir():
        return (parent / "package.json").is_file() and any(
            (parent / lock).is_file() for lock in ("bun.lock", "bun.lockb", "package-lock.json", "yarn.lock", "pnpm-lock.yaml")
        )
    return False


class Counter:
    def __init__(self):
        self.size = 0

    def write(self, data):
        self.size += len(data)
        return len(data)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--home", type=Path, default=Path("/home/coder"))
    parser.add_argument("--measure", action="store_true")
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    if not args.measure and args.output is None:
        parser.error("--measure or --output required")
    home = args.home.resolve()

    def select(member):
        relative = PurePosixPath(member.name.removeprefix("./"))
        return None if reproducible(home.joinpath(*relative.parts), relative) else member

    counter = Counter()
    if args.measure:
        with tarfile.open(fileobj=counter, mode="w|") as archive:
            archive.add(home, arcname=".", filter=select)
        print(counter.size)
    else:
        if str(args.output) == "-":
            with tarfile.open(fileobj=sys.stdout.buffer, mode="w|gz", compresslevel=4) as archive:
                archive.add(home, arcname=".", filter=select)
        else:
            with tarfile.open(args.output, mode="w:gz", compresslevel=4) as archive:
                archive.add(home, arcname=".", filter=select)


if __name__ == "__main__":
    main()
