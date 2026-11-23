#!/usr/bin/env python3
"""Generate Visionary's safe gimmick identifier registry from skyline-smash.

The input must be the lua_const.rs from the revision pinned below. Numeric
aliases in a writable fighter/weapon namespace are omitted rather than chosen
arbitrarily. A small reviewed set of shared fighter statuses is retained in a
separate read-only namespace for exact status-entry triggers.
"""

from __future__ import annotations

import argparse
import hashlib
import re
from collections import defaultdict
from dataclasses import dataclass
from pathlib import Path

REVISION = "258a566618eb5e5540df572d6cbacf48728a1b98"
SOURCE_SHA256 = "ac8aa7e32e87a440e8b1f8f99d53270a29a0ee2e9a9bbcd17eb8d71248e1963c"
CONST_RE = re.compile(r"^\s*([A-Z][A-Z0-9_]*):\s*(0x[0-9A-Fa-f]+|[0-9]+),\s*$")
WORK_RE = re.compile(r"^FIGHTER_([A-Z0-9]+)_INSTANCE_WORK_ID_(FLOAT|INT|FLAG)_(.+)$")
ARTICLE_RE = re.compile(r"^FIGHTER_([A-Z0-9]+)_GENERATE_ARTICLE_(.+)$")
STATUS_RE = re.compile(r"^WEAPON_([A-Z0-9]+)_([A-Z0-9_]+)_STATUS_KIND_([A-Z0-9_]+)$")
# Only this three-part spelling can enter the fighter-owned status namespace:
# it carries the owning fighter in the symbol itself, then is checked below
# against that fighter's *direct* FIGHTER_KIND_* spelling. This excludes helper
# owners such as POKEMON/MOVIE. Shared statuses use the separate exact-name
# allowlist below.
FIGHTER_STATUS_RE = re.compile(r"^FIGHTER_([A-Z0-9]+)_STATUS_KIND_([A-Z0-9_]+)$")
# These spellings are range/count sentinels, not statuses accepted by
# `StatusModule::status_kind`. Keeping one would let a future exact-edge
# binding select an enum boundary rather than a fighter move.
FIGHTER_STATUS_SENTINELS = frozenset({"NONE", "NUM", "PREV", "MAX"})
# Shared statuses are admitted only from this reviewed exact-name set. The
# names and values still have to appear in the pinned lua_const.rs input.
# There is no `FIGHTER_STATUS_KIND_GUARD_WAIT` in that source; `GUARD` is the
# exact common guard status spelling.
COMMON_FIGHTER_STATUSES = frozenset(
    """
    WAIT WALK WALK_BRAKE DASH TURN TURN_DASH RUN RUN_BRAKE TURN_RUN TURN_RUN_BRAKE
    SQUAT SQUAT_WAIT SQUAT_F SQUAT_B SQUAT_RV
    JUMP_SQUAT JUMP JUMP_AERIAL FALL FALL_AERIAL FALL_SPECIAL WALL_JUMP PASS
    LANDING LANDING_LIGHT LANDING_ATTACK_AIR LANDING_FALL_SPECIAL
    ATTACK ATTACK_100 ATTACK_DASH ATTACK_S3 ATTACK_HI3 ATTACK_LW3
    ATTACK_S4_START ATTACK_S4_HOLD ATTACK_S4 ATTACK_HI4_START ATTACK_HI4_HOLD ATTACK_HI4
    ATTACK_LW4_START ATTACK_LW4_HOLD ATTACK_LW4 ATTACK_AIR
    SPECIAL_N SPECIAL_S SPECIAL_HI SPECIAL_LW
    GUARD_ON GUARD GUARD_OFF GUARD_DAMAGE ESCAPE ESCAPE_F ESCAPE_B ESCAPE_AIR ESCAPE_AIR_SLIDE
    CATCH CATCH_DASH CATCH_TURN CATCH_PULL CATCH_DASH_PULL CATCH_WAIT CATCH_ATTACK CATCH_CUT THROW
    CLIFF_CATCH_MOVE CLIFF_CATCH CLIFF_WAIT CLIFF_ATTACK CLIFF_CLIMB CLIFF_ESCAPE
    CLIFF_JUMP1 CLIFF_JUMP2 CLIFF_JUMP3
    PASSIVE PASSIVE_FB PASSIVE_WALL PASSIVE_WALL_JUMP PASSIVE_CEIL
    DOWN DOWN_WAIT DOWN_STAND DOWN_STAND_FB DOWN_STAND_ATTACK
    DAMAGE DAMAGE_AIR DAMAGE_FLY DAMAGE_FLY_ROLL DAMAGE_FALL
    APPEAL SMASH_APPEAL GLIDE_START GLIDE GLIDE_ATTACK GLIDE_LANDING GLIDE_END
    ATTACH_WALL ATTACH_WALL_WAIT DETACH_WALL DETACH_WALL_JUMP DETACH_WALL_FALL
    """.split()
)
# Common statuses apply to every supported fighter, so they are emitted once
# under this owner instead of being repeated for each fighter.
COMMON_FIGHTER = "*"
COMMON_FIGHTER_STATUS_PREFIX = "FIGHTER_STATUS_KIND_"
WEAPON_KIND_RE = re.compile(r"^WEAPON_KIND_([A-Z0-9]+)_([A-Z0-9_]+)$")
FIGHTER_KIND_RE = re.compile(r"^FIGHTER_KIND_([A-Z0-9_]+)$")
# Items a fighter can be handed with ItemModule::have_item. Range markers and
# the FOR_DUET/LOT alias blocks are not item kinds.
ITEM_KIND_RE = re.compile(r"^ITEM_KIND_(?!NONE$|FOR_DUET_|LOT_)[A-Z0-9_]+(?<!_START)$")
WORK_KINDS = frozenset({"work_float", "work_int", "work_flag"})


