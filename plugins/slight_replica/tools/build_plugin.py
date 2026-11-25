#!/usr/bin/env python3
"""Build slight_replica on Windows, Linux, or macOS."""

from __future__ import annotations

import argparse
import os
import re
import shutil
import subprocess
from pathlib import Path


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "cargo_args",
        nargs=argparse.REMAINDER,
        help="additional arguments passed to cargo skyline build",
    )
    args = parser.parse_args()

    plugin_root = Path(__file__).resolve().parents[1]
    environment = os.environ.copy()
    target_value = environment.get("CARGO_TARGET_DIR")
    target_dir = Path(target_value) if target_value else plugin_root / "target"
    if not target_dir.is_absolute():
        target_dir = plugin_root / target_dir
    environment["CARGO_TARGET_DIR"] = str(target_dir)

    gc_flag = "-C link-arg=-Wl,--gc-sections"
    rustflags = environment.get("RUSTFLAGS", "").strip()
    if gc_flag not in rustflags:
        environment["RUSTFLAGS"] = f"{rustflags} {gc_flag}".strip()

    command = ["cargo", "skyline", "build", "--release", *args.cargo_args]
    subprocess.run(command, cwd=plugin_root, env=environment, check=True)

    nro = target_dir / "aarch64-skyline-switch" / "release" / "lib_effect_viewer.nro"
    if not nro.is_file():
        raise SystemExit(f"Build completed but the plugin was not found at {nro}")
    output = target_dir / "output"
    output.mkdir(parents=True, exist_ok=True)
    shutil.copy2(nro, output / nro.name)
    print(f"Built: {nro} ({nro.stat().st_size} bytes)")

    # The source an exported mod's own plugin is built from: only the files the mod flavour
    # compiles, as one crate with nothing outside it.
    mod_dir = target_dir / "mod_plugin"
    mod_env = dict(environment, CARGO_TARGET_DIR=str(mod_dir))
    subprocess.run(
        [*command, "--features", "mod_plugin"], cwd=plugin_root, env=mod_env, check=True
    )
    dep_info = mod_dir / "aarch64-skyline-switch" / "release" / "deps" / "_effect_viewer.d"
    write_mod_template(plugin_root, dep_info, output / "plugin-source")
    print(f"Wrote: {output / 'plugin-source'}")


def write_mod_template(plugin_root: Path, dep_info: Path, dest: Path) -> None:
    """Copy the files `dep_info` lists into `dest`. Files outside the crate move under
    src/shared/ and the paths that name them are rewritten to match."""
    first = dep_info.read_text().split("\n", 1)[0]
    listed = [part for part in first.split(":", 1)[1].split() if part.endswith(".rs")]
    crate = plugin_root.resolve()
    sources = {}
    for part in listed:
        path = (plugin_root / part).resolve()
        try:
            new = path.relative_to(crate)
        except ValueError:
            new = Path("src/shared") / path.relative_to(crate.parents[1] / "src")
        sources[path] = new
    if dest.exists():
        shutil.rmtree(dest)
    for path, new in sources.items():
        text = path.read_text()
        for literal in set(re.findall(r'"([^"\n]+\.rs)"', text)):
            target = (path.parent / literal).resolve()
            if target in sources and not literal.startswith(("/", "src")):
                moved = os.path.relpath(dest / sources[target], (dest / new).parent)
                if moved != literal:
                    text = text.replace(f'"{literal}"', f'"{moved}"')
        out = dest / new
        out.parent.mkdir(parents=True, exist_ok=True)
        out.write_text(text)
    cargo = (plugin_root / "Cargo.toml").read_text()
    cargo = cargo.replace("[features]\n", '[features]\ndefault = ["mod_plugin"]\n', 1)
    (dest / "Cargo.toml").write_text(cargo)
    shutil.copy2(plugin_root / "Cargo.lock", dest / "Cargo.lock")


if __name__ == "__main__":
    main()
