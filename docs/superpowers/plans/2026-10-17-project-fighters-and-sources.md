# Project Fighters and Sources Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the left panel's fighter list lead with the open project's fighters, make the project's `romfs/` override the dump in previews, keep the moveset link per project, and gather every input into one Sources window with a first-run dump check.

**Architecture:** Pure logic goes in two new modules, `src/project_scope.rs` (membership, romfs scan and overlay, moveset path portability) and `src/sources.rs` (dump validation), tested without a GUI like `project_hub.rs`. `modproject.json` gains two fields (v4). `src/app.rs` (`VisionaryApp`) wires the state in and draws the UI, following the existing pattern of UI in `app.rs` and logic in modules.

**Tech Stack:** Rust 2021, egui, serde/serde_json, `tempfile` (already a dependency) for tests.

**Spec:** `docs/superpowers/specs/2026-10-17-project-fighters-and-sources-design.md`

## Global Constraints

- `PROJECT_VERSION` becomes `4`; `OLDEST_SUPPORTED_VERSION` stays `1`. Every new field is `#[serde(default)]` so v1–v3 load unchanged.
- The dump stays global (`data_root` config). The Mod Library stays global (`mod_roots`). Only the moveset link moves into the project.
- Precedence: a file in the project's `romfs/` wins over the dump and reference mods. Between dump and reference mods, today's first-root-wins rule is unchanged.
- With no project open ("Browse without project"), the fighter list and the global `acmd_src_root` link behave exactly as today.
- An edited fighter is never hidden: any fighter with edits is added to `project_fighters`, and stays after the edit is undone.
- An absolute `moveset_source` is never written into a `modproject.json` meant for distribution. No current writer does that (`export_mod` writes none), so this is a doc comment on `make_portable_project`, not a code path.
- User-facing text says "ArcExplorer dump" / "game dump". The strings "Data Root", "data root" and "base game folder" are removed from UI text (identifiers and config keys stay).
- Commits carry no `Co-Authored-By` line.
- Build: `cargo build`. Tests: `cargo test <filter>` from the repo root.

## Review Focus

1. **Partial romfs copy.** `romfs/fighter/mario/model/body/c00/` holds only `model.numdlb` and nothing else. The model still renders (missing files fall back to the dump through `CostumeFolder`), and the move list still loads. Pinned in Task 2 (`overlay_keeps_dump_paths_for_missing_parts`).
2. **Empty or malformed romfs.** `romfs/fighter/` doesn't exist, or holds a stray file or a non-`cNN` folder. Membership and the overlay ignore it without panicking. Pinned in Task 2 (`romfs_scan_ignores_junk`).
3. **Project names a fighter the dump lacks.** `project_fighters` lists `mario` but no dump is set, or Mario was forgotten. The In this project group skips it quietly, and the entry is not dropped from the file. Pinned in Task 2 (`membership_ignores_unknown_rows`) and Task 5 (save round-trip).
4. **A moveset crate that moved or was deleted.** The project opens anyway, Moveset code shows "could not re-open …", and the stored path is kept. Pinned in Task 5 (`stale_moveset_path_is_kept`).
5. **Picking a folder above or inside the dump.** For example `ArcExplorer export/` versus `ArcExplorer export/fighter/`. The first-run check offers the right folder. Pinned in Task 4 (`suggests_parent_when_fighter_folder_picked`, `suggests_child_dump`).

---

### Task 1: Project file v4 — `project_fighters` and `moveset_source`

**Files:**
- Modify: `src/mod_project.rs` (struct `ModProjectFile` ~line 41, `Default` ~190, `is_empty` ~196, `migrate` ~219, `scope_acmd_project` ~80, tests module ~867)

