#!/usr/bin/env python3
"""Copy actual crate license/notice files from Cargo's resolved registry sources.
Run cargo metadata --locked --format-version 1 first, then pass its JSON path.
Missing packaged licenses are reported for explicit upstream retrieval.
"""
import json
import pathlib
import re
import shutil
import sys

root = pathlib.Path(__file__).resolve().parent.parent
packages = json.loads(pathlib.Path(sys.argv[1]).read_text())["packages"]
rows = []
missing = []
for package in sorted(packages, key=lambda p: (p["name"], p["version"])):
    if not (package.get("source") or "").startswith("registry+"):
        continue
    source = pathlib.Path(package["manifest_path"]).parent
    name = f'{package["name"]}-{package["version"]}'
    destination = root / "third_party/licenses/rust" / name
    destination.mkdir(parents=True, exist_ok=True)
    found = False
    for file in source.iterdir():
        if re.match(r"(?i)^(licen[sc]e|copying|notice|copyright|authors)([._-]|$)", file.name):
            if file.is_dir():
                shutil.copytree(file, destination / file.name, dirs_exist_ok=True)
            else:
                shutil.copy2(file, destination / file.name)
            found = True
    if not found:
        # Some crates omit workspace-level license texts from their .crate.
        # Keep existing, explicitly fetched upstream copies on regeneration.
        found = any(re.match(r"(?i)^(licen[sc]e|copying|notice|copyright)", f.name) for f in destination.iterdir())
        if not found:
            missing.append((name, package.get("repository"), package.get("license")))
    rows.append(f'| `{name}` | {package.get("license") or "See source"} | [texts]({name}/) |')
(root / "third_party/licenses/rust/DESKTOP_DEPENDENCIES.md").write_text(
    "# Desktop Cargo dependency notices\n\n"
    "Resolved by `apps/desktop-tauri/Cargo.lock`; includes build-time and target-specific crates, "
    "so this is a conservative superset of either shipped binary. Actual crate/upstream license "
    "texts are preserved in the versioned directories. No license text is synthesized.\n\n"
    "| Crate | Declared license expression | Copies |\n|---|---|---|\n" + "\n".join(rows) + "\n")
for name, repository, license_id in missing:
    print(name, repository, license_id, sep="\t")
if missing:
    sys.exit(1)
