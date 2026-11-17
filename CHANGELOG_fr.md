# Changelog

Changes in this fork (joobert/Visionary) relative to upstream Visionary by Common Leap.

The fork branched from upstream tag **`v0.1.2`** (`e7df465`, "Autosave project edits and
harden effect/hitbox source sync", 2026-09-17) and has since merged upstream **`v0.3.0`**.
Everything below is what the fork adds; where the merge changed one of these, the entry says
so.
For plain-language patch notes, see [CHANGELOG.md](CHANGELOG.md); for a middle ground, see
[CHANGELOG_DETAILED.md](CHANGELOG_DETAILED.md).

## Moveset source (ACMD Source window)

- **Read movesets as they are written.** Only files reached through the crate's `mod` tree
  are indexed, so backups, release copies and vanilla dumps beside the crate are ignored.
  Submodules go to the agent their installer hands them (`pacman::install(pacman)`), every
  registration form is read (`.acmd`, `.game_acmd`, `.effect_acmd`, `.sound_acmd`,
  `.expression_acmd`, `install_acmd_script!`) with the last registration winning, and status
  functions are skipped. Bare macros (`ATTACK(agent, …)`), any agent parameter name and
  one-line blocks parse.
- **Project shorthand.** Hash macros (`se!("…")`, `cmn!("…")`), numeric consts and one-call
  helper wrappers are resolved for display, and edits write back through them: a macro stays a
  macro, a const changes at its definition, a value a helper fixes is refused, and inserted
  lines match the file's own style (macro path, agent name, indentation, line endings).
- **Forwarding scripts.** A script that forwards to a shared implementation shows the callee's
  body with the caller's arguments substituted where they are statically known. Unresolved
  values are reported, and write-back is refused when the body is shared or substituted.
- **Read through cargo.** `cargo metadata` decides which crates a linked workspace builds, each
  crate's root and its build output, falling back to folder heuristics when cargo can't be
  asked. Registrations are attributed to the agent the call is made on, so a folder that
  registers a fighter and its articles no longer drops scripts.
- **Script provenance.** The status line says where the open move's ACMD came from (moveset
  source, vanilla, saved, live capture) and why a move shows vanilla when a source is linked,
  including a source that could not be re-linked at startup.
- **Custom motion names.** Motion-list script hashes are mapped back to names using
  ParamLabels, the moveset's source and compiled plugins, `motion_list.yml` and animation file
  names, so custom motions such as `attack_s4_s_reverse` show their name and reach their scripts.
- **ACMD parsing improvements** across the parser and the move model.

## Frame data

- Startup, active windows, FAF, landing lag and shieldstun/advantage for the selected move, in
  game frames, above the collision list. Inputs: parsed hitboxes, motion rate, motion-list
  cancel frames, `common.prc` / `fighter_param.prc` and landing lag set in the linked source.
  Every figure explains its source on hover.
- Every rate form counts: `FT_MOTION_RATE`, `FT_MOTION_RATE_RANGE`, `FT_DESIRED_RATE` and
  `MotionModule::set_rate`, including waits inside loops.

## Build & Deploy

- **Build & Deploy** runs `cargo skyline build --release` on the linked crate and replaces every
  deployed copy on the emulator SD: same-named files under `ultimate/mods/`, the Skyline plugins
  folder or LayeredFS `load/`; a renamed `plugin.nro` recognised by the source paths compiled
  into it; and a remembered **Deploy to…** destination.
- When the plugin exports `smashline_install`, it is also written as smashline's
  `development.nro` for hot reload.
- Build output is kept in `visionary-build.log`; the first compiler error is shown on the status
  line.

## Motion List editor (new)

- **Tools → Motion list**, ported from SSBH Editor, edits every field of the selected
  fighter's `motion_list.yml` or `motion_list.bin`: motion kinds, game and other scripts, up to
  three animations picked from the folder's `.nuanmb` files, flags, blend frames, cancel frames
  and intangibility frames. Motions can be added, duplicated, deleted and renamed, and the list
  saved in either format. A list with unsaved edits stays open until saved or discarded.
