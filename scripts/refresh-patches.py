#!/usr/bin/env python3
"""Export committed source changes after the upstream pin as ordered patches."""

import argparse
import json
from pathlib import Path
import subprocess
import sys
import tempfile


def main() -> int:
    root = Path(__file__).resolve().parents[1]
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("source", type=Path)
    args = parser.parse_args()
    try:
        source = args.source.resolve()
        base = json.loads((root / "upstream.json").read_text())["commit"]

        def git(*arguments: str) -> str:
            return subprocess.check_output(["git", "-C", str(source), *arguments], text=True).strip()

        if git("status", "--porcelain"):
            raise ValueError("source tree must be clean before exporting patches")
        git("merge-base", "--is-ancestor", base, "HEAD")
        if git("rev-list", "--merges", f"{base}..HEAD"):
            raise ValueError("patch source commits must form a linear series")
        with tempfile.TemporaryDirectory(prefix="rclone-patches-") as temp:
            git("format-patch", "--no-signature", "--output-directory", temp, f"{base}..HEAD")
            exported = sorted(Path(temp).glob("*.patch"))
            destination = root / "patches"
            destination.mkdir(exist_ok=True)
            for old in destination.glob("*.patch"):
                old.unlink()
            for patch in exported:
                (destination / patch.name.lower()).write_bytes(patch.read_bytes())
        print(f"Exported {len(exported)} patches to {destination}")
    except (OSError, ValueError, KeyError, subprocess.CalledProcessError) as exc:
        print(f"ERROR: {exc}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
