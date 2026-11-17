#!/usr/bin/env python3
"""Generate the symbolic, source-only native status WorkFlag catalogue.

The manifest deliberately stores only exact owner/symbol pairs. Validation is
against the pinned skyline-smash constants and an externally supplied,
fingerprinted ACMD corpus; numeric IDs are never emitted into Rust.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Iterable

REVISION = "258a566618eb5e5540df572d6cbacf48728a1b98"
LUA_CONST_SHA256 = "ac8aa7e32e87a440e8b1f8f99d53270a29a0ee2e9a9bbcd17eb8d71248e1963c"
CORPUS_FINGERPRINT = "fnv1a64:4ecb0b212de7f77f"
CORPUS_FILE_COUNT = 35_442
BOUNDARY_SENTINELS = frozenset({"FIGHTER_MURABITO_STATUS_SPECIAL_HI_FLAG_START"})

CONST_RE = re.compile(r"^\s*([A-Z][A-Z0-9_]*):\s*(0x[0-9A-Fa-f]+|[0-9]+),\s*$")
FIGHTER_KIND_RE = re.compile(r"FighterKindEntry\s*\{\s*fighter:\s*\"([^\"]+)\"\s*,\s*symbol:\s*\"([^\"]+)\"")
DIRECT_FLAG_RE = re.compile(
    r"\bWorkModule::(?:on_flag|off_flag|is_flag)\s*\(\s*"
    r"[A-Za-z_]\w*(?:\.[A-Za-z_]\w*)*\s*,\s*\*([A-Z][A-Z0-9_]*)\s*\)"
)
NON_CODE_START_RE = re.compile(r'//|/\*|\b(?:br|r)(#*)"|"|\'(?:\\.|[^\'\\])\'')


@dataclass(frozen=True, order=True)
class Pair:
    owner: str
    symbol: str


class ValidationError(ValueError):
    pass


def read_manifest(path: Path) -> list[Pair]:
    pairs: list[Pair] = []
    for line_number, raw in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
        line = raw.strip()
        if not line or line.startswith("#"):
            continue
        columns = raw.split("\t")
        if len(columns) != 2 or not all(columns):
            raise ValidationError(f"{path}:{line_number}: expected owner<TAB>symbol")
        pairs.append(Pair(*columns))
    return pairs


def parse_constants(
    path: Path,
    *,
    symbols: set[str] | None = None,
    require_pinned_hash: bool = True,
) -> dict[str, int]:
    raw = path.read_bytes()
    digest = hashlib.sha256(raw).hexdigest()
    if require_pinned_hash and digest != LUA_CONST_SHA256:
        raise ValidationError(
            f"refusing unpinned lua_const.rs sha256={digest}, expected {LUA_CONST_SHA256}"
        )
    constants: dict[str, int] = {}
    for line in raw.decode("utf-8").splitlines():
        match = CONST_RE.match(line)
        if match:
            name, literal = match.groups()
            if symbols is not None and name not in symbols:
                continue
            if name in constants:
                raise ValidationError(f"duplicate pinned lua_const symbol: {name}")
            constants[name] = int(literal, 0)
    return constants


def parse_fighter_kinds(registry_path: Path) -> dict[str, str]:
    mappings: dict[str, str] = {}
    for owner, symbol in FIGHTER_KIND_RE.findall(registry_path.read_text(encoding="utf-8")):
        if owner in mappings and mappings[owner] != symbol:
            raise ValidationError(f"ambiguous FIGHTER_KIND mapping for {owner}")
        mappings[owner] = symbol
    return mappings


def corpus_files(root: Path) -> list[tuple[str, bytes]]:
    files: list[tuple[str, bytes]] = []
    for fighter_dir in root.iterdir():
        if not fighter_dir.is_dir():
            continue
        for path in fighter_dir.glob("*.txt"):
            relative = f"{fighter_dir.name}/{path.name}"
            files.append((relative, path.read_bytes()))
    return sorted(files, key=lambda item: item[0])


def corpus_fingerprint(files: Iterable[tuple[str, bytes]]) -> tuple[str, int]:
    # Match src/move_nodes/corpus.rs: sorted relative paths, NUL, bytes, 0xff.
    value = 0xCBF29CE484222325
    byte_count = 0
    for relative, contents in files:
        for byte in relative.encode("utf-8") + b"\0" + contents + b"\xff":
            value ^= byte
            value = (value * 0x100000001B3) & 0xFFFF_FFFF_FFFF_FFFF
        byte_count += len(contents)
    return f"fnv1a64:{value:016x}", byte_count


def verify_corpus_fingerprint(root: Path, audit_path: Path) -> list[tuple[str, bytes]]:
    try:
        audit = json.loads(audit_path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise ValidationError(f"cannot read corpus audit JSON: {error}") from error
    files = corpus_files(root)
    fingerprint, byte_count = corpus_fingerprint(files)
    expected = {
        "schema": "move_node_corpus_audit_v1",
        "scope": "cached_acmd_source_subset",
        "source_fingerprint": CORPUS_FINGERPRINT,
        "source_file_count": CORPUS_FILE_COUNT,
    }
    for key, value in expected.items():
        if audit.get(key) != value:
            raise ValidationError(f"corpus audit {key} mismatch: {audit.get(key)!r}")
    if fingerprint != CORPUS_FINGERPRINT:
        raise ValidationError(
            f"source corpus fingerprint mismatch: {fingerprint}, expected {CORPUS_FINGERPRINT}"
        )
    if len(files) != CORPUS_FILE_COUNT or byte_count != audit.get("source_bytes"):
        raise ValidationError(
            f"source corpus inventory mismatch: {len(files)} files/{byte_count} bytes"
        )
    return files


def strip_comments_and_strings(source: str) -> str:
    # Rust block comments nest, and raw strings can contain both quotes and
    # comment delimiters. A flat substitution would admit calls inside them.
    chunks: list[str] = []
    cursor = 0
    while match := NON_CODE_START_RE.search(source, cursor):
        chunks.append(source[cursor:match.start()])
        token = match.group()
        end = match.end()
        if token == "//":
            newline = source.find("\n", end)
            end = len(source) if newline < 0 else newline
        elif token == "/*":
            depth = 1
            while depth:
                delimiter = re.search(r"/\*|\*/", source[end:])
                if delimiter is None:
                    end = len(source)
                    break
                end += delimiter.end()
                depth += 1 if delimiter.group() == "/*" else -1
        elif match.group(1) is not None:
            closing = '"' + match.group(1)
            closing_index = source.find(closing, end)
            end = len(source) if closing_index < 0 else closing_index + len(closing)
        elif token == '"':
            while end < len(source):
                if source[end] == "\\":
                    end += 2
                elif source[end] == '"':
                    end += 1
                    break
                else:
                    end += 1
        chunks.append(" ")
        cursor = end
    chunks.append(source[cursor:])
    return "".join(chunks)


def validate_pairs(
    pairs: list[Pair],
    constants: dict[str, int],
    fighter_kinds: dict[str, str],
    source_root: Path,
    *,
    boundary_sentinels: frozenset[str] = BOUNDARY_SENTINELS,
) -> None:
    if pairs != sorted(pairs):
        raise ValidationError("manifest pairs are not sorted by owner then symbol")
    if len(set(pairs)) != len(pairs):
        raise ValidationError("manifest contains duplicate owner/symbol pairs")

    owner_calls: dict[str, set[str]] = {}
    for pair in pairs:
        prefix = f"FIGHTER_{pair.owner.upper()}_STATUS_"
        if not pair.owner or pair.owner != pair.owner.lower() or pair.symbol != pair.symbol.upper():
            raise ValidationError(f"invalid owner/symbol spelling: {pair}")
        if not pair.symbol.startswith(prefix):
            raise ValidationError(f"wrong owner STATUS prefix for {pair.symbol}: {pair.owner}")
        if pair.symbol in boundary_sentinels:
            raise ValidationError(f"range boundary sentinel is not a WorkFlag: {pair.symbol}")
        expected_kind = f"FIGHTER_KIND_{pair.owner.upper()}"
        if fighter_kinds.get(pair.owner) != expected_kind:
            raise ValidationError(f"unknown or non-exact fighter owner mapping: {pair.owner}")
        value = constants.get(pair.symbol)
        if value is None:
            raise ValidationError(f"unknown pinned lua_const symbol: {pair.symbol}")
        # Native IDs are signed i32 values; both 0x20 and 0x21 high-nibble
        # namespaces are used for WorkFlag slots in this pinned corpus.
        if value > 0x7FFF_FFFF or value >> 28 != 0x2:
            raise ValidationError(
                f"pinned constant is outside the signed Boolean WorkFlag domain: {pair.symbol}"
            )
        owner_dir = source_root / pair.owner
        if not owner_dir.is_dir():
            raise ValidationError(f"missing owner source directory for {pair.owner}")
        if pair.owner not in owner_calls:
            calls: set[str] = set()
            for path in owner_dir.glob("*.txt"):
                source = strip_comments_and_strings(path.read_text(encoding="utf-8"))
                calls.update(DIRECT_FLAG_RE.findall(source))
            owner_calls[pair.owner] = calls
        if pair.symbol not in owner_calls[pair.owner]:
            raise ValidationError(
                f"no canonical direct WorkModule flag call for {pair.symbol} under {pair.owner}/"
            )


def render(pairs: list[Pair]) -> str:
    lines = [
        "// @generated by scripts/generate_native_status_flags.py; do not edit.",
        f"// skyline-smash {REVISION}; lua_const sha256 {LUA_CONST_SHA256}.",
        "pub(super) const ENTRIES: &[Entry] = &[",
    ]
    lines.extend(
        f'    Entry {{ fighter: "{pair.owner}", symbol: "{pair.symbol}" }},'
        for pair in pairs
    )
    lines.extend(["];"]) 
    return "\n".join(lines) + "\n"


def main(argv: list[str] | None = None) -> int:
    repo = Path(__file__).resolve().parents[1]
    parser = argparse.ArgumentParser()
    parser.add_argument("--manifest", type=Path, default=repo / "scripts/data/native_status_flags.tsv")
    parser.add_argument("--lua-const", type=Path, required=True)
    parser.add_argument("--registry", type=Path, default=repo / "src/gimmicks/registry_data.rs")
    parser.add_argument("--corpus-root", type=Path, required=True)
    parser.add_argument("--audit-json", type=Path, required=True)
    parser.add_argument("--output", type=Path, default=repo / "src/move_nodes/native_status_flags_data.rs")
    parser.add_argument("--check", action="store_true", help="compare without writing")
    args = parser.parse_args(argv)

    try:
        pairs = read_manifest(args.manifest)
        fighter_kinds = parse_fighter_kinds(args.registry)
        constants = parse_constants(
            args.lua_const,
            symbols={pair.symbol for pair in pairs}
            | {f"FIGHTER_KIND_{pair.owner.upper()}" for pair in pairs},
        )
        for pair in pairs:
            kind_symbol = f"FIGHTER_KIND_{pair.owner.upper()}"
            if kind_symbol not in constants:
                raise ValidationError(f"missing pinned fighter-kind constant: {kind_symbol}")
        verify_corpus_fingerprint(args.corpus_root, args.audit_json)
        validate_pairs(pairs, constants, fighter_kinds, args.corpus_root)
        generated = render(pairs)
        if args.check:
            current = args.output.read_text(encoding="utf-8")
            if current != generated:
                raise ValidationError(f"{args.output} is stale; regenerate without --check")
        else:
            args.output.write_text(generated, encoding="utf-8", newline="\n")
    except (OSError, UnicodeError, ValidationError) as error:
        parser.error(str(error))
    print(f"{'verified' if args.check else 'wrote'} {len(pairs)} source-only status flags")
    return 0


if __name__ == "__main__":
    sys.exit(main())
