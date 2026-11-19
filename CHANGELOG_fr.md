# Changelog

Everything Visionary 0.3.2 changes relative to 0.3.1, in full technical detail. Entries cover
what is new and, where 0.3.1 already had something, how it now behaves differently.
For plain-language patch notes and the earlier releases, see [CHANGELOG.md](CHANGELOG.md); for
a middle ground, see [CHANGELOG_DETAILED.md](CHANGELOG_DETAILED.md).

## Changed from 0.3.1

- **Camera.** Left drag orbits and right or middle drag pans (0.3.1 panned on left drag and
  orbited on right or middle). The camera still follows the fighter, double-click still turns
  back to the side view, and ⟲ resets the whole view.
- **Grab and throw opponent.** The **Thrown fighter** picker chooses the fighter and costume
  the opponent shows in grabs and throws, hung by the hip as before. It is on by default, and
  the victim's own scale is not applied. When the picked costume has no model or motion it
  falls back to Mario, then the fighter itself.
- **Execute blocks.** 0.3.1 recognised `if macros::is_excute(agent) {` exactly. One-line blocks
  and any agent name are now accepted too, so those movesets keep typed hitboxes and effects.
  The lexical block-end search mapped byte offsets back to lines by counting `\n`, which
  drifted after a `/* … */` comment spanning lines; it now maps offsets to the line entries.
- **Grab clears.** `grab!(…GRAB_CLEAR_ALL)` stays a `NativeCall` and is kept as written on
  export; the timeline still ends grab boxes on it.
- **Live injection.** A hitbox whose constants don't resolve waits for a capture rather than
  sending a zero. Effects keep the capture-free path.
- **Project version.** `PROJECT_VERSION` is 8 (was 7). v8 adds `project_fighters` and
  `moveset_source` (see Project fighters below). v1–v3 and v7 files are seeded with project
  fighters from their edits; v4 files, written only by development builds, and v8 files keep
  their own list. The legacy loader (`legacy_move_triggers`) passes both fields through. 0.3.1
  refuses a v8 file with its "Update Visionary" error.
- **Effects page pickers.** The Character picker keys rows by `name` plus `custom_name`, since a
  slot character (below) is its own row with its base fighter's name. `load_needing_edits`
  returns `eff_game_rel` like `loaded_rel`, so an eff opened from outside the dump is handed its
  edits by the game path the project records.

## Game versions and loaders

- **SSBU 13.0.5.** `offsets.rs` holds each main-executable offset for 13.0.4 and 13.0.5. The
  plugin detects the build before installing any hook and stays off on any other version. The
  boot log records `game=`, `loader=` and `copy=`. The gimmick hooks (damage capture and
  commit, incoming hit, cloud resource, live parameters) check the game's bytes or digests
  before patching, so on 13.0.5 they turn off rather than patch the wrong address.
- **Ozone.** The coroutine and `L2CFighterAnimcmdEffectCommon` hooks resolve their target
  through the bound import or `nn::ro::LookupSymbol` and install when a `lua2cpp` module
  loads, never on a null address. Ozone binds imports immediately and aborted on the old
  null import. The plugin builds against the smashline branch with PR 87 merged (`137aeb7`;
  the earlier `c510441` was force-pushed away).

## Timing edits in linked source

- **Source retiming** (`source_timing.rs`). Moving a hitbox in time in a linked moveset
  moves its call to the execute block at the new frame, or makes one by splitting a `wait`,
  and re-parses the result to check every hitbox's timing. Scripts timed through loops or
  branches are refused.
- **Grab clears.** A grab ends at one move-level clear. A lengthened grab suppresses the
  script's clear and injects one on the edited frame (wire category 37), and the status line
  warns when one grab is cut short by another's clear.
- **Effects between script frames.** `acmd_hooks::inject_tick` runs from the fighter's
  `StatusLine::Main` callback and fires effects retimed, added or stopped on frames no ACMD
  `frame`/`wait` reaches, on the first tick at or past the target (motion rates can step over
  the ±0.5 window). The shared latch makes it a no-op for a target a boundary already fired.

## Effect export

- An eff opened from outside the eff root (**Open effect file**, **Browse for ef_….eff**) was
  recorded by its absolute path, and export's `mod_dir.join` then overwrote the user's file. It
  is now recorded by its game path (from its `effect/` folder or `ef_<fighter>.eff` name), old
  absolute sources are rewritten on load, and export refuses any `source_rel` that is still not
  a game path.
- The live carrier's vanilla folder (`vanilla_root`, **Vanilla game files** on the Effects
  page) is remembered apart from the eff root; see Emulator and live preview.

## Tests

- The source-reading tests fold CRLF, so they pass on a Windows checkout.

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
- **Thrown fighter.** Picks the fighter and costume slot the grab/throw opponent shows
  (Mario by default, or any indexed or vanilla fighter); on throws it plays that costume's
  thrown animation. **Auto** matches the throw direction or the thrower's own thrown set. See
  Changed from 0.3.1 for how it drives the opponent.
