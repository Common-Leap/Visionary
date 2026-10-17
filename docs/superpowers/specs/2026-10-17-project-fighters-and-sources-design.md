# Project-scoped fighters and one Sources window

Date: 2026-10-17. Status: draft for approval.

## Why

Two problems, one cause.

1. The left panel's fighter list ignores the open project. `reindex_fighters`
   (`src/app.rs`) lists every fighter in the data root plus every enabled Mod Library
   root; nothing in `modproject.json` changes it. Opening a different project looks
   like nothing happened.
2. Users cannot tell where the ArcExplorer dump goes versus their own mod's files.
   Today there are five inputs picked in five places, and four of them are global:

   | Input | Picked from | Persisted |
   |---|---|---|
   | ArcExplorer dump | left panel "Open Data Root…", File → "Choose base game folder…" | global `data_root` |
   | Effect editor root | effect editor (also set by the data root) | global |
   | Reference mods | File → Mod Library | global `mod_roots` / library |
   | Moveset Rust crate | ACMD source window "Link…" | global `acmd_source` |
   | The project's own files | Project Hub → workspace `modproject.json` + `romfs/` | per project |

   The same input has three names ("data root", "base game folder", "export root") and
   none says "ArcExplorer dump". The workspace `romfs/` is not an index root, and a code
   comment notes people open their moveset's romfs *as the data root* — the confusion is
   already happening.

The organising rule of this design: **the game dump belongs to the machine; the
project's own files and code belong to the project.**

## Goals

- With a project open, the fighter list leads with the fighters that project is about.
- One place — a **Sources** window — shows every input, in load order, with plain names,
  what each is for, its status, and a picker.
- A first-run setup gets the dump right before anything else, and catches a mod folder
  picked by mistake.
- Opening a project re-links that project's moveset crate.
- The project's `romfs/` is a real source for the fighter list and previews.

## Non-goals

- Per-project dump paths. The dump stays global (same game for every project).
- Per-project reference-mod sets. The Mod Library stays global, as today.
- Changing how edits, autosave, export, or the roster window work.

## Anti-goals (a failure even if it "works")

- An edited fighter hidden in a collapsed group.
- Machine-specific absolute paths written into an exported mod's `modproject.json`.
- An old project that loads with fewer fighters visible, or fails to load.
- Browsing without a project becoming worse than today.

## Decisions (made with joobert)

### Fighter list

With a project open, the left panel's Fighters section has two groups:

- **In this project** — expanded, at the top.
- **Everything else** — every other fighter, collapsed by default, for reference
  (effect donors, comparisons). Search filters both groups and expands a group that has
  matches.

With no project open ("Browse without project"), the list is exactly today's flat list.

A fighter is **in the project** when any of these hold:

1. **Explicitly added** — stored in `modproject.json` (new field, below).
2. **Has files in the project's `romfs/fighter/<name>/`.**
3. **The linked moveset crate has scripts for it** (`SourceIndex::fighters`).
4. **Has saved edits** — any non-empty `FighterMod` in the project. *(joobert left this
   to me; my call: yes, and editing a fighter adds it to the explicit list, so it stays
   even if the edit is later undone. Rationale: an edited fighter must never be hidden.)*

Membership controls:

- New Project asks which fighter(s) the project is for (multi-select, may be skipped).
- Right-click a Base game fighter → **Add to project**; right-click an In this project
  fighter → **Remove from project**. Remove only clears the explicit entry; a fighter
  that is in by rule 2–4 stays and the menu says why ("has files in romfs/", "moveset
  has scripts", "has edits").
- An **Add fighter…** button under the In this project group opens a searchable picker.

### Sources (load order)

One **Sources** window lists the inputs top to bottom. Precedence (decided by joobert):

- **The project's `romfs/` wins over everything** for any file it provides, in the
  fighter list and in previews (models, motions, params, icons). A c00 model copied with
  "Copy base character" and edited previews from `romfs/`, not the dump.
- **Reference mods keep today's behaviour**: the first root to define a fighter owns its
  model/motion/param paths (the dump stays authoritative), and mods add fighters and
  slots. Do not change this.

So path resolution becomes: project `romfs/` file if present, else today's first-wins
resolution across dump + reference mods. A project fighter that exists only in
`romfs/` (a new character) is defined by it like any added fighter.

1. **Base game — ArcExplorer dump** (global). The folder holding `fighter/`, `effect/`,
   optionally `ui/`. Today's `data_root`; also drives the effect editor root as it
   already does.
2. **Reference mods** (global). A summary of the enabled Mod Library entries with a
   button that opens the Mod Library. Unchanged behaviour.
3. **This project's files** (per project). The workspace `romfs/`. New: it becomes an
   index root that overrides the layers above for the files it holds (see precedence).
4. **Moveset code** (per project). The linked Rust crate. Moves from global
   `acmd_source` into `modproject.json`.

Each row shows: plain name, one-line purpose, status (OK / missing / problem, with the
problem in words), and Change / Open folder actions. Every existing picker — left panel
"Open Data Root…", File → "Choose base game folder…", ACMD "Link…" — opens this window
on the relevant row instead of a bare folder dialog. The File menu gets **Sources…**.
All UI text says "ArcExplorer dump" / "game dump"; "data root" and "export root" go away
from user-facing text.

### First run

When no dump is set, a guided setup replaces the left panel's lone button:

1. **Point at your ArcExplorer dump.** Validates the folder: must contain `fighter/` and
   `effect/`; warns (does not block) when it looks like a mod — has `info.toml`, a
   `.nro`, a `romfs/` wrapper, or only a handful of fighters. If the user picked a
   folder that *contains* a valid dump one level down, offer that folder instead.
2. **Then the Project Hub** (New / Open / Import / Browse without project).

Moveset linking stays optional and lives in the Sources window and the ACMD window.

## Data format

`modproject.json` gains (bump `PROJECT_VERSION` to 4; both fields `#[serde(default)]` so
v1–v3 load unchanged):

- `project_fighters: Vec<String>` — the explicit membership list (internal names).
- `moveset_source: Option<String>` — the linked crate path. Stored **relative to the
  workspace folder** when the crate is inside it, absolute otherwise.

Export rule: see "Which writes strip paths" below. Re-opening a stripped copy just
shows "Moveset code: not linked".

## Migration

- An old project has no `project_fighters`. On load, seed it from fighters with edits and
  fighters with authored roster entries (`RosterMod`), so the In this project group is
  never emptier than what the project already touches.
- An old project has no `moveset_source`; see "Linking while a project is open" below.
- Browse without project keeps using the global link, as today.

## Import Mod as Project

*(My call; flag if wrong.)* Behaviour is unchanged — the imported mod folder still goes
into the global Mod Library as a reference mod. The imported fighters are seeded into
`project_fighters` so they appear in the In this project group.

## Calls settled after review (mine; flag if wrong)

- **Membership is per list row, not per internal name.** A mod character on extra slots
  (same internal `name`, own `custom_name`) is its own row and is added/removed on its
  own. `project_fighters` entries therefore carry the row identity (internal name plus,
  for slot characters, the character's name — stable when slots are added or moved). Rule 2 matches a row when its slots have files
  under `romfs/`; a romfs folder for a slot no row owns matches the base fighter row.
- **The second group is "Everything else", not "Base game".** Whole new characters from
  reference mods sit there with their existing `[mod]` tag.
- **Unlabelled moveset crates count for nothing under rule 3.** `SourceIndex` files
  unlabelled scripts under `""`, which matches every fighter; ignore it for membership.
  The Moveset code row in Sources then says "this crate doesn't name its fighters — add
  the fighter to the project yourself".
- **Linking while a project is open writes the project only.** The global `acmd_source`
  changes only in Browse without project. The one-time adoption of the global link is
  dropped: a project with no `moveset_source` just shows "not linked" with a one-click
  "Use <global crate>" offer in the Sources row. That removes the need for an "adopted"
  marker and lets an intentional unlink stick.
- **Which writes strip paths.** Ordinary autosave, Save As, and File → Export editable
  project keep `moveset_source` (they are the user's own working copy). Save As
  re-expresses it against the new location: relative if the crate is inside the new
  workspace, absolute otherwise. Any `modproject.json` written *into a mod folder for
  distribution* (today `write_portable_project` paths and any future one) never carries
  an absolute `moveset_source`.

## Left to the builder

- Widget layout, wording polish, icons, and whether Sources is a window or a panel tab.
- Exact heuristics thresholds for "looks like a mod".
- How membership rule 3 is cached so the list doesn't rescan the crate every frame.
- Test structure; pure helpers for membership and path relativisation should be unit
  tested without a GUI, matching `project_hub.rs`'s style.