@dataclass(frozen=True, order=True)
class Entry:
    fighter: str
    group: str
    kind: str
    name: str
    value: int


def is_work_boundary_name(name: str) -> bool:
    """Whether an exact Work ID spelling denotes its range/count boundary."""
    match = WORK_RE.match(name)
    if not match:
        return False
    suffix = match.group(3)
    return suffix in {"START", "TERM"} or suffix.endswith(("_START", "_TERM"))


def remove_value_collisions(entries: list[Entry]) -> tuple[list[Entry], int]:
    """Omit every alias in a consuming namespace after candidate filtering."""
    by_value: dict[tuple[str, str, str, int], list[Entry]] = defaultdict(list)
    for entry in entries:
        by_value[(entry.fighter, entry.group, entry.kind, entry.value)].append(entry)
    collisions = {
        entry for group in by_value.values() if len(group) > 1 for entry in group
    }
    return sorted(set(entries) - collisions), len(collisions)


def parse(path: Path) -> tuple[
    list[Entry], list[tuple[str, str, int]], list[tuple[str, str, int]], int
]:
    raw = path.read_bytes()
    digest = hashlib.sha256(raw).hexdigest()
    if digest != SOURCE_SHA256:
        raise SystemExit(
            f"refusing unpinned lua_const.rs: sha256 is {digest}, expected {SOURCE_SHA256}"
        )
    found: list[Entry] = []
    fighter_status_candidates: list[Entry] = []
    common_fighter_statuses: dict[str, int] = {}
    weapon_kinds: dict[tuple[str, str], int] = {}
    fighter_kinds: dict[str, tuple[str, int]] = {}
    names: set[str] = set()
    for line in raw.decode("utf-8").splitlines():
        match = CONST_RE.match(line)
        if not match:
            continue
        name, literal = match.groups()
        value = int(literal, 0)
        work = WORK_RE.match(name)
        article = ARTICLE_RE.match(name)
        status = STATUS_RE.match(name)
        fighter_status = FIGHTER_STATUS_RE.match(name)
        weapon_kind = WEAPON_KIND_RE.match(name)
        fighter_kind = FIGHTER_KIND_RE.match(name)
        if name.startswith(COMMON_FIGHTER_STATUS_PREFIX):
            suffix = name.removeprefix(COMMON_FIGHTER_STATUS_PREFIX)
            if suffix in COMMON_FIGHTER_STATUSES:
                if name in common_fighter_statuses:
                    raise SystemExit(f"duplicate common fighter status: {name}")
                common_fighter_statuses[name] = value
        entry = None
        if work and not is_work_boundary_name(name):
            entry = Entry(work.group(1).lower(), "", f"work_{work.group(2).lower()}", name, value)
        elif article:
            entry = Entry(article.group(1).lower(), "", "article", name, value)
        elif status and status.group(3) not in {"NONE", "NUM"}:
            entry = Entry(status.group(1).lower(), status.group(2).lower(), "weapon_status", name, value)
        elif ITEM_KIND_RE.match(name) and value <= 0x7FFF_FFFF:
            entry = Entry(COMMON_FIGHTER, "item", "item", name, value)
        elif fighter_status and fighter_status.group(2) not in FIGHTER_STATUS_SENTINELS:
            # Do not accept this until every FIGHTER_KIND_* spelling has been
            # read.  A matching prefix is not proof that the owner is a real
            # fighter (POKEMON and MOVIE have status constants too).
            fighter_status_candidates.append(
                Entry(
                    fighter_status.group(1).lower(),
                    "",
                    "fighter_status",
                    name,
                    value,
                )
            )
        if entry:
            if name in names:
                raise SystemExit(f"duplicate constant name: {name}")
            names.add(name)
            found.append(entry)
        if weapon_kind:
            key = (weapon_kind.group(1).lower(), weapon_kind.group(2).lower())
            previous = weapon_kinds.setdefault(key, value)
            if previous != value:
                raise SystemExit(f"ambiguous weapon kind for {key}: {previous:#x}/{value:#x}")
        # A behavior artifact must compare an object's kind against a named
        # lua constant. Keep only an exact, source-spelled fighter-name match:
        # aliases (and non-fighter helper constants such as FIGHTER_KIND_ALL)
        # never become a mapping by convention or numeric guess.
        if fighter_kind:
            fighter = fighter_kind.group(1).lower()
            if fighter in fighter_kinds:
                raise SystemExit(f"duplicate fighter kind for {fighter}: {fighter_kinds[fighter]}/{name}")
            fighter_kinds[fighter] = (name, value)

    # Reviewed ownership policy for FighterStatus: preserve only a direct,
    # exact FIGHTER_KIND_<owner> symbol from this pinned source.  Never map
    # aliases, shared FIGHTER_STATUS_KIND_* values, or non-fighter helpers by
    # numeric coincidence or naming convention.
    found.extend(
        entry for entry in fighter_status_candidates if entry.fighter in fighter_kinds
    )
    missing_common = COMMON_FIGHTER_STATUSES - {
        name.removeprefix(COMMON_FIGHTER_STATUS_PREFIX)
        for name in common_fighter_statuses
    }
    if missing_common:
        raise SystemExit(
            "pinned lua_const.rs is missing allowlisted common fighter statuses: "
            + ", ".join(sorted(missing_common))
        )

    # Values only have to be unique within the engine namespace that consumes
    # them: fighter+work-kind, fighter+articles, or fighter+weapon+statuses.
    safe, collision_count = remove_value_collisions(found)
    # Common status entries are scoped to every fighter with an exact direct
    # FIGHTER_KIND_* binding and an existing safe registry entry. Their group
    # stays distinct from fighter-owned statuses so identical numeric values
    # in the two named namespaces are not collapsed or guessed as aliases.
    safe_fighters = {entry.fighter for entry in safe} - {COMMON_FIGHTER}
    supported_fighters = sorted(fighter for fighter in fighter_kinds if fighter in safe_fighters)
    if supported_fighters:
        safe.extend(
            Entry(COMMON_FIGHTER, "common", "fighter_status", symbol, value)
            for symbol, value in common_fighter_statuses.items()
        )
    safe.sort()
    validate(safe)
    # A status request validates the target weapon's concrete kind, not just
    # its owner. This table is internal and only contains groups which already
    # have a generated safe weapon-status entry.
    status_groups = {(entry.fighter, entry.group) for entry in safe if entry.kind == "weapon_status"}
    kinds = sorted(
        (fighter, group, value)
        for (fighter, group), value in weapon_kinds.items()
        if (fighter, group) in status_groups
    )
    # The registry proves which internal fighter names this export supports;
    # a kind name has to match one of those spellings exactly. `pokemon` and
    # `shield`, for example, own weapon constants but are not fighter kinds.
    kinds_by_fighter = sorted(
        (fighter, symbol, value)
        for fighter, (symbol, value) in fighter_kinds.items()
        if fighter in safe_fighters
    )
    fighter_values = [value for _, _, value in kinds_by_fighter]
    if len(fighter_values) != len(set(fighter_values)):
        raise SystemExit("ambiguous numeric fighter kind in generated registry")
    return safe, kinds, kinds_by_fighter, collision_count