**Interfaces:**
- Produces:
  - `#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)] pub struct ProjectFighter { pub name: String, #[serde(default, skip_serializing_if = "Option::is_none")] pub character: Option<String> }`. `character` is the slot character's display name (`FighterEntry::custom_name`). `None` is the fighter's own row.
  - `ProjectFighter::fighter(name: &str) -> Self` and `ProjectFighter::character(name: &str, character: &str) -> Self`
  - `ModProjectFile.project_fighters: BTreeSet<ProjectFighter>` (`#[serde(default, skip_serializing_if = "BTreeSet::is_empty")]`)
  - `ModProjectFile.moveset_source: Option<String>` (`#[serde(default, skip_serializing_if = "Option::is_none")]`). In memory this is always an absolute path string. Task 3 makes it portable on write.
  - `FighterMod::has_edits(&self) -> bool`: the per-fighter predicate currently inlined in `is_empty`, which then calls `!f.has_edits()`
  - `ModProjectFile::seed_project_fighters(&mut self)`: inserts `ProjectFighter::fighter(name)` for every `fighters` entry with `has_edits()`, and `ProjectFighter::character(&e.donor, &e.display_name)` for every `roster.authored` entry. Additive and idempotent.
- `is_empty` deliberately ignores both new fields: a project that only names fighters has nothing to export.
- `scope_acmd_project` copies neither field into the scoped view. They aren't ACMD.

- [ ] **Step 1: Write failing tests** in the `#[cfg(test)]` module of `src/mod_project.rs`:
  - `v3_project_loads_with_empty_scope`: deserialize `{"version":3,"name":"x"}`, `migrate()`. Then `version == 4`, `project_fighters.is_empty()`, `moveset_source.is_none()`.
  - `project_fighters_round_trip`: insert `fighter("mario")` and `character("pacman","Shy Guy")`, serialize, deserialize, compare equal. The JSON for the first has no `character` key.
  - `seed_adds_edited_and_authored`: fighter `fox` with a non-empty `acmd`, fighter `falco` with a default (empty) `FighterMod`, and one authored entry donor `pacman`, display `Shy Guy`. Seeding gives exactly `{fighter("fox"), character("pacman","Shy Guy")}`. Seeding twice gives the same set.
  - `scope_only_project_is_empty`: a project with only `project_fighters` and `moveset_source` set has `is_empty() == true`.
- [ ] **Step 2:** Run `cargo test mod_project::tests`. Expected: FAIL to compile (missing items).
- [ ] **Step 3:** Implement the items above, and bump `PROJECT_VERSION` to `4`. Update `migrate`'s doc comment to mention v4.
- [ ] **Step 4:** Run `cargo test mod_project`. Expected: PASS, including existing tests.
- [ ] **Step 5:** Commit: `git commit -m "Record a project's fighters and moveset link in modproject.json"`

---

### Task 2: `project_scope` — romfs scan, overlay, membership

**Files:**
- Create: `src/project_scope.rs`
- Modify: `src/main.rs` (add `mod project_scope;` alphabetically)

**Interfaces:**
- Consumes: `ProjectFighter` (Task 1), `crate::data::FighterEntry`
- Produces:
  - `pub fn romfs_fighter_slots(romfs: &Path) -> BTreeMap<String, BTreeSet<u8>>`: for each `romfs/fighter/<name>/{model,motion}/<part>/cNN` folder holding at least one regular file (anywhere below it), the fighter's name maps to slot `NN`. Names are lowercased. Skips symlinks, like `merge_romfs_overlay` does. A missing `romfs` gives an empty map.
  - `pub fn apply_romfs_overlay(entry: &mut FighterEntry, romfs: &Path)`: for the entry's base slot `cNN = format!("c{:02}", entry.base_slot())`, set `model_dir` to `romfs/fighter/<name>/model/body/cNN` when that folder exists, and the same for `motion_dir`. Set `param_path` to `romfs/fighter/<name>/param/vl.prc` or `fighter_param.prc` when either exists, and `effect_dir` to `romfs/effect/fighter/<name>` when it exists. Leaves each field alone otherwise. It never touches `fighter_dir`, `slots`, `source` or the name fields.
  - `#[derive(Debug, Clone, Copy, PartialEq, Eq)] pub enum MembershipReason { Added, RomfsFiles, MovesetScripts }`. "Has edits" isn't a reason here: Task 5 turns edits into `Added`.
  - `pub struct MembershipInputs<'a> { pub added: &'a BTreeSet<ProjectFighter>, pub romfs_slots: &'a BTreeMap<String, BTreeSet<u8>>, pub moveset_fighters: &'a BTreeSet<String> }`
  - `pub fn membership(entry: &FighterEntry, all_rows: &[FighterEntry], inputs: &MembershipInputs) -> Vec<MembershipReason>`, with reasons in the enum's order. The row key is `ProjectFighter { name: entry.name, character: entry.custom_name }`.
    - `Added`: the key is in `added`.
    - `RomfsFiles`: `romfs_slots[name]` intersects `entry.slots`. Or, for a fighter's own row only (`custom_name == None`), it contains a slot that no row in `all_rows` with that `name` owns.
    - `MovesetScripts`: `moveset_fighters` contains `name`, and the row is the fighter's own row.
  - `pub fn moveset_fighters(index: &crate::acmd_src::SourceIndex) -> BTreeSet<String>`: the keys of `index.fighters`, excluding `""` (unlabelled crates count for nothing).
  - `pub fn reason_label(reason: MembershipReason) -> &'static str`: `"added to project"`, `"has files in romfs/"`, `"moveset has scripts"`.

