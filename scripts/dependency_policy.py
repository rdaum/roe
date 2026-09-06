#!/usr/bin/env python3
# Copyright (C) 2026 Ryan Daum <ryan.daum@gmail.com>
# SPDX-License-Identifier: GPL-3.0-only
"""Validate parsed Cargo declarations and the recorded Mica lockfile revision."""

from pathlib import Path
import re
import sys
import tomllib

MICA_GIT = "https://github.com/timbran-project/mica.git"
MICA_CRATES = ("mica-compiler", "mica-driver", "mica-external-http")
SECTIONS = ("dependencies", "dev-dependencies", "build-dependencies")


def declarations(manifest):
    for section in SECTIONS:
        for name, declaration in manifest.get(section, {}).items():
            yield section, name, declaration
    for target, manifest in manifest.get("target", {}).items():
        for section in SECTIONS:
            for name, declaration in manifest.get(section, {}).items():
                yield f"target.{target}.{section}", name, declaration


def validate(manifest, lockfile, revision, members):
    errors = []
    central = manifest.get("workspace", {}).get("dependencies", {})
    compio = central.get("compio", {})
    if not isinstance(compio, dict) or compio.get("version") != "=0.18.0":
        errors.append("Cargo.toml: set workspace.dependencies.compio.version to =0.18.0")

    for name in MICA_CRATES:
        declaration = central.get(name, {})
        if not isinstance(declaration, dict) or declaration.get("git") != MICA_GIT:
            errors.append(f"Cargo.toml: declare {name} from {MICA_GIT} in workspace.dependencies")
            continue
        if any(key in declaration for key in ("rev", "branch", "tag", "path")):
            errors.append(f"Cargo.toml: {name} uses the lockfile policy; remove manifest source selectors")
    driver = central.get("mica-driver", {})
    if not isinstance(driver, dict) or driver.get("default-features") is not False:
        errors.append("Cargo.toml: set mica-driver.default-features to false")
    if not isinstance(driver, dict) or set(driver.get("features", [])) != {"source-provider"}:
        errors.append("Cargo.toml: mica-driver.features must contain only source-provider")

    if not re.fullmatch(r"[0-9a-f]{40}", revision):
        errors.append("mica/MICA-REVISION: record one full lowercase Git commit ID")
    packages = lockfile.get("package", [])
    locked_names = {package.get("name") for package in packages}
    for name in MICA_CRATES:
        if name not in locked_names:
            errors.append(f"Cargo.lock: missing {name}; regenerate the lockfile and review its changes")
    for package in packages:
        name = package.get("name", "")
        if name.startswith("mica-") and package.get("source") != f"git+{MICA_GIT}#{revision}":
            errors.append(f"Cargo.lock: {name} must use the commit recorded in mica/MICA-REVISION")
        if name == "compio" and package.get("version") != "0.18.0":
            errors.append("Cargo.lock: compio must resolve to 0.18.0")
    if "compio" not in locked_names:
        errors.append("Cargo.lock: missing compio")

    local_manifests = {Path(path).resolve() for path in members}
    for path, member in members.items():
        for section, name, declaration in declarations(member):
            where = f"{path}: {section}.{name}"
            if isinstance(declaration, dict) and "path" in declaration:
                target = (Path(path).parent / declaration["path"] / "Cargo.toml").resolve()
                if target not in local_manifests:
                    errors.append(f"{where}: path dependencies must refer to a workspace member")
                # Workspace-local crate relationships do not declare external versions.
                if any(key in declaration for key in ("git", "registry", "version")):
                    errors.append(f"{where}: keep local path dependencies free of external source selectors")
                continue
            if not isinstance(declaration, dict) or declaration.get("workspace") is not True:
                errors.append(f"{where}: inherit the declaration with workspace = true")
                continue
            if name not in central:
                errors.append(f"{where}: add its declaration to Cargo.toml workspace.dependencies")
            if any(key in declaration for key in ("version", "git", "rev", "branch", "tag", "registry")):
                errors.append(f"{where}: move source and version declarations into workspace.dependencies")
            resolved = central.get(name, {})
            package = resolved.get("package", name) if isinstance(resolved, dict) else name
            if package == "mica-driver":
                if declaration.get("default-features") is True:
                    errors.append(f"{where}: default features must remain disabled")
                if set(declaration.get("features", [])) - {"source-provider"}:
                    errors.append(f"{where}: only the source-provider feature is permitted")
    return errors


def read_toml(path):
    with path.open("rb") as source:
        return tomllib.load(source)


def main():
    root = Path(__file__).resolve().parent.parent
    try:
        manifest = read_toml(root / "Cargo.toml")
        lockfile = read_toml(root / "Cargo.lock")
        revision = (root / "mica/MICA-REVISION").read_text().strip()
        members = {}
        for pattern in manifest.get("workspace", {}).get("members", []):
            paths = sorted(root.glob(pattern))
            if not paths:
                raise ValueError(f"workspace member pattern has no match: {pattern}")
            for path in paths:
                members[str(path.relative_to(root) / "Cargo.toml")] = read_toml(path / "Cargo.toml")
        errors = validate(manifest, lockfile, revision, members)
    except (OSError, ValueError, tomllib.TOMLDecodeError) as error:
        print(f"dependency policy: {error}", file=sys.stderr)
        return 1
    for error in errors:
        print(f"dependency policy: {error}", file=sys.stderr)
    if not errors:
        print("Dependency policy: manifest, lockfile, and Mica revision agree")
    return int(bool(errors))


if __name__ == "__main__":
    sys.exit(main())
