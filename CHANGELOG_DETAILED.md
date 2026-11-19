# Changelog

## 0.3.2

Adds per-move frame data, one-click Build & Deploy for linked movesets, a Motion List editor, a choice of grab and throw opponent with its thrown animations, in-game fighter scale, model checks before asset previews, SSBH Editor-style camera controls and a Settings window. The in-game plugin runs on SSBU 13.0.5 and under Ozone. Visionary now reads real-world moveset source much more faithfully, keeps hitbox, grab and effect timing edits in your own code, and loads costumes the way ARCropolis does. Fixes scrambled character select alongside roster mods, misplaced helper and out-of-order bones, incomplete recolour costumes, effect exports that overwrote an opened file, and live effect previews that timed out against a mod-only root.

> Supports Eden, yuzu, Ryujinx, sudachi, suyu, citron and torzu. The node system is still unfinished: keep backups of your projects.

### Changed from 0.3.1

- **Camera drag** — left drag orbits and right or middle drag pans with the model following the pointer (0.3.1 had them the other way round). Scroll zooms, double-click returns to the side view, the camera still follows the fighter, and **⟲** resets the view
- **Project version 8** — projects saved by 0.3.2 are version 8 and carry the project's fighter list and linked moveset. Every earlier version loads; v1–v3 and v7 files get their fighter list built from their edits, v4 and v8 files keep their own. 0.3.1 and older refuse a v8 file with an "Update Visionary" message rather than dropping its sections
- **Grab and throw opponent** — chosen with the toolbar's **Thrown fighter** picker (fighter, costume, thrown motion). It is on by default, and falls back to Mario, then the fighter itself, when the picked costume has no model or motion. The opponent's own fighter scale is not applied

### Game versions and loaders

- **SSBU 13.0.5** — the plugin detects 13.0.4 or 13.0.5 before installing any hook and reads every main-executable offset from that build's table; any other version leaves it off with a log line. The boot log records `game=` and `loader=`. The gimmick hooks check the game's own bytes before patching and turn off where they don't match
- **Ozone** — the four `lua2cpp_common` hooks install once their symbols resolve, never on a null import, so the game boots under Ozone

### Timing edits in linked source

- **Source retiming** — moving a hitbox in time in a linked moveset moves its call to the execute block at the new frame (or makes one, splitting a `wait`), then re-parses to check every hitbox's timing. Scripts timed through loops or branches are refused
- **Grab clears** — a grab ends at one move-level clear (`GrabModule::clear_all` or `grab!(…GRAB_CLEAR_ALL)`). A lengthened grab suppresses the script's clear and injects one on the edited frame, and the status line warns when one grab is cut short by another's clear. `grab!` forms are kept as written on export
- **Effects between script frames** — effects retimed, added or stopped on a frame no ACMD `frame`/`wait` reaches fire from the fighter's per-frame status line; a shared latch stops a target firing twice
- **Execute blocks** — one-line blocks and any agent name (`if macros::is_excute(fighter) { … }`) parse as typed hitboxes and effects, and a `/* … */` comment spanning lines no longer misplaces block ends
- **Live hitboxes** — a hitbox with a constant Visionary can't resolve waits for a live capture instead of sending a zero

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
- **Thrown animations** — on throws the opponent plays its costume's thrown animation: **Auto** matches the throw direction or the thrower's own thrown set; `_dx` / `_big` / `_girl` variants are picked from the list. Slot-character rows are not offered as opponents
- **In-game scale** — `fighter_param` scale or its `param_config` override, then `ModelModule::set_scale`, `AttackModule::set_attack_scale` and `GrabModule::set_size_mul` from the moveset's `on_line` callback. Hitboxes and grabs take their own scale; a toolbar label shows each value and its source line

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

- A mod character on a vanilla fighter's extra slots (Shy Guy on Pac-Man c40–c47) gets its own fighter row under its own name, in the fighter list and the Effects page's Character picker
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
- **Precedence** — a file in the project's `romfs/` wins over the dump and mods, in the fighter list and in previews (models, motions, params, icons). Effects still resolve from the dump and mods only. Between the dump and mods the first root to define a fighter still owns its model, motion and parameter paths
- **Moveset link per project** — stored in `modproject.json`, relative to the project folder when the crate is inside it. Linking with a project open changes only the project; the global link changes only without one. A project with no link offers "Use" for the global crate
- **Dump check** — a folder must hold `fighter/` and `effect/`. A folder that looks like a mod (`info.toml`, a `.nro`, a `romfs/` wrapper, or few fighters) gets a warning you can override, and a folder with a valid dump one level down offers that folder
- User-facing text says "ArcExplorer dump" instead of "data root", "export root" or "base game folder"