- [ ] **Step 1: Write failing tests** in `src/project_scope.rs` `#[cfg(test)] mod tests`. Use `tempfile::tempdir()` and a helper `entry(name, custom, slots) -> FighterEntry` with dump-style paths.
  - `romfs_scan_finds_model_and_motion_slots`: files at `fighter/mario/model/body/c00/model.numdlb` and `fighter/mario/motion/body/c03/a.nuanmb` give `{"mario": {0,3}}`.
  - `romfs_scan_ignores_junk`: a missing romfs gives an empty map. `fighter/readme.txt`, `fighter/mario/model/body/notaslot/x`, and an empty `fighter/luigi/model/body/c00/` give an empty map.
  - `overlay_redirects_existing_parts`: a romfs with `model/body/c00/x` and `param/vl.prc`. After the overlay, `model_dir` and `param_path` point into romfs, `motion_dir` is unchanged, and `fighter_dir` is unchanged.
  - `overlay_keeps_dump_paths_for_missing_parts`: an empty romfs leaves the entry `==` field-for-field unchanged (compare each path).
  - `membership_by_row`: rows `mario` (slots 0..8) and `pacman`/`Shy Guy` (slots 40..48). With `added = {character("pacman","Shy Guy")}`, Shy Guy gets `[Added]` and the plain `pacman` row gets `[]`.
  - `romfs_slot_matches_owning_row`: romfs slots `pacman: {40}` mark Shy Guy `RomfsFiles` but not plain `pacman`. Romfs slots `pacman: {99}` (no row owns 99) mark plain `pacman` `RomfsFiles`.
  - `moveset_marks_own_row_only`: `moveset_fighters = {"pacman"}` marks plain `pacman`, not Shy Guy.
  - `membership_ignores_unknown_rows`: `added` contains `fighter("ghost")` and no row is named `ghost`. Every row returns `[]`, and the call doesn't panic.
  - `unlabelled_moveset_counts_for_nothing`: build a `SourceIndex` whose `fighters` has only the key `""` (use whatever test constructor `acmd_src.rs` tests already use, or `SourceIndex::build` on a temp crate with an unlabelled `game_` fn). `moveset_fighters` gives an empty set.
- [ ] **Step 2:** Run `cargo test project_scope`. Expected: FAIL to compile.
- [ ] **Step 3:** Implement the interfaces above. Slot parsing: a folder name `c` followed by exactly two or more ASCII digits that parse as `u8`.
- [ ] **Step 4:** Run `cargo test project_scope`. Expected: PASS.
- [ ] **Step 5:** Commit: `git commit -m "Work out which fighters a project is about, and overlay its romfs"`

---

### Task 3: Portable `moveset_source`

**Files:**
- Modify: `src/project_scope.rs` (add functions)
- Modify: `src/mod_export.rs` (`make_portable_project` ~317, `resolve_project_assets` ~435)