- The move list reads whichever of `motion_list.bin` / `motion_list.yml` was written last.

## Viewport

- **Helper bones and material animations.** `model.nuhlpb` and `model.numatb` are applied, so
  `H_` helper bones move and a move's material tracks play. Hitbox, hurtbox and effect
  positions now come from the same skeleton evaluation that skins the mesh, which also fixes
  bones listed before their parent (Mario's jaw and lips were drawn at the origin).
- **Costumes load the way ARCropolis does.** Each file resolves from the costume folder, then a
  `config.json` `share-to-vanilla` / `share-to-added` entry (followed through chains), then the
  vanilla dump. Model, weapon skeletons, move-list animations, eyelid animation and motion list
  all use it. A toolbar note lists borrowed files and their sources.
- **Thrown fighter.** Picks the fighter and costume slot upstream's grab/throw opponent shows
  (Mario by default, or any indexed or vanilla fighter); on throws it plays that costume's
  thrown animation. **Auto** matches the throw direction or the thrower's own thrown set.
  Since the v0.3.0 merge it drives upstream's opponent (hung by the hip, also in grabs) instead
  of a separate model placed through the `rot` bone, and the victim's own scale is not applied.
- **Game scale.** Fighters are drawn at the scale the game gives them: `fighter_param` scale or a
  `param_config` override, then `ModelModule::set_scale`, `AttackModule::set_attack_scale` and
  `GrabModule::set_size_mul` read from the moveset's `on_line` callback (through helpers,
  skipping results-screen and entry branches). Hitbox and grab sizes take their own scale; a
  toolbar label shows each value and where it came from.
- **Camera like SSBH Editor.** Left drag orbits, right or middle drag pans with the model
  following the pointer, scroll zooms. A reset button beside the gear returns to the default
  side view.