- **Game scale.** Fighters are drawn at the scale the game gives them: `fighter_param` scale or a
  `param_config` override, then `ModelModule::set_scale`, `AttackModule::set_attack_scale` and
  `GrabModule::set_size_mul` read from the moveset's `on_line` callback (through helpers,
  skipping results-screen and entry branches). Hitbox and grab sizes take their own scale; a
  toolbar label shows each value and where it came from.
- **Camera from SSBH Studio.** `renderer::Camera` keeps a `target` (relative to `follow`):
  `turn_to` rotates while holding the target's view position, `pan_view` moves it with the
  view, `frame` solves the translation that puts a world sphere straight ahead at
  `radius / sin(fov / 2) * 1.1`, and `zoom` scales by distance (clamped at z = -1). The view
  matrix stays translation * rotation * `-follow`. `viewport_navigation` holds the SSBH and
  Blender schemes (a Blender drag's mode is fixed at `drag_started_by`), three-button emulation,
  arrow-key panning, numpad views and the Period / Home frame requests; the app answers those
  with the selected hitbox's or the fighter's bone bounds, recorded in the overlay pass. This
  swaps 0.3.1's drag buttons; see Changed from 0.3.1.
- **Orientation widget** (`orientation_widget.rs`, ported from SSBH Studio). Axis balls or a
  cube with turn arrows in an `egui::Area` at the viewport's top right, placed below
  `viewport_toolbar_rect` when the toolbar reaches it and clamped inside the viewport. Being an
  Area, it takes its own clicks, so they don't pick a hitbox underneath.
- **World axes** (`preview_axes.rs`). A full-screen triangle whose fragments intersect their
  view ray with y = 0 and keep pixels within 1.5 pixels (`fwidth`) of x = 0 or z = 0, faded
  like SSBH Studio's grid. Drawn after the floor in the scene pass with premultiplied blending,
  depth tested without writing, at a depth pulled 0.1% toward the eye so the opaque floor at the
  same height doesn't hide it. `naga` is a dev-dependency (the version wgpu already builds) to
  validate the WGSL in a test.
- **Camera animation.** `ssbh_wgpu::animation::camera::animate_camera` samples the picked
  `.nuanmb` at the timeline's animation frame (clamped to its last frame); `apply_animation`
  decomposes its rotation * translation * scale view into the camera's translation and XYZ
  Euler rotation and zeroes `follow`, so overlays and effects project through the same camera.
  A manual move holds it off for that frame. Session state, not a preference.
- **Settings window.** The gear opens **Settings**, saved to `preferences.json` in the config
  directory: show console window, interface scale (kept in step with Ctrl +/-), viewport
  background colour (SSBH Editor's dark grey by default), floor grid, world axes, bloom, helper
  bones, and a Camera section: navigation scheme, three-button emulation, orientation widget
  style, translation, rotation, field of view, near and far clip, reset, **Save as default**
  (`default_camera`, applied on the first viewport frame and by ⟲ and double-click) and
  **Reset default**, and the camera animation.

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
- **`modproject.json` v8.** `project_fighters: BTreeSet<ProjectFighter>` (`name` plus
  optional `character`) and `moveset_source: Option<String>`, both `#[serde(default)]`; every
  earlier version still loads. `migrate()` seeds a v1–v3 or v7 project's `project_fighters`
  from edited fighters and authored `RosterMod` entries; a v4 or v8 file's list is kept as
  saved. `moveset_source` is relative to the workspace when
  the crate is inside it, absolute otherwise; Save As re-expresses it for the new location.
  Distribution copies never carry an absolute `moveset_source`.
- **Project `romfs/` as an index root.** It overrides the dump and reference mods for any file
  it provides, in the fighter list and in previews (models, motions, params, icons). Effects
  are not overlaid (`apply_romfs_overlay` leaves `effect_dir` alone and `reindex_fighters`
  looks for effect folders in the shared roots only), so effect edits keep resolving against
  the dump and an export never writes over a romfs file. Between the dump and reference mods
  the first-root-wins rule is unchanged.
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
- The live effect carrier is built from a separate vanilla root (**Vanilla game files** on the
  Effects page) rather than the eff root, resolving the carrier base and each donor per file across roots.
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

- `ssbh_wgpu` moved from [Common-Leap/ssbh_wgpu](https://github.com/Common-Leap/ssbh_wgpu)
  (`c7ccdb4`) to [joobert/ssbh_wgpu](https://github.com/joobert/ssbh_wgpu) (`6702cb4`), which is
  `c7ccdb4` plus two commits: `ModelFolder::load_files` (costume models from files in different
  folders) and the skeleton edit hook in `apply_anims_with_skel_edit` (fighter scale at the
  root bones).
- Added `serde_yaml` for `motion_list.yml`.

## Tooling and docs

- `moveset_survey` ignored test runs the source indexer over a folder of moveset projects
  (`VISIONARY_MOVESETS`, `VISIONARY_SURVEY_ONLY`, `VISIONARY_SURVEY_PLAN`).
- The single-root carrier builder is gated to tests.
- README documents moveset styles, frame data, Build & Deploy, the viewport toolbar, costume
  loading, model checks and the features above.