**Interfaces:**
- Produces:
  - `pub fn store_moveset_path(crate_root: &Path, project_dir: &Path) -> String`: when `crate_root` is under `project_dir`, the path relative to it with `/` separators; otherwise the absolute path as given.
  - `pub fn resolve_moveset_path(stored: &str, project_dir: &Path) -> PathBuf`: an absolute `stored` is returned as is; otherwise `project_dir.join(stored)`.
- `make_portable_project(project, project_dir, …)`: if `moveset_source` is `Some(abs)`, it writes `store_moveset_path(abs, project_dir)`. This covers autosave, Save As, export-editable and import, because every one writes through `write_portable_project`. Save As to a folder outside the crate's tree therefore stores an absolute path, as the spec requires. Add the doc comment from Global Constraints about distribution copies.
- `resolve_project_assets(project, project_path)`: if `moveset_source` is `Some(s)`, replace it with `resolve_moveset_path(s, parent).display().to_string()`.

- [ ] **Step 1: Write failing tests.**
  - In `project_scope` tests: `inside_crate_is_relative` (`/w/proj` with crate `/w/proj/moveset` gives `"moveset"`), `outside_crate_is_absolute`, and `resolve_round_trips` (store then resolve returns the original for both cases). Build paths from `tempdir()` so they're real absolute paths on Windows.
  - In `mod_export` tests: `portable_project_relativises_moveset`. `make_portable_project` with `moveset_source = <dir>/moveset` and `project_dir = <dir>` gives `Some("moveset")`. `resolve_project_assets` on that result with `<dir>/modproject.json` brings back the absolute path.
- [ ] **Step 2:** Run `cargo test moveset`. Expected: FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run `cargo test project_scope mod_export`. Expected: PASS.
- [ ] **Step 5:** Commit: `git commit -m "Store a project's moveset link relative to the project when it can"`

---

### Task 4: `sources` — dump validation

**Files:**
- Create: `src/sources.rs`; modify `src/main.rs` (`mod sources;`)

**Interfaces:**
- Produces:
  - `#[derive(Debug, Clone, PartialEq, Eq)] pub enum ModHint { InfoToml, Plugin, RomfsWrapper, FewFighters(usize), NotVanilla }`
  - `#[derive(Debug, Clone, PartialEq, Eq)] pub enum DumpCheck { Ok, Missing { fighter: bool, effect: bool, suggestion: Option<PathBuf> }, LooksLikeMod(Vec<ModHint>) }`
  - `pub fn check_dump(dir: &Path) -> DumpCheck`:
    - `Missing` when `dir/fighter` or `dir/effect` isn't a directory. `suggestion` is set when `dir`'s parent has both (the user picked `fighter/` itself), or when exactly one child of `dir` has both (they picked the folder above).
    - Otherwise, collect hints: `InfoToml` if `dir/info.toml` exists; `Plugin` if any `*.nro` exists within depth 3; `RomfsWrapper` if `dir` or a child is named `romfs`; `FewFighters(n)` if `dir/fighter` has fewer than `FEW_FIGHTERS = 20` subfolders; `NotVanilla` if `dir/<CARRIER_BASE_REL>` (`crate::eff_editor::CARRIER_BASE_REL`) isn't a file. No hints gives `Ok`, otherwise `LooksLikeMod(hints)`.
  - `pub fn hint_text(hint: &ModHint) -> String`: plain sentences, e.g. `"has an info.toml, which mods ship and dumps don't"`, `"holds only 3 fighters — a full dump has about 90"`.
  - `pub const FEW_FIGHTERS: usize = 20;`

- [ ] **Step 1: Write failing tests** (tempdir fixtures):
  - `vanilla_like_dump_is_ok`: 20 fighter subfolders, `effect/`, and the carrier-base file give `Ok`.
  - `mod_folder_flags_hints`: `fighter/mario`, `effect/`, `info.toml`, `plugin.nro` give `LooksLikeMod` containing `InfoToml`, `Plugin`, `FewFighters(1)` and `NotVanilla`.
  - `suggests_parent_when_fighter_folder_picked`: checking `<d>/fighter` (where `<d>` is a valid dump) gives `Missing { suggestion: Some(<d>), .. }`.
  - `suggests_child_dump`: checking `<p>` where `<p>/export/` is a valid dump gives `suggestion: Some(<p>/export)`.
  - `empty_folder_is_missing_without_suggestion`.