- **Settings window.** The gear opens **Settings**, saved to `preferences.json` in the config
  directory: show console window, interface scale (kept in step with Ctrl +/-), viewport
  background colour (SSBH Editor's dark grey by default), floor grid, bloom, helper bones, and a
  Camera section with translation, rotation, field of view and reset.

## Asset preview

- **Model checks**, ported from SSBH Editor, run on model import, before every **Reload asset
  preview**, on export (warnings only) and from **Check model** in Project Files. Unreadable
  files, mesh/material references that don't exist, unknown shader labels and cube/2D texture
  mismatches block a reload; everything else is a warning. Issues the costume's vanilla model
  already has are left out.

## Fighter list

- **Slot characters.** A mod that plays its own character on a vanilla fighter's extra slots
  (Shy Guy on Pac-Man c40–c47) gets its own row under its own name; hover names the base fighter
  and its slot runs, and searching the base fighter still finds it. The data root is searched
  for these too, and an empty base-fighter row is dropped when the character takes every slot.
  The thrown-fighter list skips slot-character rows.

## Project fighters and Sources

- **Project-scoped fighter list.** With a project open, the `reindex_fighters` output is split
  into "In this project" (expanded) and "Everything else" (collapsed; search filters both and
  expands a matching group). No project: the flat list, unchanged. Membership: explicit
  `project_fighters` entry, files in `romfs/fighter/<name>/`, scripts for the fighter in the
  linked crate (`SourceIndex::fighters`, unlabelled scripts under `""` ignored), or a non-empty
  `FighterMod` (`has_edits()`, on the fighter's own row). A fighter's first edit adds the
  explicit entry, so it survives undo; removing it sticks while the edits keep the row listed
  as "has edits". Membership is per row
  (internal name plus the character name for slot characters). Right-click Add to / Remove
  from project (with the reason when a rule keeps it in), an **Add fighter…** picker, and a
  fighter picker in New Project. Import Mod as Project seeds the imported fighters.
- **`modproject.json` v4.** `project_fighters: BTreeSet<ProjectFighter>` (`name` plus optional `character`) and `moveset_source: Option<String>`,
  both `#[serde(default)]`; v1-v3 still load. `migrate()` seeds a v1-v3 project's
  `project_fighters` from edited fighters and authored `RosterMod` entries; a v4 file's list is
  kept as saved. `moveset_source` is relative to the workspace when
  the crate is inside it, absolute otherwise; Save As re-expresses it for the new location.
  Distribution copies never carry an absolute `moveset_source`.
- **Project `romfs/` as an index root.** It overrides the dump and reference mods for any file
  it provides, in the fighter list and in previews (models, motions, params, icons). Effects
  are not overlaid (`apply_romfs_overlay` leaves `effect_dir` alone and `reindex_fighters`
  looks for effect folders in the shared roots only): the eff editor's saved `source_rel`
  falls back to an absolute path outside the dump, and export would write it back over the
  romfs file. Between
  the dump and reference mods the first-root-wins rule is unchanged.
- **Per-project moveset link.** Opening a project re-links its crate. Linking with a project
  open writes the project only; the global `acmd_source` changes only in Browse without
  project. A project with no link shows "not linked" with a one-click "Use <global crate>".
- **Sources window.** File → Sources… lists the dump, reference mods, project files and moveset
  code in load order with purpose, status and Change / Open folder. The left panel, File menu
  and ACMD **Link…** now open it on the matching row. UI text says "ArcExplorer dump".
- **Dump check (`sources::check_dump`).** Requires `fighter/` and `effect/`; warns on
  `info.toml`, a `.nro`, a `romfs/` wrapper or few fighters; offers a valid dump one level down.
  A failing folder is held as `pending_dump` and resolved in the Sources window.
- **First run.** With no dump, the left panel shows "Step 1 — your game dump" and **Choose game
  dump…** (`pick_game_dump`, shared with the Sources row). `VisionaryApp::new` keeps the hub
  hidden while no dump is set; applying a dump when none was set and no project is open opens
  the Project Hub (`apply_game_dump`, no longer gated on recent projects),
  on both the immediate path and the confirmed-from-Sources path.

## Emulator and live preview

- SD detection also probes sudachi, suyu, citron and torzu, and picks the SD root the game wrote
  to most recently instead of the first one found.
- The live effect carrier is built from a separate **Vanilla dump** root (set in the Eff Editor)
  rather than the eff root, resolving the carrier base and each donor per file across roots.
  When the carrier can't be built, the send indicator stops immediately with the reason instead
  of timing out.

## Windows

- Visionary is a GUI-subsystem program. Started from a terminal it logs there as before;
  otherwise a console opens only while **Show console window** is on. cargo, bash and python
  runs no longer flash a console window.

## In-game plugin (slight_replica)

- Removed the `ui_chara_db` ARCropolis probe callback. ARCropolis keeps one callback per file,
  so the "no-op" probe replaced the loader that roster mods patch through and scrambled
  character select whenever the plugin was installed.
- The roster heap patch stays disabled until the `ui_chara_db` in-memory layout is measured;
  CSS order changes apply on the next menu load.

## Dependencies

- `ssbh_wgpu` moved from ScanMountGoat's upstream (`f7bfb4b`) to
  [joobert/ssbh_wgpu](https://github.com/joobert/ssbh_wgpu) (`592d0d1`), which adds
  `ModelFolder::load_files` (models from files in different folders) and
  `apply_anims_with_skel_edit` (the thrown-fighter placement).
- Added `serde_yaml` for `motion_list.yml`.

## Tooling and docs

- `moveset_survey` ignored test runs the source indexer over a folder of moveset projects
  (`VISIONARY_MOVESETS`, `VISIONARY_SURVEY_ONLY`, `VISIONARY_SURVEY_PLAN`).
- The single-root carrier builder is gated to tests.
- README documents moveset styles, frame data, Build & Deploy, the viewport toolbar, costume
  loading, model checks and the features above.