def validate(entries: list[Entry]) -> None:
    names: set[tuple[str, str]] = set()
    values: set[tuple[str, str, str, int]] = set()
    for entry in entries:
        name_key = (entry.fighter, entry.name)
        if name_key in names:
            raise ValueError(f"duplicate name in generated registry: {entry.name}")
        names.add(name_key)
        key = (entry.fighter, entry.group, entry.kind, entry.value)
        if key in values:
            raise ValueError(f"duplicate value in generated registry namespace: {key}")
        values.add(key)
        if entry.kind == "fighter_status":
            if entry.name.rsplit("_STATUS_KIND_", 1)[-1] in FIGHTER_STATUS_SENTINELS:
                raise ValueError(f"fighter-status sentinel entered generated registry: {entry.name}")
            # The live status sampler returns a signed i32. A value outside
            # that width cannot be compared exactly later, so reject it at
            # generation rather than silently casting it in a runtime hook.
            if entry.value > 0x7FFF_FFFF:
                raise ValueError(f"fighter-status does not fit signed i32: {entry.name}")
        if entry.kind in WORK_KINDS and is_work_boundary_name(entry.name):
            raise ValueError(f"work identifier boundary entered generated registry: {entry.name}")


def rust_string(value: str) -> str:
    return '"' + value.replace("\\", "\\\\").replace('"', '\\"') + '"'


