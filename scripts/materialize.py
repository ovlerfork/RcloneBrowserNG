#!/usr/bin/env python3
"""Fetch the pinned upstream commit and apply the ordered source patches."""

import argparse
import json
from pathlib import Path
import re
import subprocess
import sys


def main() -> int:
    root = Path(__file__).resolve().parents[1]
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("target", type=Path)
    parser.add_argument("--revision", help="full upstream commit to replay instead of the pin")
    args = parser.parse_args()
    try:
        pin = json.loads((root / "upstream.json").read_text())
        revision = args.revision or pin["commit"]
        if not re.fullmatch(r"[0-9a-f]{40}", revision):
            raise ValueError("the upstream revision must be a full commit SHA")
        target = args.target.absolute()
        if target.exists() or target.is_symlink():
            raise ValueError(f"target already exists: {target}")
        target.mkdir(parents=True)

        def git(*arguments: str) -> str:
            return subprocess.check_output(
                ["git", "-C", str(target), "-c", "user.name=Patch Set Builder",
                 "-c", "user.email=patchset@localhost", *arguments], text=True,
            ).strip()

        git("init", "--quiet")
        git("remote", "add", "origin", pin["repository"])
        if revision != pin["commit"]:
            git("fetch", "--depth", "1", "origin", pin["commit"])
        git("fetch", "--depth", "1", "origin", revision)
        git("checkout", "--detach", "FETCH_HEAD")
        if git("rev-parse", "HEAD") != revision:
            raise ValueError("fetched upstream does not match the requested commit")
        patches = sorted((root / "patches").glob("*.patch"))
        if patches:
            git("am", "--3way", "--keep-cr", *(str(patch) for patch in patches))
        print(f"Materialized {revision} with {len(patches)} patches at {target}")
    except (OSError, ValueError, KeyError, subprocess.CalledProcessError) as exc:
        print(f"ERROR: {exc}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