- [ ] **Step 2:** Run `cargo test sources`. Expected: FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run `cargo test sources`. Expected: PASS.
- [ ] **Step 5:** Commit: `git commit -m "Check that a folder is an ArcExplorer dump before using it"`

---

### Task 5: Wire project scope into the app

**Files:**
- Modify: `src/app.rs`. Relevant spots: `VisionaryApp` fields (~3056) and init (~3600); startup ACMD relink (~3733); `all_roots` (3787); `reindex_fighters` (3865); `link_acmd_source` (4870); `unlink_acmd_source` (4976); `build_project` (18012); `new_project_in` (18523); `import_mod_folder` (18602); `load_project_from` (20033); the `BrowseWithoutProject` hub action handler.

**Interfaces:**
- Consumes: Tasks 1–3.
- Produces (fields on `VisionaryApp`, used by Tasks 6–7):
  - `project_fighters: BTreeSet<ProjectFighter>`
  - `project_moveset: Option<PathBuf>`: the project's crate (absolute). `None` means not linked.
  - `project_romfs_slots: BTreeMap<String, BTreeSet<u8>>`: recomputed in `reindex_fighters`
  - `fn project_workspace(&self) -> Option<PathBuf>`: `project_path`'s parent
  - `fn row_membership(&self, idx: usize) -> Vec<MembershipReason>`: empty when no project is open
  - `fn add_to_project(&mut self, idx: usize)` / `fn remove_from_project(&mut self, idx: usize)`: edit `project_fighters` by row key, then let autosave pick it up (they change what `build_project` returns, so the existing dirty check sees it)

**Behaviour:**
- `all_roots()` appends `workspace/romfs` last when a project is open and that folder exists. The dump and mods keep owning fighters they define (first wins); the romfs root only adds fighters and slots. Then, at the end of `reindex_fighters`, call `apply_romfs_overlay` on every entry and refresh `project_romfs_slots`.
- `new_project_in`, `load_project_from` and `import_mod_folder` call `reindex_fighters()` after adopting the path, because the romfs root changed. So does the `BrowseWithoutProject` action when it clears the project.
- `load_project_from`: `project.seed_project_fighters()`, then take `project_fighters`. For the moveset: `Some(path)` gives `link_acmd_source(path)` in project mode. If the build fails, keep `project_moveset = Some(path)` and set `acmd_src_problem` to `"could not re-open {path}: {error}"`. Never drop the stored path. `None` gives `acmd_src = None` with no problem text.
- `import_mod_folder`: before `write_portable_project`, insert `ProjectFighter::fighter(name)` into the imported project's `project_fighters` for every key of its `fighters` map, edits or not. Authored entries are covered by the seeding in `load_project_from`.
- `new_project_in`: `project_fighters` and `project_moveset` start empty.
- `build_project` writes `project_fighters` and `moveset_source = project_moveset.map(display)`. It then inserts `ProjectFighter::fighter(name)` into `self.project_fighters` for every built fighter with `has_edits()`, so edits add membership and it persists after an undo. Insert into both the returned project and `self`.
- `link_acmd_source(root)`: when a project is open, set `project_moveset = Some(root)` and **do not** call `save_config_path(ACMD_SRC_CONFIG_KEY, …)`. Otherwise keep today's behaviour. `unlink_acmd_source` follows the same split, with `project_moveset = None` in project mode.
- `BrowseWithoutProject` re-links from `load_config_path(ACMD_SRC_CONFIG_KEY)`, as startup does today.
- Startup still relinks the global source first. Resuming a project then replaces it through `load_project_from`.

