#!/usr/bin/env python3
"""Puts a version (a tag like v1.2.3, or 1.2.3) into package.json, tauri.conf.json and Cargo.toml."""
import json
import re
import sys
from pathlib import Path

if len(sys.argv) != 2:
    sys.exit("usage: set-version.py v1.2.3")
version = sys.argv[1].removeprefix("v")
if not re.fullmatch(r"\d+\.\d+\.\d+([-+][0-9A-Za-z.-]+)?", version):
    sys.exit(f"not a version: {version}")

root = Path(__file__).resolve().parent.parent
for name in ("package.json", "src-tauri/tauri.conf.json"):
    path = root / name
    data = json.loads(path.read_text())
    data["version"] = version
    path.write_text(json.dumps(data, indent=2, ensure_ascii=False) + "\n")

cargo = root / "src-tauri" / "Cargo.toml"
text = cargo.read_text()
text, count = re.subn(r'(?m)^version = "[^"]*"', f'version = "{version}"', text, count=1)
if count != 1:
    sys.exit("no version in Cargo.toml")
cargo.write_text(text)
print(f"version {version}")
