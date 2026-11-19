# Changelog

Adds per-move frame data, one-click Build & Deploy for linked movesets, a Motion List editor, a thrown-fighter preview, in-game fighter scale, model checks before asset previews, SSBH Editor-style camera controls and a Settings window. Visionary now reads real-world moveset source much more faithfully and loads costumes the way ARCropolis does. Fixes scrambled character select alongside roster mods, misplaced helper and out-of-order bones, incomplete recolour costumes, and live effect previews that timed out against a mod-only root.

> Fork 0.3.2, on top of Visionary v0.3.1. Supports Eden, yuzu, Ryujinx, sudachi, suyu, citron and torzu.

## New in 0.3.2

- **Visionary v0.3.1 merged** — Move nodes, Gimmicks, the Effects page (with v0.3.1's character and move pickers and edits kept across file switches), effects in the preview, stages, objects and the grab opponent, with this fork's features carried on top
- **SSBU 13.0.5** — the plugin detects 13.0.4 or 13.0.5 before installing any hook and reads every main-executable offset from that build's table; any other version leaves it off with a log line. The boot log records `game=` and `loader=`. Upstream's gimmick hooks check the game's own bytes before patching and turn off where they don't match
- **Ozone** — the four `lua2cpp_common` hooks install once their symbols resolve, never on a null import, so the game boots under Ozone
- **Source retiming** — moving a hitbox in time in a linked moveset moves its call to the execute block at the new frame (or makes one, splitting a `wait`), then re-parses to check every hitbox's timing. Scripts timed through loops or branches are refused
- **Grab clears** — a grab ends at one move-level clear (`GrabModule::clear_all` or `grab!(…GRAB_CLEAR_ALL)`). A lengthened grab suppresses the script's clear and injects one on the edited frame, and the status line warns when one grab is cut short by another's clear. `grab!` forms are kept as written on export
- **Effects between script frames** — effects retimed, added or stopped on a frame no ACMD `frame`/`wait` reaches fire from the fighter's per-frame status line; a shared latch stops a target firing twice
- **Grab and throw opponent** — the **Thrown fighter** picker (fighter, costume, thrown motion) drives the opponent in grabs and throws. It is on by default, and falls back to Mario, then the fighter itself, when the picked costume has no model or motion
- **Execute blocks** — one-line blocks and any agent name (`if macros::is_excute(fighter) { … }`) parse as typed hitboxes and effects, and a `/* … */` comment spanning lines no longer misplaces block ends
- **Live hitboxes** — a hitbox with a constant Visionary can't resolve waits for a live capture instead of sending a zero
- **Project version 8** — v1–v3 files and Common-Leap v7 files get their fighter list built from their edits; a fork v4 or v8 file keeps its own. The project loader keeps the fighter list and linked moveset. Builds older than this refuse a v8 file rather than dropping its sections

## New features

### Frame data

A frame data line above the collision list for the selected move, recalculated as you edit. Hover any figure to see where it came from.

- **Startup**, **Active** windows (`5-7, 9-10`), **FAF** and **Landing lag** for aerials
- **Shieldstun and advantage** from the strongest hitbox on the startup frame, using the smash-attack or aerial multiplier from `common.prc`
- Every figure is in game frames. `FT_MOTION_RATE`, `FT_MOTION_RATE_RANGE`, `FT_DESIRED_RATE` and `MotionModule::set_rate` all count, including waits inside loops
- Landing lag set in your linked source (`param_config` overrides) wins over a mod's `fighter_param.prc`, which wins over vanilla

### Build & Deploy

- **Build & Deploy** in the ACMD Source header runs `cargo skyline build --release` on the linked crate
- Replaces every deployed copy on the emulator SD: same-named plugins under `ultimate/mods/`, the Skyline plugins folder or LayeredFS `load/`, a renamed `plugin.nro` recognised by the source paths compiled into it, and any destination chosen with **Deploy to…**
- Plugins that export `smashline_install` are also written as smashline's `development.nro`, so **L + R + D-pad Up** reloads them without restarting the game
- Builds run in the background; the first compiler error and its location appear on the status line, with the full output in `visionary-build.log`

### Motion List editor

Ported from SSBH Editor. **Tools → Motion list** edits the selected fighter's `motion_list.yml` or `motion_list.bin`.

- Motion kinds, game and other scripts, up to three animations picked from the folder's `.nuanmb` files, flags, blend frames, cancel frames and intangibility frames
- Add, duplicate, delete and rename motions; save as either format
- A list with unsaved edits stays open until saved or discarded
- The move list reads whichever of `.bin` and `.yml` was written last

### Moveset source reading

- **Cargo-aware** — `cargo metadata` decides which crates a workspace builds, where each crate's root is and where its plugin is built to, with folder heuristics as a fallback
- **Only compiled code** — files are followed through the `mod` tree, so `srcOLD/` backups, release copies and reference dumps beside the crate are ignored
- **Every registration form** — `.acmd`, `.game_acmd`, `.effect_acmd`, `.sound_acmd`, `.expression_acmd` and `install_acmd_script!`, with the last registration winning as in game. Multi-agent installers give each module to the agent they hand it, and status functions are never mistaken for ACMD
- **Any style** — bare macros, any agent parameter name and one-line `if is_excute` blocks parse
- **Project shorthand** — hash macros (`se!`, `cmn!`), numeric consts and one-call helper wrappers show their real values
- **Shared implementations** — scripts that forward to a shared function show its body with the caller's arguments substituted where certain
- **Custom motion names** — hashed script names in `motion_list.bin` are named from ParamLabels, your source, compiled plugins, `motion_list.yml` and animation file names
- **Script provenance** — the status line says whether the open move came from your source, vanilla, the project's saved copy or a live capture, and why a linked project still shows vanilla

### Style-preserving edits

- A value changes in place and nothing else on the line is rewritten
- Hashes written through your macros stay in them (`se!("jump01")` → `se!("jump02")`)
- A const-backed value changes at its definition, and the report counts its other uses
- Values a helper fixes inside its own body are refused with a note
- Added lines use the file's own macro path, agent name, frame style, indentation and line endings
- Write-back is refused for shared or substituted bodies

### Viewport

- **Helper bones and material animations** — `model.nuhlpb` and `model.numatb` are applied, so `H_` helpers move the mesh and glows, colour changes and eye swaps play
- **ARCropolis costume loading** — each file resolves from the costume folder, then `config.json` `share-to-vanilla` / `share-to-added` (followed through chains), then the vanilla dump. A toolbar note lists borrowed files and their sources
- **Thrown fighter** — picks the fighter and costume upstream's grab/throw opponent shows (Mario by default, any indexed or vanilla fighter). On throws it plays that costume's thrown animation: **Auto** matches the throw direction or the thrower's own thrown set; `_dx` / `_big` / `_girl` variants are picked from the list
- **In-game scale** — `fighter_param` scale or its `param_config` override, then `ModelModule::set_scale`, `AttackModule::set_attack_scale` and `GrabModule::set_size_mul` from the moveset's `on_line` callback. Hitboxes and grabs take their own scale; a toolbar label shows each value and its source line
- **SSBH Editor camera** — left drag orbits, right or middle drag pans with the model following the pointer, scroll zooms, and **⟲** resets to the side view

### Settings window

The viewport's **⚙** opens **Settings**, saved to `preferences.json` in the config directory.

- **Show console window** (Windows), **Interface scale** kept in step with Ctrl +/-
- **Background** colour (SSBH Editor's dark grey by default), **Floor grid**, **Bloom** and **Enable helper bones**
- **Camera** translation, rotation and field of view, with **⟲ Reset view**

### Model checks

Ported from SSBH Editor. Runs on model import, before every **Reload asset preview**, on export, and from **Check model** in Project Files.

- **Blocking** — unreadable files, `model.numdlb` entries naming a missing mesh or material, unknown shader labels, and cube maps in 2D slots or the reverse
- **Warnings** — missing vertex attributes, RENORMAL meshes without `model.adjb` entries, sRGB mismatches, clamped tiling, uneven skin weights, duplicate names and missing textures
- Issues the costume's vanilla model already has are left out; exports list findings but are never blocked

### Slot characters

- A mod character on a vanilla fighter's extra slots (Shy Guy on Pac-Man c40–c47) gets its own fighter row under its own name
- Hover shows the base fighter and slot runs; searching the base fighter's name still finds it
- The data root is searched too, and an empty base-fighter row is dropped when the character takes every slot

### Project fighters

With a project open, the left panel splits the fighter list into **In this project** (expanded) and **Everything else** (collapsed); search filters both and opens a group that matches. With no project open the list is the flat list as before.

- A fighter is in the project when it was **added** to it, has **files in the project's `romfs/fighter/<name>/`**, has **scripts in the linked moveset crate**, or has **saved edits**. A fighter's first edit adds it to the project, and it stays after the edit is undone. Removing an edited fighter sticks: it stays listed under "has edits" only until its edits are gone
- Right-click **Add to project** / **Remove from project**, or use **Add fighter…** under the group. Remove only clears the explicit entry; the menu says why a fighter stays ("Still in the project because it has files in romfs/ and has scripts in the moveset"); the row's hover gives every reason the same way
- Membership is per list row, so a slot character is added and removed on its own
- A crate that does not label its fighters counts for nothing here; the Sources row says so
- New Project asks which fighters it is for (optional). Import Mod as Project adds the imported fighters. Older projects start with the fighters they already edit or give roster entries to

### Sources window

**File → Sources…** lists every input in load order with its purpose and status (OK, missing, or the problem in words), plus Change and Open folder actions. Every older picker (left panel, File menu, ACMD **Link…**) opens it on the relevant row.

- **Base game — ArcExplorer dump** (global), **Reference mods** (global, opens the Mod Library), **This project's files** (the workspace `romfs/`) and **Moveset code** (per project)
- **Precedence** — a file in the project's `romfs/` wins over the dump and mods, in the fighter list and in previews (models, motions, params, icons). Effects still resolve from the dump and mods only, so an exported effect edit can't overwrite a `romfs/` file. Between the dump and mods the first root to define a fighter still owns its model, motion and parameter paths
- **Moveset link per project** — stored in `modproject.json`, relative to the project folder when the crate is inside it. Linking with a project open changes only the project; the global link changes only without one. A project with no link offers "Use" for the global crate
- **Dump check** — a folder must hold `fighter/` and `effect/`. A folder that looks like a mod (`info.toml`, a `.nro`, a `romfs/` wrapper, or few fighters) gets a warning you can override, and a folder with a valid dump one level down offers that folder
- User-facing text says "ArcExplorer dump" instead of "data root", "export root" or "base game folder"

### First run

With no dump set, the left panel shows "Step 1 — your game dump" and **Choose game dump…**, running the same check as the Sources window. The Project Hub stays hidden until a dump is set; once a dump is accepted with none set before and no project open, the hub opens as step 2.

### Emulator support

- SD detection adds sudachi, suyu, citron and torzu alongside Eden, yuzu and Ryujinx
- With several SD roots, Visionary uses the one the game wrote to most recently

## Bug fixes

- **Effect export overwrote its source** — an eff opened from outside the dump was saved by its absolute path, and export joined that onto the mod folder, writing over the original file. Effects are now recorded by their game path, old absolute sources are rewritten on load, and export refuses any that is still absolute
- **Stage picker over the viewport toolbar** — both were drawn in the viewport's top-left corner. The picker now sits under the toolbar, and the Effects page's view switch moves down when the toolbar reaches it
- **Broken tooltip text** — six tooltips and status messages had a run of spaces in the middle where a line continuation was lost
- **Effects page character picker** — picking a mod character on another fighter's slots (Shy Guy on Pac-Man) opened the fighter; the picker now tells the two apart

- **Character select scrambled with roster mods** — the in-game plugin registered a "no-op" ARCropolis probe on `ui_chara_db.prc`. ARCropolis keeps one callback per file, so it replaced the loader roster mods patch through. The probe is removed, and the unmeasured roster heap patch stays disabled; CSS order changes apply on the next menu load
- **Misplaced bones** — hitbox, hurtbox and effect positions now come from the same skeleton evaluation that skins the mesh, so they follow helper bones, and bones listed before their parent (Mario's jaw and lips) are no longer drawn at the origin
- **Incomplete costumes** — texture-only recolours rendered with no mesh and animations shared through `config.json` were missing from the move list
- **Live effect carrier** — the carrier is built from a separate **Vanilla dump** root (set in the Eff Editor), resolving the base and each donor per file across roots. A carrier that can't be built stops the send indicator at once with the reason instead of timing out
- **Dropped scripts** — a folder registering a fighter and its articles lost scripts; registrations are now attributed to the agent the call is made on
- **Custom motions showing vanilla** — motions such as `attack_s4_s_reverse` now reach their real scripts when the name-based guess misses
- **Frame data with rate changes** — rate forms other than `FT_MOTION_RATE`, and rate calls inside loops, were ignored
- **Console window flashes** — cargo, bash and python runs no longer open console windows on Windows

## Other changes

- Visionary is a Windows GUI-subsystem program. Started from a terminal it logs there; otherwise a console opens only while **Show console window** is on, with its close button removed
- The helper bones toggle moved from a render settings popup into the Settings window
- The thrown-fighter victim list skips slot-character rows
- `ssbh_wgpu` moved to [joobert/ssbh_wgpu](https://github.com/joobert/ssbh_wgpu) for `ModelFolder::load_files` and `apply_anims_with_skel_edit`; `serde_yaml` added for `motion_list.yml`
- New ignored `moveset_survey` test runs the source indexer over a folder of moveset projects (`VISIONARY_MOVESETS`, `VISIONARY_SURVEY_ONLY`, `VISIONARY_SURVEY_PLAN`)
- The single-root carrier builder is gated to tests
- README documents moveset styles, frame data, Build & Deploy, the viewport toolbar, Settings, costume loading, model checks, the Motion List editor and slot characters