- [ ] **Step 1: Write failing tests** in a new `#[cfg(test)] mod project_scope_wiring` in `app.rs`, using whatever headless `VisionaryApp` test constructor the existing app tests use (search `fn test_app` / `VisionaryApp::new_for_test` near the existing `#[cfg(test)]` blocks; reuse it, don't add another):
  - `editing_adds_fighter_and_survives_undo`: give fighter `fox` an ACMD edit, `build_project()`, and `fighter("fox")` is in `project_fighters`. Clear the edit, `build_project()` again, and it's still there.
  - `project_fighters_round_trip_through_save`: set `project_fighters = {fighter("ghost")}` (not indexed), save, load, and it's still there.
  - `stale_moveset_path_is_kept`: load a project whose `moveset_source` is a path that doesn't exist. Afterwards `project_moveset == Some(that)`, `acmd_src_problem.is_some()`, and the next `build_project().moveset_source` is unchanged.
  - `linking_in_project_mode_leaves_global_alone`: with a project open, `link_acmd_source(tmp_crate)` leaves `load_config_path(ACMD_SRC_CONFIG_KEY)` unchanged. If config writes aren't isolable in tests, assert through a `fn link_target(&self) -> LinkTarget { Project, Global }` helper and test that instead.
- [ ] **Step 2:** Run `cargo test project_scope_wiring`. Expected: FAIL.
- [ ] **Step 3:** Implement the behaviour above.
- [ ] **Step 4:** Run `cargo test`. Expected: all PASS. Run `cargo build`. Expected: no new warnings.
- [ ] **Step 5:** Manual check: open a project with an edited `romfs/fighter/<f>/model/body/c00/`. The viewport shows the edited model. Opening a different project re-links (or unlinks) the moveset.
- [ ] **Step 6:** Commit: `git commit -m "Scope the fighter list, previews and moveset link to the open project"`

---

### Task 6: Left panel — "In this project" / "Everything else"

**Files:**
- Modify: `src/app.rs` `draw_left_panel` (9076), New Project flow (`new_project_in` caller in the hub and File menu)

**Behaviour:**
- With no project open, the list is unchanged.
- With a project open, the Fighters scroll area shows two `CollapsingHeader`s:
  - `"In this project (N)"`: default open, id `("fighter_group","project")`, rows where `row_membership` isn't empty.
  - `"Everything else (M)"`: default closed, id `("fighter_group","rest")`.
  - The search filter applies inside both. While searching, force-open any group with matches, the way the move list handles categories (reuse `persistent_collapsing_open` / `set_persistent_collapsing_open`).
- Row hover text on project rows appends `"In project: " + reasons joined with ", "` (from `reason_label`).
- Context menu, above the existing "Cached fighter data" section:
  - Everything else rows get **Add to project**.
  - Project rows get **Remove from project**. It's enabled only when `Added` is a reason. When other reasons remain, it gets the hover text `"Still in the project because it {reasons}"`, and after removal the row stays in the group.
- Under the In this project group, an **Add fighter…** small button opens a popup with a search field and the Everything else rows. Clicking one calls `add_to_project`.
- When the group is empty, show weak text `"No fighters yet — use Add fighter… or right-click one below."`
- New Project: after `new_project_in` succeeds and fighters are indexed, open the same Add fighter popup with the title `"Which fighters is this project for?"`. The user can multi-click and close it to skip.

- [ ] **Step 1:** Implement.
- [ ] **Step 2:** Run `cargo build`. Expected: OK. Run `cargo test`. Expected: PASS.
- [ ] **Step 3:** Manual check against the Review Focus items: search across both groups, add, remove (both with another reason still holding the row and without), a slot character added separately from its fighter, and Browse without project unchanged.
- [ ] **Step 4:** Commit: `git commit -m "Lead the fighter list with the open project's fighters"`

---

### Task 7: Sources window

**Files:**
- Modify: `src/app.rs`: new field `show_sources: bool` and `sources_focus: Option<SourceRow>`, new `fn draw_sources_window(&mut self, ctx)`, called beside `draw_project_hub`; File menu (~33966); left-panel button; ACMD "Link…" (~7765).

**Interfaces:**
- Produces: `#[derive(Clone, Copy, PartialEq, Eq)] enum SourceRow { Dump, ReferenceMods, ProjectFiles, MovesetCode }` and `fn open_sources(&mut self, row: SourceRow)`.

**Behaviour:** one `egui::Window::new("Sources")` with four rows, in this order and with this copy:
1. **Game dump (ArcExplorer)**: "The fighter/, effect/ and ui/ folders you exported from data.arc. Shared by every project."
   - Status: the path, plus `check_dump` turned into text (Ok / missing parts / each `hint_text`).
   - Actions: **Change…** (folder picker, then `check_dump`; on `Missing{suggestion: Some(p)}` offer "Use {p} instead"; on `LooksLikeMod`, show the hints with **Use anyway** / **Pick another**; then `set_data_root`) and **Open folder**.
2. **Reference mods**: "Other people's mods to view alongside the game. Shared by every project."
   - Status: `"{enabled} of {total} enabled"`.
   - Action: **Open Mod Library** (`self.roster.open = true`).
3. **This project's files**: "Your edited game files (romfs/). These win over the dump in previews and are included when you export." Shown only when a project is open; otherwise "Open or create a project to have its own files."
   - Status: `"{n} fighters with files"` from `project_romfs_slots`.
   - Actions: **Open folder**, **Project Files…** (`show_workspace = true`).
4. **Moveset code**: "The Rust project that builds your plugin. Saved with this project." Without a project the description reads "…Used while browsing without a project."
   - Status: linked path / `acmd_src_problem` / "not linked"; when the crate is unlabelled (`has_unattributed_scripts`), add "this crate doesn't name its fighters — add the fighter to the project yourself".
   - Actions: **Link…** and **Unlink**. In project mode with no link but a global `acmd_src_root` set, also **Use {global path}**.
- `sources_focus` scrolls to and highlights its row the first frame after `open_sources`.
- Rerouting:
  - File → **Choose base game folder…** becomes **Sources…** (`open_sources(Dump)`).
  - The ACMD window's **Link…** calls `open_sources(MovesetCode)`.
  - The left panel's no-dump button is replaced in Task 8.
  - Rename user-facing "Data Root" / "base game folder" strings across `app.rs` (`grep -n "Data Root\|data root\|base game folder" src/app.rs`, UI strings only) to "game dump". One example is the Project Files status `"Choose a base game folder from File first."`, which becomes `"Set your game dump in File → Sources first."`.

- [ ] **Step 1:** Implement.
- [ ] **Step 2:** Run `cargo build` and `cargo test`. Expected: OK / PASS.
- [ ] **Step 3:** Manual check: each row's action works; picking `…/fighter` offers the parent; picking a mod folder shows the hints; and in project mode linking leaves the global link alone (re-open Browse without project and the old link is back).
- [ ] **Step 4:** Commit: `git commit -m "Gather the dump, mods, project files and moveset into one Sources window"`

---

### Task 8: First-run setup, README and changelogs

**Files:**
- Modify: `src/app.rs` `draw_left_panel` no-dump branch (9077–9087); `README.md` "Game data" section; `CHANGELOG.md`, `CHANGELOG_DETAILED.md`, `CHANGELOG_fr.md` (follow each file's existing voice and level of detail)

**Behaviour:**
- When `state.data_root.is_none()`, the left panel shows:
  - the heading "Step 1 — your game dump"
  - a two-line explanation (the folder ArcExplorer exported from `data.arc`, holding `fighter/` and `effect/`)
  - a **Choose game dump…** button that runs the same pick, check and confirm flow as the Sources Dump row (factor it into `fn pick_game_dump(&mut self)` and use it in both places)
- After a dump is accepted on first run (no project open and no recent projects), set `project_hub.show = true` as step 2.
- README "Game data": name the Sources window, the four layers, and the precedence rule (project romfs wins; between the dump and mods, the dump stays authoritative). Say that the moveset link is saved per project.

- [ ] **Step 1:** Implement the UI and `pick_game_dump`.
- [ ] **Step 2:** Run `cargo build` and `cargo test`. Expected: OK / PASS.
- [ ] **Step 3:** Manual check with the `data_root` config file removed: the guided step shows, a mod folder gets warned about, a valid dump leads to the Project Hub.
- [ ] **Step 4:** Update the README and the three changelogs.
- [ ] **Step 5:** Commit: `git commit -m "Guide first run to the game dump and document Sources"`