### First run

With no dump set, the left panel shows "Step 1 — your game dump" and **Choose game dump…**, running the same check as the Sources window. The Project Hub stays hidden until a dump is set; once a dump is accepted with none set before and no project open, the hub opens as step 2.

### Emulator support

- SD detection adds sudachi, suyu, citron and torzu alongside Eden, yuzu and Ryujinx
- With several SD roots, Visionary uses the one the game wrote to most recently

### Bug fixes

- **Effect export overwrote its source** — an eff opened from outside the dump (**Open effect file**, **Browse for ef_….eff**) was saved by its absolute path, and export joined that onto the mod folder, writing over the original file. Effects are now recorded by their game path, old absolute sources are rewritten on load, and export refuses any that is still absolute
- **Character select scrambled with roster mods** — the in-game plugin registered a "no-op" ARCropolis probe on `ui_chara_db.prc`. ARCropolis keeps one callback per file, so it replaced the loader roster mods patch through. The probe is removed, and the unmeasured roster heap patch stays disabled; CSS order changes apply on the next menu load
- **Misplaced bones** — hitbox, hurtbox and effect positions now come from the same skeleton evaluation that skins the mesh, so they follow helper bones, and bones listed before their parent (Mario's jaw and lips) are no longer drawn at the origin
- **Incomplete costumes** — texture-only recolours rendered with no mesh and animations shared through `config.json` were missing from the move list
- **Live effect carrier** — the carrier is built from a separate vanilla folder (**Vanilla game files** on the Effects page), resolving the base and each donor per file across roots. A carrier that can't be built stops the send indicator at once with the reason instead of timing out
- **Dropped scripts** — a folder registering a fighter and its articles lost scripts; registrations are now attributed to the agent the call is made on
- **Custom motions showing vanilla** — motions such as `attack_s4_s_reverse` now reach their real scripts when the name-based guess misses
- **Console window flashes** — cargo, bash and python runs no longer open console windows on Windows

### Other changes

- Visionary is a Windows GUI-subsystem program. Started from a terminal it logs there; otherwise a console opens only while **Show console window** is on, with its close button removed
- `ssbh_wgpu` moved from [Common-Leap/ssbh_wgpu](https://github.com/Common-Leap/ssbh_wgpu) (`c7ccdb4`) to [joobert/ssbh_wgpu](https://github.com/joobert/ssbh_wgpu) (`6702cb4`), which adds `ModelFolder::load_files` and `apply_anims_with_skel_edit` on top; `serde_yaml` added for `motion_list.yml`
- New ignored `moveset_survey` test runs the source indexer over a folder of moveset projects (`VISIONARY_MOVESETS`, `VISIONARY_SURVEY_ONLY`, `VISIONARY_SURVEY_PLAN`)
- The single-root carrier builder is gated to tests
- README documents moveset styles, frame data, Build & Deploy, the viewport toolbar, Settings, costume loading, model checks, the Motion List editor and slot characters

### Known limits

- Played mostly on Mario, Cloud, Hero, Joker, Little Mac, Inkling, Steve, Villager, Rosalina, Ice Climbers and Donkey Kong; most other fighters' mechanics aren't recreated
- A Grab box block alone doesn't catch anyone (use Go to another action → catch); Villager's pocket storing and firing, Steve's mining and tool breaking, and Isabelle are untested
- Some effects may not match the game exactly; throws were checked only roughly
- Villager's Lloid Rocket and Steve's minecart aren't drawn
- The borrow question can't yet switch effects inside a facing check or spawned by an object (Mario's fireball); it lists those moves
- Rare blocks (Final Smash cameras, hitbox size, reaction time, shake) don't update live yet and are named in the top bar

## 0.3.1

- **Character and Move pickers** on the Effects page — searchable (Enter picks the first match); the move list is grouped like the main window's and marks moves that play the selected effect; picking a move lists only its effects ("Only in <move>") and shows one in it
- **Kept edits** — each load takes the project's edits for the file it opens, so switching files, pages or borrowing no longer shows effects unedited, and saving no longer writes that empty diff over the project. A loaded project is marked unsent
- **Kept picks** — each file reopens on its last effect and part, the move list opens on the open move, and part fields keep their scroll position per effect and part
- **Borrowed uses** — spawns a project adds or redirects count as uses, so a borrowed effect isn't greyed out and can play in those moves; a spawn pointed at a borrowed copy, or removed, no longer counts for the original
- **Borrow question** — keyed by the move's name instead of its script name, so redirects reach forward smash; moves that play the effect inside a facing check or from an object are listed as not switchable yet
- **Borrowing UI** — plainer words, a larger replacement list, "Borrowed from" with Remove, the window comes back in front, and its list is no longer rebuilt every frame
- **Layout** — the timeline keeps its height across pages and moves and stays (with a note) while an effect plays alone; the file search is focused and cleared; the pre-edit colour is a narrow swatch that restores it
- The Tools menu closes when one of its windows is toggled

## 0.3.0

- **Effects page** — edit an effect's look with the preview updating as you go; play it alone or in a move that spawns it, and send it to a connected game. Colours take a picker, a colour code (#FF8800) or exact numbers, and each spawn's colour, opacity and rate can be changed from the move
- **Game-accurate effects** — the game's shaders, textures, glow and colours; effects follow bones, fade when killed and are hidden behind the fighter
- **Objects** — projectiles and their flight, returning boomerangs, held and crafted weapons, items, Link's bow, Inkling's roller and squid, and the grab/throw opponent
- **Weapon swing trails** and a **stage picker** — moves play on the stage, aerials above the ground, ledge moves on the ledge
- **Camera** — right or middle drag orbits the fighter (#5), double-click returns to the side view
- **Fixes** — the borrow window can move to another monitor (#32); new effects can be added to moves with no effect script, making the function in the linked source (#35); a failed live read no longer panics ARCropolis (#38)
- **Self-contained exports** — exported mods build their own plugin named after the mod and don't need Visionary; live editing, Export mod and Export source code no longer mix files
- **Script-less moves** — editable, and the script a move really runs is the one loaded
- **Navigation** — one top bar for Classic editor, Move nodes, Gimmicks, Effects and Characters; searchable, grouped pickers for fighters, moves, bones and actions
- **Blocks** — plainer words, blocks can show their own code, a Code block; cancel, slow down and remove blocks work, and every block updates live. Gimmicks can run on chosen costumes only
- **New characters** — start from the donor's model and animations

## 0.2.1

- **Fighter-scoped live edits** — rules matched only the move name, so editing Cloud's neutral special also changed Joker's; edits now stay on their fighter
- **Resources on hit** — Cloud's Limit, Hero's MP, Joker's Rebel Gauge, Little Mac's KO meter (a full one gives the real KO Uppercut), Inkling's ink
- **Partner control** — Nana's neutral special, Nana's and Luma's damage, and Luma's state (dead removes her); attack, jump, shield and stand still are listed but only the neutral special was played. Nana's hits are no longer ignored
- **Steve** — hits earn materials and upgrade a tool's tier
- **Grabbing moves** — Go to another action set to catch (tested on Donkey Kong's side special)
- **Values and calls** — every fighter's game values are in the block list; about 45 more game calls update live
- **Gimmicks** — adding a rule or Tell a gimmick makes the gimmick when it's missing; plainer words in rules, actions and object pickers
- **Graph** — Fit shows a whole move, new cards don't overlap, the action search box works
- **Fix** — a line in the game's own script blocked every edit to Inkling's jab

## 0.2.0

Prerelease of the node system.

- **Move nodes tab** — build a move slot out of blocks, empty or from a starter (Quick strike, Heavy hit, Lunge forward and hit, Shoot a projectile), with a 3D preview beside the graph and a plain-words read-back of the whole move
- **If blocks** — check being on the ground, a held button or a gimmick value while the move runs
- **Character gimmicks** — always-on rules (on dealing or taking damage, timers, meters) or ready-made ones like extra jumps, and moves can talk to them
- **Live updates** — node edits reach the match without apply, restart or export, and new hitboxes don't need the move done first
- **Exports without Rust** — the mod carries the rules tested live plus the plugin; with Rust and the plugin source it still compiles. Install only one plugin copy: an exported mod has its own `plugin.nro`
- Planning docs and research scripts removed from the repo