def render(
    entries: list[Entry],
    weapon_kinds: list[tuple[str, str, int]],
    fighter_kinds: list[tuple[str, str, int]],
    omitted: int,
) -> str:
    lines = [
        "// @generated by scripts/generate_gimmick_registry.py; do not edit.",
        f'pub const SKYLINE_SMASH_REVISION: &str = "{REVISION}";',
        f'pub const LUA_CONST_SHA256: &str = "{SOURCE_SHA256}";',
        f"pub const OMITTED_AMBIGUOUS_ALIASES: usize = {omitted};",
        "#[rustfmt::skip]",
        "pub static ENTRIES: &[RegistryEntry] = &[",
    ]
    for item in entries:
        lines.append(
            "    RegistryEntry { fighter: %s, group: %s, kind: IdentifierKind::%s, symbol: %s, value: 0x%X },"
            % (
                rust_string(item.fighter),
                rust_string(item.group),
                {
                    "work_float": "WorkFloat",
                    "work_int": "WorkInt",
                    "work_flag": "WorkFlag",
                    "article": "Article",
                    "weapon_status": "WeaponStatus",
                    "fighter_status": "FighterStatus",
                    "item": "Item",
                }[item.kind],
                rust_string(item.name),
                item.value,
            )
        )
    lines.extend([
        "];",
        "",
        "#[rustfmt::skip]",
        "#[allow(dead_code)] // Used by the F1.5 transport API until the UI adapter lands.",
        "pub static WEAPON_KINDS: &[WeaponKindEntry] = &[",
    ])
    for fighter, group, value in weapon_kinds:
        lines.append(
            "    WeaponKindEntry { fighter: %s, group: %s, value: 0x%X },"
            % (rust_string(fighter), rust_string(group), value)
        )
    lines.extend(["];", ""])
    lines.extend([
        "#[rustfmt::skip]",
        "pub static FIGHTER_KINDS: &[FighterKindEntry] = &[",
    ])
    for fighter, symbol, value in fighter_kinds:
        lines.append(
            "    FighterKindEntry { fighter: %s, symbol: %s, value: 0x%X },"
            % (rust_string(fighter), rust_string(symbol), value)
        )
    lines.extend(["];", ""])
    return "\n".join(lines)


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("source", type=Path)
    parser.add_argument("output", type=Path)
    parser.add_argument(
        "--check",
        action="store_true",
        help="verify that output is current without rewriting it",
    )
    args = parser.parse_args()
    entries, weapon_kinds, fighter_kinds, omitted = parse(args.source)
    output = render(entries, weapon_kinds, fighter_kinds, omitted)
    if args.check:
        try:
            current = args.output.read_text(encoding="utf-8")
        except OSError as error:
            parser.error(f"cannot read {args.output}: {error}")
        if current != output:
            parser.error(f"{args.output} is stale; regenerate it without --check")
    else:
        args.output.write_text(output, encoding="utf-8", newline="\n")
    print(
        f"{'verified' if args.check else 'wrote'} {len(entries)} safe identifiers, "
        f"{len(weapon_kinds)} weapon-kind checks, "
        f"and {len(fighter_kinds)} fighter-kind symbols; "
        f"omitted {omitted} ambiguous aliases"
    )


if __name__ == "__main__":
    main()
