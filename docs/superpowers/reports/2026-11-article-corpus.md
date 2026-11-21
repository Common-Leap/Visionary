# Article corpus sweep, November 2026

The acceptance check for article support: every moveset under `Movesets/` opened the way the app
opens a linked moveset (`SourceIndex::build`, then `articles::catalog_for`, `settings_for`,
`param_overrides_for`, `view::status_call_rows`), with every article listed, every id resolved
where it can be, every setting read with its origin, and every script with article calls put
through a byte round trip.

Run it with

```text
VISIONARY_MOVESETS="C:/Users/notja/Documents/Coding/SSBU Modding/Movesets" cargo test --bin visionary corpus_sweep -- --ignored --nocapture
```

`VISIONARY_DUMP` names the ArcExplorer dump; unset, the data root Visionary saved in its config is
used (here `D:\Ultimate-Unleashed\dev\tools\files\ArcExplorer\export`). The sweep only reads the
movesets and the dump: its round trips run on text in memory and write nothing. Test:
`src/articles/corpus_sweep.rs`, `corpus_sweep_reports_every_article`; it writes this report's
detailed part to `target/article-corpus.md`.

## Result

**35 movesets swept. 0 round-trip differences, 0 misattributions, 0 refused edits.** 621
scripts with article calls, in 205 files, edited one call at a time, saved, then put back: every save
touched only the edited statement, and every revert restored the file byte for byte (CRLF files
included).

First run: 8 hard failures (an article listed twice under one owner: Latios ×4, Dark Bowser ×2,
Empoleon, HDR) and 1543 residue items, 54 of them refused edits. Final run: 0 failures, 1333
residue items, none of them refused edits.

### What the round trip checks

For every script on a fighter or one of its articles whose body has article calls, read from the
span a save edits (a forwarding wrapper's callee, as `sync_script` does):

1. Each unedited call emits its statement as written.
2. A save with no edits leaves the body unchanged.
3. Each call, alone, has one argument changed (its first non-target argument, else its article),
   is saved through `rewrite_editor_article_calls`, and the result may differ from the original
   only inside that call's statement. The changed file is then read back and the value put back:
   the body must equal the original byte for byte. (The revert takes the saved file as the
   editor's list, as after a reload; an undo inside one editor session after a save is still
   refused, the known Task 8 limitation.)

A refusal (the save declines and says why) is residue, not a failure; a byte outside the edited
statement, an edit not written, or a revert that does not restore the file is a failure.

### Misattribution checks

Ids are resolved with `ArticleCatalog::resolve_id` and no locals, as the app's timeline labels
do. A failure is any of:

- an agent listed in two catalogs, or twice in one;
- a clone listed under a fighter its `clone_weapon` does not name as the new owner;
- an id the catalog answers to, or a resolved id in a script or status line, shaped
  `FIGHTER_<X>_GENERATE_ARTICLE_*` (or `WEAPON_<X>_<ARTICLE>_GENERATE_ARTICLE_*`) where `X` is a
  fighter other than the catalog's. A name that is no fighter (`FIGHTER_KINGTERESA_…`) is the
  moveset's own and passes; Kirby and the echo/semi-clone pairs (samusd/samus, pichu/pikachu,
  richter/simon, falco/fox, shizue/murabito, daisy/peach, lucina/marth, chrom/roy, ken/ryu,
  pitb/pit) are listed under "Ids that name another fighter on purpose" instead;
- an article script the index puts on `<owner>_<name>` that is neither registered by that agent
  in a file naming its function, nor in a file or nearest agent-naming folder that names one of
  the owner's agents.

`corpus_sweep::tests::wrong_attributions_fail_the_checks` gives each check a deliberate wrong
attribution and shows it fails. On the corpus all four pass: 0 failures.

### Fighter names

The owner of an agent is found among: every vanilla fighter (`data::VANILLA_FIGHTERS`), the dump's
`fighter/` folders, and the moveset's own `romfs/fighter/` folders. The app lists only fighters
it can load from its roots, so with this partial dump a few owners would not be selectable there
at all; using the full name list here lets the sweep still attribute their articles.

## Fixes made from the residue

| # | Shape (corpus line) | Fix | Regression test | Commit |
|---|---|---|---|---|
| 1 | A clone's model folder shipped under its new owner listed as a vanilla article *and* as the clone (Latios `samus/model/lativolt`, Dark Bowser, Empoleon `falco/model/cannon`) — the first run's hard failures | A folder a clone of this fighter adds is the clone's | `articles::tests::a_clones_own_folder_is_not_a_vanilla_article` | 1281ef4 |
| 2 | The same `clone_weapon` declared in two compiled crates (HDR `ports/fighters/packun/src/lib.rs:102`, `vendor/hdr/fighters/packun/src/lib.rs:101`) — hard failure | One article; the other calls kept on it (`Article::also_declared`) and shown in its clone notes | `articles::tests::the_same_clone_declared_twice_is_one_article` | 1281ef4 |
| 3 | Vanilla ids spelled differently from their folder: `FIGHTER_MARIO_GENERATE_ARTICLE_HUGE_FLAME` for `hugeflame` (Iron-Man `final_proton.rs:64`), `FIGHTER_MURABITO_GENERATE_ARTICLE_BOWLING_BALL` (Toad) | Match the game's own articles ignoring underscores, when exactly one matches | `articles::tests::resolves_the_games_own_spellings` | 1281ef4 |
| 4 | An article's id for its owner's other articles, `WEAPON_MURABITO_SLINGSHOT_GENERATE_ARTICLE_BULLET` (Toad `ground.rs:231`) | `WEAPON_<OWNER>_<X>_GENERATE_ARTICLE_<NAME>` resolves to the owner's `<NAME>` | same test | 1281ef4 |
| 5 | A bare old-API `clone_weapon(..)` with its id written as a const, `FIGHTER_MASTER_GENERATE_ARTICLE_PLASMASHOT:i32 = 0x9` (Master Chief `weapons/mod.rs:7`, `consts.rs:38`) | Bind a clone with no id to the project's `FIGHTER_<OWNER>_GENERATE_ARTICLE_<NAME>` const or static | `articles::clones::tests::bare_call_binds_the_conventional_const` | 7e6ad8a |
| 6 | Identical calls in one frame (Cuphead `specials.rs:78-96`, four identical bullets; HDR dedede, Potemkin, Sackboy): every edit refused as "could be more than one" — 54 refusals | While the file holds the calls exactly as loaded, pair them by position | `acmd_src::tests::identical_calls_as_loaded_pair_by_position` | da1e038 |
| 7 | `Agent::new("fighter")` in a module that installs `dedede` statuses (King Boo `status/status_catched_portrait.rs:75`) made all 30 of the module's parameter-agent statuses "claimed by several agents" | smashline's generic `fighter`/`weapon` agents never own a module | `acmd_src::tests::a_generic_fighter_agent_does_not_claim_the_module` | dd232f3 |
| 8 | The `noop_status!` macro's own body (`$agent.status($phase, $kind, ..)`, Sceptile/Shy Guy/Toad/template `common/util.rs`) read as an unattributed registration | A registration whose phase or kind is a `$` metavariable is a macro template, skipped (its invocations are read) | `acmd_src::tests::a_macro_rules_body_is_not_a_registration` | dd232f3 |
| 9 | Status lines `if ArticleModule::is_exist(acc, ID) {` (10 movesets) listed as unread, and commented-out calls listed as calls | Read the call inside a condition or expression; leave a commented-out line out | `articles::view::tests::status_lines_read_calls_inside_conditions_and_skip_comments` | 4766165 |
| 10 | Clones made through a helper, `unsafe fn clone(name, source, generate_id: *mut i32, ..)` called with `addr_of_mut!(FIGHTER_MARIO_GENERATE_ARTICLE_..)` (Sceptile `registration.rs:144-173`): 1 unresolved clone, 6 objects missing | Inline a helper at each call; a store through a pointer parameter binds the call's static | `articles::clones::tests::helper_calls_are_one_clone_each` | 4c09b5e |
| 11 | `.status(End, KIND, fly::bullet_seed_end)` with `bullet_seed_end` defined in two modules (Sceptile `bullet_seed/status/mod.rs:19`, 4 statuses) | Follow the path's module to the function | `acmd_src::tests::a_module_path_picks_the_function_in_that_module` | b8a0fd2 |
| 12 | `*FIGHTER_PEACH_GENERATE_ARTICLE_DAIKON` for the dump's `peach/motion/peachdaikon` (HDR `vendor/hdr/fighters/peach/src/acmd/specials.rs:379`; daisy the same) | A vanilla folder may carry its owner's name | `articles::tests::resolves_the_games_own_spellings` | cc6d0f6 |

Review round 1 (no change to the counts above; the final sweep's detail is identical):

- Clone helpers: a binding is looked up only among the bindings before it, with a depth guard,
  so `clone(name)` inside `fn reg(name: &str)`, a `CLONES.iter()` loop with the same names, or a
  two-helper cycle can't recurse; each is listed as unresolved at its call
  (`clones::tests::self_named_helper_arguments_are_unknown_not_a_loop`). Helpers are found across
  the project and through `self::`/`util::` paths, a same-named function elsewhere is still read,
  and a helper no call reaches lists its clones as "made inside helper `<fn>`, whose callers
  Visionary couldn't follow" (`clones::tests::helpers_are_found_across_files_and_paths`).
- A clone declared again from another source is a conflict on the article naming both sources
  and places, shown in its notes and warnings; an identical one merges its bindings
  (`articles::tests::a_clone_declared_from_two_sources_is_a_conflict`).
- A status function path naming no module beside the file (`super::status::f`, a `pub use`
  re-export) falls back to the unique definition, and the ambiguity reason says how many places
  and why (`acmd_src::tests::a_path_naming_no_module_beside_the_file_keeps_the_unique_match`).
- The misattribution checks above replace the earlier ones, which `build` made true by
  construction.

Fix 7 changes `fighter_for_file`, which ACMD script attribution uses too: a folder holding a
fighter's module and an `Agent::new("fighter")` now goes to the fighter instead of to nobody. No
other attribution changes; the full suite passes.

## Residue that remains, with reasons

| Kind | Count | Judgement |
|---|---|---|
| Fighter not in the local dump (or only partly dumped) | 912 + 24 | **Environment.** The configured dump holds 34 fighters, some only partly (`cloud` has only `motion/`, `miifighter/model` only `b_tifa_f`). Their vanilla articles cannot be listed, so ids and article agents of samus, ganon, shizue, master, koopa, zelda, captain and most HDR fighters stay unresolved. Re-run with a full dump; nothing in the code is missing for these. |
| `param_config` override the card doesn't model | 185 | **Not modelled by design** (Task 6/9): 158 multiply a value (`update_attribute_mul_2`), 26 have a computed name, index or value, 1 a kind Visionary can't name. Each is listed on the Settings card with its reason. |
| Another fighter's id | 114 | **Genuine, needs a design call.** Kirby's copy abilities (83: `FIGHTER_MASTER_GENERATE_ARTICLE_BOW`, `FIGHTER_KROOL_GENERATE_ARTICLE_BLUNDERBUSS` on kirby) and echo/semi-clone fighters (falco→fox 9, pichu→pikachu 6, richter→simon 5, samusd→samus 2, shizue→murabito 1) spawn with the other fighter's constant, whose number the game reads against the spawning fighter's own list. Latios (samus spawning `FIGHTER_LUCARIO_GENERATE_ARTICLE_AURABALL`, 2) and Master Chief (`FIGHTER_MASTERCHIEF_GENERATE_ARTICLE_GRAVLIFT` on master, 6; a const naming no fighter) are one-offs. Resolving them needs a per-fighter rule (copy = the copied fighter's articles, echo = the same list), not a reader fix. |
| No-op status (`noop_status!`) | 45 | **Nothing to read**: listed so the status isn't silently missing. |
| Numeric id | 31 | **Genuinely dynamic by design** (ids are resolved by name only): Sackboy's `const GENERATE_ARTICLE_WARPDOOR: i32 = 3` (21), HDR's `articles::plizardon::ROCK: i32 = 0x3` and siblings (9), a literal `0` (1). |
| Parameter-agent statuses several agents claim | 10 | **Genuinely ambiguous**: HDR `iceclimber/status/mod.rs` installs the same statuses on popo and nana. |
| Registered twice | 13 | **The moveset's layout**: HDR compiles both `ports/` and `vendor/hdr/` copies of the same scripts; the index rightly refuses to pick one. |
| `Agent::new(BASE_FIGHTER)` | 2 | **Left, outside the article paths**: the joob template and Parvus name the fighter through a `const &str`. The index reads literal agent names only, for ACMD and statuses alike; following the const changes fighter attribution generally, so it is reported rather than changed here. |
| Status call split over lines | 5 | **Kept, unread**: `ArticleModule::change_status(` with its arguments on the next lines (HDR kirby). Status rows read one line (Task 9 minor); the row says it is kept as written. |
| Enum-dispatched spawn | (no row) | Sceptile spawns its six clones through `ArticleKind` → `FIGHTER_MARIO_GENERATE_ARTICLE_*` in `lifecycle.rs:41-45`, so its scripts show no direct article rows. Genuinely dynamic. |

Every remaining unresolved item is either the environment (the partial dump), outside the article
model by design, or a shape whose resolution is a design decision rather than a reader gap.

## Concerns for review

- The view's fallback names an id by the name its constant spells even when that constant is
  another fighter's (`FIGHTER_LUCARIO_GENERATE_ARTICLE_AURABALL` on samus reads "Creates the
  auraball"). Right for Kirby's copies and echo fighters, wrong for Latios's borrowed constant.
- Fix 7 touches ACMD attribution outside the article paths (see above).
- The numbers here depend on the dump; a full dump would turn most of the 936 environment items
  into resolved ids (koopajr's `mechakoopa` has no folder even in this dump, and stays listed) and show whatever reader gaps they hide.

---

# Detail (as written by the sweep)

Dump: `D:\Ultimate-Unleashed\dev\tools\files\ArcExplorer\export`.

## Skipped folders

- `.github` — no Cargo.toml
- `Empoleon-Moveset(OLD)` — an older copy of a moveset that is swept under its own name
- `Geralt-Moveset` — no Cargo.toml
- `Goose-Moveset.zip` — an archive, not a folder
- `Pokemon-Trainer-Two-Moveset` — no Cargo.toml
- `Regigigas-Moveset` — no Cargo.toml
- `Shy-Guy-Moveset.zip` — an archive, not a folder
- `Toon-Zelda-Moveset` — no Cargo.toml
- `conquest-side-b-stash` — a stash copy of part of another moveset
- `potemkin_layout_ui_slotting.zip` — an archive, not a folder

## Summary

| Moveset | Articles (V/R/C) | Clones (unresolved) | Ids resolved / unresolved | Status ids resolved / unresolved | Settings lit/const/param/computed | Round trip (scripts, files) | Edits refused | Failures |
|---|---|---|---|---|---|---|---|---|
| -Moveset-Wii-Sports | 4/3/0 | 0 (0) | 2 / 0 | 2 / 0 | 0/0/0/2 | identical (1, 1) | 0 | 0 |
| Amy-Moveset | 5/1/1 | 1 (0) | 4 / 0 | 1 / 0 | 4/0/0/1 | identical (4, 2) | 0 | 0 |
| AnuGamer-Akaza-Moveset | 0/0/2 | 2 (0) | 13 / 2 | 0 / 0 | 7/0/0/1 | identical (8, 1) | 0 | 0 |
| Bluster-Kong-Moveset | 0/0/0 | 0 (0) | 0 / 0 | 0 / 0 | 0/0/0/0 | identical (0, 0) | 0 | 0 |
| Captain-America | 11/1/0 | 0 (0) | 2 / 0 | 5 / 0 | 0/0/0/1 | identical (2, 1) | 0 | 0 |
| Chaos-Moveset | 1/0/3 | 3 (0) | 4 / 0 | 3 / 0 | 3/0/0/11 | identical (2, 1) | 0 | 0 |
| Conker-Moveset-Dev | 7/0/1 | 1 (0) | 9 / 0 | 47 / 0 | 0/0/0/0 | identical (9, 3) | 0 | 0 |
| Conquest-Moveset | 3/0/0 | 0 (0) | 0 / 0 | 0 / 0 | 0/0/0/0 | identical (0, 0) | 0 | 0 |
| Cuphead-Moveset | 1/2/0 | 0 (0) | 40 / 0 | 1 / 0 | 13/0/0/62 | identical (10, 1) | 0 | 0 |
| Dark-Bowser-Moveset | 0/0/2 | 2 (0) | 8 / 2 | 0 / 0 | 1/0/0/0 | identical (5, 3) | 0 | 0 |
| Doomslayer-Moveset | 1/0/0 | 0 (0) | 0 / 0 | 0 / 0 | 0/0/0/0 | identical (0, 0) | 0 | 0 |
| Effect Preview Moveset | 0/0/0 | 0 (0) | 0 / 0 | 0 / 0 | 0/0/0/0 | identical (0, 0) | 0 | 0 |
| Empoleon-Moveset | 4/2/1 | 1 (0) | 1 / 0 | 0 / 0 | 1/0/0/2 | identical (1, 1) | 0 | 0 |
| Gallade-Moveset | 0/0/0 | 0 (0) | 0 / 0 | 0 / 0 | 0/0/0/0 | identical (0, 0) | 0 | 0 |
| Goose-Moveset | 0/0/1 | 1 (0) | 3 / 4 | 0 / 0 | 1/5/0/2 | identical (4, 1) | 0 | 0 |
| HDR-Standalone-Movesets | 255/56/5 | 6 (0) | 281 / 636 | 45 / 167 | 25/0/12/34 | identical (446, 137) | 0 | 0 |
| Iron-Man | 4/3/3 | 3 (0) | 20 / 0 | 5 / 0 | 9/0/0/9 | identical (18, 6) | 0 | 0 |
| King-Boo-Moveset | 12/0/5 | 5 (0) | 13 / 0 | 0 / 0 | 19/2/0/16 | identical (9, 6) | 0 | 0 |
| Latios-Moveset | 4/0/4 | 4 (0) | 15 / 6 | 0 / 0 | 2/0/0/9 | identical (11, 3) | 0 | 0 |
| Master-Chief-Moveset | 0/0/2 | 2 (0) | 3 / 14 | 0 / 9 | 4/0/0/10 | identical (14, 4) | 0 | 0 |
| Parvus-Moveset | 0/0/0 | 0 (0) | 0 / 0 | 0 / 0 | 0/0/0/0 | identical (0, 0) | 0 | 0 |
| Potemkin-SL2 | 6/0/4 | 4 (0) | 28 / 0 | 39 / 0 | 15/0/0/0 | identical (9, 4) | 0 | 0 |
| Sackboy-Moveset | 0/0/1 | 1 (0) | 2 / 43 | 0 / 31 | 1/0/0/3 | identical (17, 8) | 0 | 0 |
| Sceptile-Moveset | 7/0/6 | 6 (0) | 0 / 0 | 0 / 0 | 2/16/0/11 | identical (0, 0) | 0 | 0 |
| Shadow-The-Hedgehog | 4/1/0 | 0 (0) | 2 / 0 | 0 / 0 | 0/0/0/0 | identical (2, 1) | 0 | 0 |
| Shigaraki | 5/0/0 | 0 (0) | 0 / 0 | 0 / 0 | 0/0/0/0 | identical (0, 0) | 0 | 0 |
| Shy-Guy-Moveset | 6/3/0 | 0 (0) | 1 / 0 | 1 / 0 | 3/24/0/5 | identical (1, 1) | 0 | 0 |
| Skull-Kid-Moveset | 0/0/0 | 0 (0) | 0 / 0 | 0 / 0 | 0/0/0/0 | identical (0, 0) | 0 | 0 |
| Springtrap-Moveset | 0/0/2 | 2 (0) | 6 / 23 | 0 / 9 | 82/0/2/21 | identical (18, 9) | 0 | 0 |
| Tails-Moveset | 3/1/2 | 2 (0) | 5 / 0 | 0 / 0 | 0/0/0/13 | identical (3, 2) | 0 | 0 |
| Toad-Moveset | 47/5/0 | 0 (0) | 30 / 0 | 0 / 0 | 3/0/0/9 | identical (19, 5) | 0 | 0 |
| Waluigi-Moveset | 0/0/1 | 1 (0) | 2 / 0 | 0 / 0 | 2/0/0/0 | identical (2, 1) | 0 | 0 |
| joob-ssbu-moveset-template | 0/0/0 | 0 (0) | 0 / 0 | 0 / 0 | 0/0/0/0 | identical (0, 0) | 0 | 0 |
| lily-srs-template | 7/0/0 | 0 (0) | 0 / 0 | 0 / 0 | 0/0/0/0 | identical (0, 0) | 0 | 0 |
| ssbuPhosphoraMoveset | 0/0/0 | 0 (0) | 0 / 6 | 0 / 0 | 0/0/0/0 | identical (6, 3) | 0 | 0 |

## -Moveset-Wii-Sports

Root `C:/Users/notja/Documents/Coding/SSBU Modding/Movesets\-Moveset-Wii-Sports`. Fighters: mario (dump).

| Article | Origin | Ids | Scripts | Rows (spawns) | Status rows | Param block | Settings |
|---|---|---|---|---|---|---|---|
| mario_cappy | rewritten | FIGHTER_MARIO_GENERATE_ARTICLE_CAPPY | 2 | 0 (0) | 2 | absent |  |
| mario_fireball | rewritten | FIGHTER_MARIO_GENERATE_ARTICLE_FIREBALL | 2 | 2 (1) | 0 | present | Speed X: computed `speed_x`; Speed Y: computed `speed_y` |
| mario_pump | rewritten | FIGHTER_MARIO_GENERATE_ARTICLE_PUMP | 13 | 0 (0) | 0 | present |  |

## Amy-Moveset

Root `C:/Users/notja/Documents/Coding/SSBU Modding/Movesets\Amy-Moveset`. Fighters: kamui (dump).

| Article | Origin | Ids | Scripts | Rows (spawns) | Status rows | Param block | Settings |
|---|---|---|---|---|---|---|---|
| kamui_ryusensya | rewritten | FIGHTER_KAMUI_GENERATE_ARTICLE_RYUSENSYA | 3 | 2 (2) | 0 | present |  |
| kamui_present | clone of krool/WEAPON_KIND_KROOL_IRONBALL | FIGHTER_AMY_GENERATE_ARTICLE_PRESENT | 5 | 2 (2) | 1 | present | Acceleration X: literal 0; Gravity: literal 0.05; Max speed X: literal 0; Max speed Y: literal 2; Other movement: computed `sv_kinetic_energy!(enable,weapon,WEAPON_KINETIC_ENERGY_RESERVE_ID_NORM` |

### Unread param_config overrides (31)

- `src/params.rs:20` `param_config::update_attribute_mul_2(*FIGHTER_KIND_KAMUI, marked_slots.clone(), (hash40("walk_accel_mul"), 0, 0.105/0.105))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/params.rs:21` `param_config::update_attribute_mul_2(*FIGHTER_KIND_KAMUI, marked_slots.clone(), (hash40("walk_accel_add"), 0, 0.105/0.105))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/params.rs:22` `param_config::update_attribute_mul_2(*FIGHTER_KIND_KAMUI, marked_slots.clone(), (hash40("walk_slow_speed_mul"), 0, 0.2/0.2))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/params.rs:23` `param_config::update_attribute_mul_2(*FIGHTER_KIND_KAMUI, marked_slots.clone(), (hash40("walk_middle_ratio"), 0, 0.5/0.5))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/params.rs:24` `param_config::update_attribute_mul_2(*FIGHTER_KIND_KAMUI, marked_slots.clone(), (hash40("walk_fast_ratio"), 0, 0.75/0.75))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/params.rs:25` `param_config::update_attribute_mul_2(*FIGHTER_KIND_KAMUI, marked_slots.clone(), (hash40("walk_speed_max"), 0, 1.44/1.208))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/params.rs:26` `param_config::update_attribute_mul_2(*FIGHTER_KIND_KAMUI, marked_slots.clone(), (hash40("ground_brake"), 0, 0.08/0.11))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/params.rs:27` `param_config::update_attribute_mul_2(*FIGHTER_KIND_KAMUI, marked_slots.clone(), (hash40("dash_speed"), 0, 1.7/2.09))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/params.rs:28` `param_config::update_attribute_mul_2(*FIGHTER_KIND_KAMUI, marked_slots.clone(), (hash40("run_accel_mul"), 0, 0.135/0.10978))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/params.rs:29` `param_config::update_attribute_mul_2(*FIGHTER_KIND_KAMUI, marked_slots.clone(), (hash40("run_accel_add"), 0, 0.04/0.044))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/params.rs:30` `param_config::update_attribute_mul_2(*FIGHTER_KIND_KAMUI, marked_slots.clone(), (hash40("run_speed_max"), 0, 3.0/1.54))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/params.rs:31` `param_config::update_attribute_mul_2(*FIGHTER_KIND_KAMUI, marked_slots.clone(), (hash40("jump_speed_x"), 0, 0.9/1.0))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/params.rs:32` `param_config::update_attribute_mul_2(*FIGHTER_KIND_KAMUI, marked_slots.clone(), (hash40("jump_speed_x_mul"), 0, 1.2/0.8))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/params.rs:33` `param_config::update_attribute_mul_2(*FIGHTER_KIND_KAMUI, marked_slots.clone(), (hash40("jump_speed_x_max"), 0, 1.5/1.3))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/params.rs:34` `param_config::update_attribute_mul_2(*FIGHTER_KIND_KAMUI, marked_slots.clone(), (hash40("jump_aerial_speed_x_max"), 0, 0.9/0.9))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/params.rs:35` `param_config::update_attribute_mul_2(*FIGHTER_KIND_KAMUI, marked_slots.clone(), (hash40("jump_y"), 0, 36.0/32.02))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/params.rs:36` `param_config::update_attribute_mul_2(*FIGHTER_KIND_KAMUI, marked_slots.clone(), (hash40("jump_aerial_y"), 0, 36.0/30.71))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/params.rs:37` `param_config::update_attribute_mul_2(*FIGHTER_KIND_KAMUI, marked_slots.clone(), (hash40("jump_initial_y"), 0, 19.25/17.611))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/params.rs:38` `param_config::update_attribute_mul_2(*FIGHTER_KIND_KAMUI, marked_slots.clone(), (hash40("mini_jump_y"), 0, 16.0/15.38))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/params.rs:39` `param_config::update_attribute_mul_2(*FIGHTER_KIND_KAMUI, marked_slots.clone(), (hash40("air_accel_x_mul"), 0, 0.04/0.08))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/params.rs:40` `param_config::update_attribute_mul_2(*FIGHTER_KIND_KAMUI, marked_slots.clone(), (hash40("air_accel_x_add"), 0, 0.01/0.01))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/params.rs:41` `param_config::update_attribute_mul_2(*FIGHTER_KIND_KAMUI, marked_slots.clone(), (hash40("air_accel_x_stable"), 0, 1.208/1.281))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/params.rs:42` `param_config::update_attribute_mul_2(*FIGHTER_KIND_KAMUI, marked_slots.clone(), (hash40("air_brake_x"), 0, 0.01/0.004))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/params.rs:43` `param_config::update_attribute_mul_2(*FIGHTER_KIND_KAMUI, marked_slots.clone(), (hash40("dive_speed_y"), 0, 3.0/2.88))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/params.rs:44` `param_config::update_attribute_mul_2(*FIGHTER_KIND_KAMUI, marked_slots.clone(), (hash40("damage_fly_top_air_accel_y"), 0, 0.070896/0.072912))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/params.rs:45` `param_config::update_attribute_mul_2(*FIGHTER_KIND_KAMUI, marked_slots.clone(), (hash40("damage_fly_top_air_accel_y_stable"), 0, 1.8/1.8))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/params.rs:46` `param_config::update_attribute_mul_2(*FIGHTER_KIND_KAMUI, marked_slots.clone(), (hash40("air_break_y"), 0, 0.01/0.01))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/params.rs:47` `param_config::update_attribute_mul_2(*FIGHTER_KIND_KAMUI, marked_slots.clone(), (hash40("air_accel_y"), 0, 0.12/0.13))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/params.rs:48` `param_config::update_attribute_mul_2(*FIGHTER_KIND_KAMUI, marked_slots.clone(), (hash40("air_accel_y_stable"), 0, 1.8/1.65))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/params.rs:49` `param_config::update_attribute_mul_2(*FIGHTER_KIND_KAMUI, marked_slots.clone(), (hash40("weight"), 0, 83.0/92.0))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/params.rs:50` `param_config::update_attribute_mul_2(*FIGHTER_KIND_KAMUI, marked_slots.clone(), (hash40("shield_break_y"), 0, 39.0/44.0))` — it multiplies the value instead of setting it, which the card doesn't show

## AnuGamer-Akaza-Moveset

Root `C:/Users/notja/Documents/Coding/SSBU Modding/Movesets\AnuGamer-Akaza-Moveset\code`. Fighters: captain (not in the dump).

| Article | Origin | Ids | Scripts | Rows (spawns) | Status rows | Param block | Settings |
|---|---|---|---|---|---|---|---|
| captain_airtype | clone of cloud/WEAPON_KIND_CLOUD_WAVE | FIGHTER_CAPTAIN_GENERATE_ARTICLE_AIRTYPE | 4 | 6 (6) | 0 | no param file | Life: literal 40; Other movement: computed `sv_kinetic_energy!( set_speed, weapon, WEAPON_KINETIC_ENERGY_RESERVE_I`; Speed X: literal 3, literal 3; Speed Y: literal -0.5, literal 0 |
| captain_needle | clone of rockman/WEAPON_KIND_ROCKMAN_ROCKMANX | FIGHTER_CAPTAIN_GENERATE_ARTICLE_NEEDLE | 2 | 7 (3) | 0 | no param file | Size: literal 5, literal 1 |

### Unresolved ids (scripts) (2)

- `src/captain/mod.rs:2154` captain/game_finalairend `*FIGHTER_CAPTAIN_GENERATE_ARTICLE_BLUEFALCON` (remove_exist) — environment: captain is not in the local dump, so its vanilla `bluefalcon` is not listed
- `src/captain/mod.rs:2154` captain/game_finalend `*FIGHTER_CAPTAIN_GENERATE_ARTICLE_BLUEFALCON` (remove_exist) — environment: captain is not in the local dump, so its vanilla `bluefalcon` is not listed

### Agents no catalog lists (1)

- `captain_bluefalcon` (14 scripts, 0 statuses): environment: captain is not in the local dump, so its vanilla articles are not listed

## Bluster-Kong-Moveset

Root `C:/Users/notja/Documents/Coding/SSBU Modding/Movesets\Bluster-Kong-Moveset`. Fighters: donkey (not in the dump).

| Article | Origin | Ids | Scripts | Rows (spawns) | Status rows | Param block | Settings |
|---|---|---|---|---|---|---|---|

## Captain-America

Root `C:/Users/notja/Documents/Coding/SSBU Modding/Movesets\Captain-America`. Fighters: dedede (dump).

| Article | Origin | Ids | Scripts | Rows (spawns) | Status rows | Param block | Settings |
|---|---|---|---|---|---|---|---|
| dedede_gordo | rewritten | FIGHTER_DEDEDE_GENERATE_ARTICLE_GORDO | 7 | 0 (0) | 5 | present | Life: computed `gordo_hop_life` |
| dedede_shrine | vanilla | FIGHTER_DEDEDE_GENERATE_ARTICLE_SHRINE | 0 | 2 (0) | 0 | absent |  |

### Unread param_config overrides (3)

- `src/lib.rs:74` `param_config::update_int_2(*FIGHTER_KIND_DEDEDE, marked_slots.clone(), (smash::hash40("param_motion"), smash::hash40("motion_share"), *FIGHTER_MOTION_SHARE_TYPE_NORMAL))` — its param name or value is not a literal
- `src/lib.rs:78` `param_config::update_int_2(*FIGHTER_KIND_DEDEDE, marked_slots.clone(), (smash::hash40("aerial_type"), 0, *FIGHTER_JUMP_AERIAL_TYPE_NORMAL))` — its param name or value is not a literal
- `src/lib.rs:83` `param_config::update_int_2(*FIGHTER_KIND_DEDEDE, marked_slots.clone(), (smash::hash40("attack_combo_type"), 0, *FIGHTER_COMBO_TYPE_HIT))` — its param name or value is not a literal

## Chaos-Moveset

Root `C:/Users/notja/Documents/Coding/SSBU Modding/Movesets\Chaos-Moveset\Chaos-rs`. Fighters: gekkouga (dump).

| Article | Origin | Ids | Scripts | Rows (spawns) | Status rows | Param block | Settings |
|---|---|---|---|---|---|---|---|
| gekkouga_waterball | clone of koopajr/WEAPON_KIND_KOOPAJR_CANNONBALL | FIGHTER_CHAOS_GENERATE_ARTICLE_WATERBALL | 3 | 4 (2) | 0 | no param file | Acceleration X: literal 0; Acceleration Y: computed `-air_accel_y * air_accel_y_mul`; Life: computed `life`, computed `life`; Speed X: computed `move_speed_x`; Speed Y: computed `move_speed_y` |
| gekkouga_ripple | clone of bayonetta/WEAPON_KIND_BAYONETTA_WICKEDWEAVEARM | FIGHTER_CHAOS_GENERATE_ARTICLE_RIPPLE | 5 | 0 (0) | 1 | no param file | Other movement: computed `sv_kinetic_energy!(set_speed, weapon, WEAPON_KINETIC_ENERGY_RESERVE_ID`, computed `sv_kinetic_energy!(set_stable_speed, weapon, WEAPON_KINETIC_ENERGY_RES`, computed `sv_kinetic_energy!(set_accel, weapon, WEAPON_KINETIC_ENERGY_RESERVE_ID`; Size: computed `RIPPLE_SIZE[ENTRY_ID]`, computed `RIPPLE_SIZE[ENTRY_ID]`; Speed X: literal 0; Speed Y: literal 0 |
| gekkouga_puddle | clone of ike/WEAPON_KIND_IKE_SWORD | FIGHTER_CHAOS_GENERATE_ARTICLE_PUDDLE | 4 | 0 (0) | 2 | no param file | Size: computed `scale` |

## Conker-Moveset-Dev

Root `C:/Users/notja/Documents/Coding/SSBU Modding/Movesets\Conker-Moveset-Dev\code`. Fighters: mario (dump).

| Article | Origin | Ids | Scripts | Rows (spawns) | Status rows | Param block | Settings |
|---|---|---|---|---|---|---|---|
| mario_cappy | vanilla | FIGHTER_MARIO_GENERATE_ARTICLE_CAPPY | 0 | 2 (0) | 5 | absent |  |
| mario_fireball | vanilla | FIGHTER_MARIO_GENERATE_ARTICLE_FIREBALL | 0 | 6 (6) | 0 | present |  |
| mario_pump | vanilla | FIGHTER_MARIO_GENERATE_ARTICLE_PUMP | 0 | 0 (0) | 42 | present |  |
| mario_cannonballcloned | clone of koopajr/WEAPON_KIND_KOOPAJR_CANNONBALL | FIGHTER_MARIO_GENERATE_ARTICLE_ARROW | 0 | 1 (1) | 0 | absent |  |

## Conquest-Moveset

Root `C:/Users/notja/Documents/Coding/SSBU Modding/Movesets\Conquest-Moveset`. Fighters: ganon (not in the dump).

| Article | Origin | Ids | Scripts | Rows (spawns) | Status rows | Param block | Settings |
|---|---|---|---|---|---|---|---|

### Unread param_config overrides (4)

- `src/lib.rs:188` `param_config::update_int_2(*FIGHTER_KIND_GANON, marked_slots.clone(), (smash::hash40("wall_jump_type"), 1, 1))` — its second hash is not a literal
- `src/lib.rs:191` `param_config::update_int_2(*FIGHTER_KIND_GANON, marked_slots.clone(), (HAS_FLOATY_JUMP, 0, 1))` — its param name or value is not a literal
- `src/lib.rs:192` `param_config::update_float_2(*FIGHTER_KIND_GANON, marked_slots.clone(), (FLOATY_JUMP_PARAM_1, 0, -0.05))` — its param name or value is not a literal
- `src/lib.rs:193` `param_config::update_float_2(*FIGHTER_KIND_GANON, marked_slots.clone(), (FLOATY_JUMP_PARAM_2, 0, 0.05))` — its param name or value is not a literal

## Cuphead-Moveset

Root `C:/Users/notja/Documents/Coding/SSBU Modding/Movesets\Cuphead-Moveset`. Fighters: buddy (not in the dump).

| Article | Origin | Ids | Scripts | Rows (spawns) | Status rows | Param block | Settings |
|---|---|---|---|---|---|---|---|
| buddy_bullet | rewritten | FIGHTER_BUDDY_GENERATE_ARTICLE_BULLET | 25 | 40 (40) | 0 | no param file | Life: literal 40, literal 9, literal 120, literal 70, literal 90, literal 60, literal 40, literal 50, literal 80, literal 300, literal 60, literal 5; Other movement: computed `sv_kinetic_energy::get_speed_x(weapon.lua_state_agent)`, computed `sv_kinetic_energy::get_speed_y(weapon.lua_state_agent)`, computed `sv_kinetic_energy::get_speed_x(weapon.lua_state_agent)`, computed `sv_kinetic_energy::get_speed_y(weapon.lua_state_agent)`, computed `sv_kinetic_energy::get_speed_x(weapon.lua_state_agent)`, computed `sv_kinetic_energy::get_speed_y(weapon.lua_state_agent)`, computed `sv_kinetic_energy::get_speed_x(weapon.lua_state_agent)`, computed `sv_kinetic_energy::get_speed_y(weapon.lua_state_agent)`, computed `sv_kinetic_energy::get_speed_x(weapon.lua_state_agent)`, computed `sv_kinetic_energy::get_speed_y(weapon.lua_state_agent)`, computed `sv_kinetic_energy::get_speed_x(weapon.lua_state_agent)`, computed `sv_kinetic_energy::get_speed_y(weapon.lua_state_agent)`, computed `sv_kinetic_energy::get_speed_x(weapon.lua_state_agent)`, computed `sv_kinetic_energy::get_speed_y(weapon.lua_state_agent)`, computed `sv_kinetic_energy::get_speed_x(weapon.lua_state_agent)`, computed `sv_kinetic_energy::get_speed_y(weapon.lua_state_agent)`; Size: computed `size`, computed `size`, computed `size`, computed `size`, computed `size`, computed `size`, computed `size`, computed `size`, computed `size`, computed `size`; Speed X: computed `speed_x`, computed `new_x`, computed `speed_x`, computed `speed_x`, computed `speed_x`, computed `speed_x`, computed `speed_x`, computed `speed_x`, computed `speed_x`, computed `speed_x`, computed `speed_x`, computed `speed_x`, computed `speed_x`, computed `speed_x`, computed `new_x`, computed `speed_x`, computed `speed_x`, computed `speed_x`; Speed Y: computed `speed_y`, computed `new_y`, computed `speed_y`, computed `speed_y`, computed `speed_y`, computed `speed_y`, computed `speed_y`, computed `speed_y`, computed `speed_y`, computed `speed_y`, computed `speed_y`, computed `speed_y`, computed `speed_y`, computed `speed_y`, computed `new_y`, computed `speed_y`, computed `speed_y`, computed `speed_y` |
| buddy_horn | rewritten | FIGHTER_BUDDY_GENERATE_ARTICLE_HORN | 1 | 0 (0) | 1 | no param file |  |
| buddy_strings | vanilla | FIGHTER_BUDDY_GENERATE_ARTICLE_STRINGS | 0 | 0 (0) | 0 | no param file | Size: literal 1.25 |

### Unread param_config overrides (28)

- `src/lib.rs:70` `param_config::update_int_2(*FIGHTER_KIND_BUDDY, marked_slots.clone(), (smash::hash40("aerial_type"), 0, *FIGHTER_JUMP_AERIAL_TYPE_NORMAL))` — its param name or value is not a literal
- `src/lib.rs:72` `param_config::update_attribute_mul_2(*FIGHTER_KIND_BUDDY, marked_slots.clone(), (smash::hash40("walk_accel_mul"), 0, 2.7))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/lib.rs:73` `param_config::update_attribute_mul_2(*FIGHTER_KIND_BUDDY, marked_slots.clone(), (smash::hash40("walk_accel_add"), 0, 1.8))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/lib.rs:74` `param_config::update_attribute_mul_2(*FIGHTER_KIND_BUDDY, marked_slots.clone(), (smash::hash40("walk_speed_max"), 0, 1.068))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/lib.rs:75` `param_config::update_attribute_mul_2(*FIGHTER_KIND_BUDDY, marked_slots.clone(), (smash::hash40("walk_slow_speed_mul"), 0, 1.0))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/lib.rs:76` `param_config::update_attribute_mul_2(*FIGHTER_KIND_BUDDY, marked_slots.clone(), (smash::hash40("walk_middle_ratio"), 0, 1.32))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/lib.rs:77` `param_config::update_attribute_mul_2(*FIGHTER_KIND_BUDDY, marked_slots.clone(), (smash::hash40("walk_fast_ratio"), 0, 1.0))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/lib.rs:78` `param_config::update_attribute_mul_2(*FIGHTER_KIND_BUDDY, marked_slots.clone(), (smash::hash40("ground_brake"), 0, 1.24))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/lib.rs:79` `param_config::update_attribute_mul_2(*FIGHTER_KIND_BUDDY, marked_slots.clone(), (smash::hash40("dash_speed"), 0, 1.166))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/lib.rs:80` `param_config::update_attribute_mul_2(*FIGHTER_KIND_BUDDY, marked_slots.clone(), (smash::hash40("run_accel_mul"), 0, 1.57))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/lib.rs:81` `param_config::update_attribute_mul_2(*FIGHTER_KIND_BUDDY, marked_slots.clone(), (smash::hash40("run_accel_add"), 0, 0.55))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/lib.rs:82` `param_config::update_attribute_mul_2(*FIGHTER_KIND_BUDDY, marked_slots.clone(), (smash::hash40("run_speed_max"), 0, 0.735))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/lib.rs:83` `param_config::update_attribute_mul_2(*FIGHTER_KIND_BUDDY, marked_slots.clone(), (smash::hash40("jump_speed_x"), 0, 1.056))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/lib.rs:84` `param_config::update_attribute_mul_2(*FIGHTER_KIND_BUDDY, marked_slots.clone(), (smash::hash40("jump_speed_x_mul"), 0, 1.13))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/lib.rs:85` `param_config::update_attribute_mul_2(*FIGHTER_KIND_BUDDY, marked_slots.clone(), (smash::hash40("jump_speed_x_max"), 0, 1.0))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/lib.rs:86` `param_config::update_attribute_mul_2(*FIGHTER_KIND_BUDDY, marked_slots.clone(), (smash::hash40("jump_aerial_speed_x_mul"), 0, 1.13))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/lib.rs:87` `param_config::update_attribute_mul_2(*FIGHTER_KIND_BUDDY, marked_slots.clone(), (smash::hash40("jump_initial_y"), 0, 1.1))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/lib.rs:88` `param_config::update_attribute_mul_2(*FIGHTER_KIND_BUDDY, marked_slots.clone(), (smash::hash40("jump_y"), 0, 1.1))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/lib.rs:89` `param_config::update_attribute_mul_2(*FIGHTER_KIND_BUDDY, marked_slots.clone(), (smash::hash40("mini_jump_y"), 0, 0.646))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/lib.rs:90` `param_config::update_attribute_mul_2(*FIGHTER_KIND_BUDDY, marked_slots.clone(), (smash::hash40("jump_aerial_y"), 0, 1.025))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/lib.rs:91` `param_config::update_attribute_mul_2(*FIGHTER_KIND_BUDDY, marked_slots.clone(), (smash::hash40("air_accel_x_mul"), 0, 1.667))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/lib.rs:92` `param_config::update_attribute_mul_2(*FIGHTER_KIND_BUDDY, marked_slots.clone(), (smash::hash40("air_accel_x_add"), 0, 1.0))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/lib.rs:93` `param_config::update_attribute_mul_2(*FIGHTER_KIND_BUDDY, marked_slots.clone(), (smash::hash40("air_speed_x_stable"), 0, 1.256))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/lib.rs:94` `param_config::update_attribute_mul_2(*FIGHTER_KIND_BUDDY, marked_slots.clone(), (smash::hash40("air_brake_x"), 0, 3.58))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/lib.rs:95` `param_config::update_attribute_mul_2(*FIGHTER_KIND_BUDDY, marked_slots.clone(), (smash::hash40("air_accel_y"), 0, 0.98))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/lib.rs:96` `param_config::update_attribute_mul_2(*FIGHTER_KIND_BUDDY, marked_slots.clone(), (smash::hash40("air_speed_y_stable"), 0, 1.023))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/lib.rs:97` `param_config::update_attribute_mul_2(*FIGHTER_KIND_BUDDY, marked_slots.clone(), (smash::hash40("air_brake_y"), 0, 0.72))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/lib.rs:98` `param_config::update_attribute_mul_2(*FIGHTER_KIND_BUDDY, marked_slots.clone(), (smash::hash40("weight"), 0, 0.85))` — it multiplies the value instead of setting it, which the card doesn't show

## Dark-Bowser-Moveset

Root `C:/Users/notja/Documents/Coding/SSBU Modding/Movesets\Dark-Bowser-Moveset`. Fighters: koopa (not in the dump).

| Article | Origin | Ids | Scripts | Rows (spawns) | Status rows | Param block | Settings |
|---|---|---|---|---|---|---|---|
| koopa_chromcloned | clone of reflet/WEAPON_KIND_REFLET_CHROM | FIGHTER_KOOPA_GENERATE_ARTICLE_CHROMCLONED | 2 | 2 (1) | 0 | no param file | Size: literal 0.82 |
| koopa_cannonballcloned | clone of koopajr/WEAPON_KIND_KOOPAJR_CANNONBALL | FIGHTER_KOOPA_GENERATE_ARTICLE_CANNONBALLCLONED | 1 | 6 (2) | 0 | no param file |  |

### Unresolved ids (scripts) (2)

- `src/koopad/koopa/acmd/special/special_n_fire.rs:6` koopa/game_specialairnfire `*FIGHTER_KOOPA_GENERATE_ARTICLE_BREATH` (generate_article) — environment: koopa is not in the local dump, so its vanilla `breath` is not listed
- `src/koopad/koopa/acmd/special/special_n_fire.rs:6` koopa/game_specialnfire `*FIGHTER_KOOPA_GENERATE_ARTICLE_BREATH` (generate_article) — environment: koopa is not in the local dump, so its vanilla `breath` is not listed

### Agents no catalog lists (1)

- `koopa_breath` (3 scripts, 3 statuses): environment: koopa is not in the local dump, so its vanilla articles are not listed

### Unread param_config overrides (3)

- `src/lib.rs:79` `param_config::update_attribute_mul_2(*FIGHTER_KIND_KOOPA, marked_slots.clone(), (smash::hash40("air_speed_y_stable"), 0, 0.9))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/lib.rs:80` `param_config::update_attribute_mul_2(*FIGHTER_KIND_KOOPA, marked_slots.clone(), (smash::hash40("dive_speed_y"), 0, 0.9))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/lib.rs:81` `param_config::update_attribute_mul_2(*FIGHTER_KIND_KOOPA, marked_slots.clone(), (smash::hash40("weight"), 0, 0.9))` — it multiplies the value instead of setting it, which the card doesn't show

## Doomslayer-Moveset

Root `C:/Users/notja/Documents/Coding/SSBU Modding/Movesets\Doomslayer-Moveset`. Fighters: ganon (not in the dump).

| Article | Origin | Ids | Scripts | Rows (spawns) | Status rows | Param block | Settings |
|---|---|---|---|---|---|---|---|

## Effect Preview Moveset

Root `C:/Users/notja/Documents/Coding/SSBU Modding/Movesets\Effect Preview Moveset`. Fighters: .

| Article | Origin | Ids | Scripts | Rows (spawns) | Status rows | Param block | Settings |
|---|---|---|---|---|---|---|---|

## Empoleon-Moveset

Root `C:/Users/notja/Documents/Coding/SSBU Modding/Movesets\Empoleon-Moveset`. Fighters: falco (not in the dump).

| Article | Origin | Ids | Scripts | Rows (spawns) | Status rows | Param block | Settings |
|---|---|---|---|---|---|---|---|
| falco_arwing | rewritten | FIGHTER_FALCO_GENERATE_ARTICLE_ARWING | 2 | 0 (0) | 0 | present |  |
| falco_blaster_bullet | rewritten | FIGHTER_FALCO_GENERATE_ARTICLE_BLASTER_BULLET | 3 | 1 (1) | 0 | present | Life: computed `life`, computed `life`; Size: literal 1 |
| falco_cannon | clone of link/WEAPON_KIND_LINK_SWORD_BEAM, original code | FIGHTER_FALCO_GENERATE_ARTICLE_CANNON | 5 | 0 (0) | 0 | present |  |

## Gallade-Moveset

Root `C:/Users/notja/Documents/Coding/SSBU Modding/Movesets\Gallade-Moveset`. Fighters: .

| Article | Origin | Ids | Scripts | Rows (spawns) | Status rows | Param block | Settings |
|---|---|---|---|---|---|---|---|

## Goose-Moveset

Root `C:/Users/notja/Documents/Coding/SSBU Modding/Movesets\Goose-Moveset`. Fighters: ganon (not in the dump).

| Article | Origin | Ids | Scripts | Rows (spawns) | Status rows | Param block | Settings |
|---|---|---|---|---|---|---|---|
| ganon_boomerangcloned | clone of link/WEAPON_KIND_LINK_BOOMERANG | FIGHTER_GOOSE_GENERATE_ARTICLE_BREAD | 3 | 3 (3) | 0 | no param file | Acceleration X: literal 0; Gravity: const AIR_ACCEL_Y = 0; Life: const LIFE = 500, const LIFE = 500; Speed X: const MOVE_SPEED_X = 1, computed `speed_x`; Speed Y: const MOVE_SPEED_Y = 1, computed `speed_y` |

### Unresolved ids (scripts) (4)

- `src/goose/acmd.rs:522` ganon/game_specialairnturn `*FIGHTER_GANON_GENERATE_ARTICLE_SWORD` (remove_exist) — environment: ganon is not in the local dump, so its vanilla `sword` is not listed
- `src/goose/acmd.rs:523` ganon/game_specialairnturn `*FIGHTER_GANON_GENERATE_ARTICLE_SWORD` (generate_article) — environment: ganon is not in the local dump, so its vanilla `sword` is not listed
- `src/goose/acmd.rs:423` ganon/game_specialnturn `*FIGHTER_GANON_GENERATE_ARTICLE_SWORD` (remove_exist) — environment: ganon is not in the local dump, so its vanilla `sword` is not listed
- `src/goose/acmd.rs:424` ganon/game_specialnturn `*FIGHTER_GANON_GENERATE_ARTICLE_SWORD` (generate_article) — environment: ganon is not in the local dump, so its vanilla `sword` is not listed

## HDR-Standalone-Movesets

Root `C:/Users/notja/Documents/Coding/SSBU Modding/Movesets\HDR-Standalone-Movesets`. Fighters: bayonetta (not in the dump), brave (not in the dump), buddy (not in the dump), captain (not in the dump), chrom (not in the dump), cloud (dump), daisy (not in the dump), dedede (dump), demon (not in the dump), diddy (dump), dolly (not in the dump), donkey (not in the dump), duckhunt (not in the dump), edge (dump), eflame (not in the dump), elight (not in the dump), falco (not in the dump), fox (not in the dump), gamewatch (not in the dump), ganon (not in the dump), gaogaen (not in the dump), gekkouga (dump), ike (not in the dump), inkling (not in the dump), jack (dump), kamui (dump), ken (not in the dump), kirby (not in the dump), koopa (not in the dump), koopajr (dump), krool (dump), link (dump), littlemac (dump), lucario (dump), lucas (not in the dump), lucina (not in the dump), luigi (dump), mario (dump), mariod (dump), marth (not in the dump), master (not in the dump), metaknight (not in the dump), mewtwo (dump), miifighter (dump), miigunner (not in the dump), miiswordsman (dump), murabito (dump), ness (not in the dump), packun (not in the dump), pacman (dump), palutena (dump), peach (dump), pfushigisou (dump), pichu (dump), pickel (not in the dump), pikachu (dump), pikmin (not in the dump), pit (not in the dump), pitb (not in the dump), plizardon (not in the dump), ptrainer (not in the dump), purin (dump), pzenigame (not in the dump), reflet (not in the dump), richter (dump), ridley (dump), robot (not in the dump), rockman (dump), rosetta (not in the dump), roy (not in the dump), ryu (not in the dump), samus (not in the dump), samusd (not in the dump), sheik (not in the dump), shizue (not in the dump), shulk (not in the dump), simon (not in the dump), snake (not in the dump), sonic (dump), szerosuit (not in the dump), tantan (not in the dump), toonlink (dump), trail (not in the dump), wario (dump), wiifit (not in the dump), wolf (not in the dump), yoshi (not in the dump), younglink (dump), zelda (not in the dump).

| Article | Origin | Ids | Scripts | Rows (spawns) | Status rows | Param block | Settings |
|---|---|---|---|---|---|---|---|
| dedede_gordo | rewritten | FIGHTER_DEDEDE_GENERATE_ARTICLE_GORDO | 9 | 0 (0) | 3 | present |  |
| dedede_jethammer | rewritten | FIGHTER_DEDEDE_GENERATE_ARTICLE_JETHAMMER | 2 | 6 (2) | 0 | absent |  |
| dedede_star | rewritten | FIGHTER_DEDEDE_GENERATE_ARTICLE_STAR | 1 | 4 (4) | 0 | present |  |
| diddy_gun | vanilla | FIGHTER_DIDDY_GENERATE_ARTICLE_GUN | 0 | 4 (2) | 2 | present |  |
| edge_fire | rewritten | FIGHTER_EDGE_GENERATE_ARTICLE_FIRE | 9 | 6 (6) | 0 | present | Life: computed `life (reads param_fire.life_s, param_fire.life_m, param_fire.life_l)`, computed `life (reads param_fire.life_s, param_fire.life_m, param_fire.life_l)`, computed `life (reads param_fire.life_s, param_fire.life_m, param_fire.life_l)`, computed `life (reads param_fire.life_s, param_fire.life_m, param_fire.life_l)`, computed `life (reads param_fire.life_s, param_fire.life_m, param_fire.life_l)`, computed `life (reads param_fire.life_s, param_fire.life_m, param_fire.life_l)` |
| edge_flare1 | rewritten | FIGHTER_EDGE_GENERATE_ARTICLE_FLARE1 | 1 | 2 (2) | 0 | present | Max speed X: computed `vec.x`; Max speed Y: literal 0; Speed X: computed `vec.x`; Speed Y: literal 0 |
| edge_flare2 | rewritten | FIGHTER_EDGE_GENERATE_ARTICLE_FLARE2 | 3 | 0 (0) | 0 | present |  |
| edge_flash | rewritten | FIGHTER_EDGE_GENERATE_ARTICLE_FLASH | 4 | 2 (2) | 0 | present |  |
| kamui_dragonhand | rewritten | FIGHTER_KAMUI_GENERATE_ARTICLE_DRAGONHAND | 6 | 8 (0) | 0 | absent |  |
| kamui_ryusensya | rewritten | FIGHTER_KAMUI_GENERATE_ARTICLE_RYUSENSYA | 2 | 0 (0) | 0 | present |  |
| kamui_spearhand | rewritten | FIGHTER_KAMUI_GENERATE_ARTICLE_SPEARHAND | 3 | 24 (7) | 0 | present |  |
| kamui_waterdragon | rewritten | FIGHTER_KAMUI_GENERATE_ARTICLE_WATERDRAGON | 2 | 13 (3) | 2 | absent |  |
| koopajr_cannonball | rewritten | FIGHTER_KOOPAJR_GENERATE_ARTICLE_CANNONBALL | 1 | 0 (0) | 0 | present |  |
| koopajr_hammer | vanilla | FIGHTER_KOOPAJR_GENERATE_ARTICLE_HAMMER | 0 | 2 (0) | 0 | absent |  |
| koopajr_kart | vanilla | FIGHTER_KOOPAJR_GENERATE_ARTICLE_KART | 0 | 0 (0) | 2 | absent |  |
| koopajr_magichand | vanilla | FIGHTER_KOOPAJR_GENERATE_ARTICLE_MAGICHAND | 0 | 2 (0) | 0 | absent |  |
| krool_backpack | rewritten | FIGHTER_KROOL_GENERATE_ARTICLE_BACKPACK | 2 | 0 (0) | 12 | present |  |
| krool_blunderbuss | vanilla | FIGHTER_KROOL_GENERATE_ARTICLE_BLUNDERBUSS | 0 | 0 (0) | 4 | absent |  |
| krool_crown | rewritten | FIGHTER_KROOL_GENERATE_ARTICLE_CROWN | 1 | 0 (0) | 2 | present |  |
| krool_ironball | rewritten | FIGHTER_KROOL_GENERATE_ARTICLE_IRONBALL | 2 | 0 (0) | 0 | present |  |
| krool_piratehat | vanilla | FIGHTER_KROOL_GENERATE_ARTICLE_PIRATEHAT | 0 | 0 (0) | 2 | absent |  |
| link_boomerang | rewritten | FIGHTER_LINK_GENERATE_ARTICLE_BOOMERANG | 4 | 0 (0) | 0 | no param file |  |
| link_bow | vanilla | FIGHTER_LINK_GENERATE_ARTICLE_BOW | 0 | 0 (0) | 4 | no param file |  |
| link_bowarrow | vanilla | FIGHTER_LINK_GENERATE_ARTICLE_BOWARROW | 0 | 0 (0) | 1 | no param file |  |
| link_swordbeam | vanilla | FIGHTER_LINK_GENERATE_ARTICLE_SWORDBEAM | 0 | 1 (1) | 0 | no param file |  |
| lucario_auraball | rewritten | FIGHTER_LUCARIO_GENERATE_ARTICLE_AURABALL | 2 | 6 (0) | 0 | present | Size: computed `scale`, computed `scale` |
| lucario_qigong | rewritten | FIGHTER_LUCARIO_GENERATE_ARTICLE_QIGONG | 2 | 2 (2) | 0 | present |  |
| luigi_fireball | rewritten | FIGHTER_LUIGI_GENERATE_ARTICLE_FIREBALL | 1 | 2 (2) | 0 | present |  |
| luigi_obakyumu | rewritten | FIGHTER_LUIGI_GENERATE_ARTICLE_OBAKYUMU | 2 | 10 (2) | 0 | present |  |
| luigi_plunger | vanilla | FIGHTER_LUIGI_GENERATE_ARTICLE_PLUNGER | 0 | 5 (1) | 0 | present |  |
| mario_cappy | vanilla | FIGHTER_MARIO_GENERATE_ARTICLE_CAPPY | 0 | 2 (2) | 2 | absent |  |
| mario_fireball | rewritten | FIGHTER_MARIO_GENERATE_ARTICLE_FIREBALL | 1 | 2 (2) | 0 | present |  |
| mariod_drcapsule | rewritten | FIGHTER_MARIOD_GENERATE_ARTICLE_DRCAPSULE | 1 | 6 (2) | 2 | present |  |
| mariod_drmantle | vanilla | FIGHTER_MARIOD_GENERATE_ARTICLE_DRMANTLE | 0 | 6 (2) | 0 | absent |  |
| mewtwo_bindball | rewritten | FIGHTER_MEWTWO_GENERATE_ARTICLE_BINDBALL | 2 | 2 (2) | 0 | present |  |
| mewtwo_escapeairdummy | vanilla | FIGHTER_MEWTWO_GENERATE_ARTICLE_ESCAPEAIRDUMMY | 0 | 2 (0) | 0 | absent |  |
| mewtwo_shadowball | rewritten | FIGHTER_MEWTWO_GENERATE_ARTICLE_SHADOWBALL | 4 | 0 (0) | 0 | present |  |
| miiswordsman_chakram | rewritten | FIGHTER_MIISWORDSMAN_GENERATE_ARTICLE_CHAKRAM | 3 | 6 (0) | 0 | present |  |
| miiswordsman_lightshuriken | rewritten | FIGHTER_MIISWORDSMAN_GENERATE_ARTICLE_LIGHTSHURIKEN | 2 | 4 (2) | 0 | present |  |
| miiswordsman_tornadoshot | rewritten | FIGHTER_MIISWORDSMAN_GENERATE_ARTICLE_TORNADOSHOT | 4 | 4 (4) | 0 | present |  |
| murabito_bowlingball | rewritten | FIGHTER_MURABITO_GENERATE_ARTICLE_BOWLINGBALL | 1 | 2 (0) | 0 | present |  |
| murabito_bullet | rewritten | FIGHTER_MURABITO_GENERATE_ARTICLE_BULLET | 1 | 0 (0) | 0 | present |  |
| murabito_butterflynet | vanilla | FIGHTER_MURABITO_GENERATE_ARTICLE_BUTTERFLYNET | 0 | 1 (0) | 0 | absent |  |
| murabito_clayrocket | rewritten | FIGHTER_MURABITO_GENERATE_ARTICLE_CLAYROCKET | 7 | 0 (0) | 0 | present |  |
| murabito_firework | rewritten | FIGHTER_MURABITO_GENERATE_ARTICLE_FIREWORK | 1 | 2 (1) | 0 | absent |  |
| murabito_flowerpot | rewritten | FIGHTER_MURABITO_GENERATE_ARTICLE_FLOWERPOT | 2 | 2 (1) | 0 | present |  |
| murabito_seed | vanilla | FIGHTER_MURABITO_GENERATE_ARTICLE_SEED | 0 | 3 (1) | 0 | absent |  |
| murabito_slingshot | vanilla | FIGHTER_MURABITO_GENERATE_ARTICLE_SLINGSHOT | 0 | 1 (1) | 0 | absent |  |
| murabito_sprout | rewritten | FIGHTER_MURABITO_GENERATE_ARTICLE_SPROUT | 2 | 6 (0) | 0 | present |  |
| murabito_tree | rewritten | FIGHTER_MURABITO_GENERATE_ARTICLE_TREE | 5 | 0 (0) | 0 | present |  |
| murabito_umbrella | vanilla | FIGHTER_MURABITO_GENERATE_ARTICLE_UMBRELLA | 0 | 2 (1) | 0 | absent |  |
| murabito_weeds | vanilla | FIGHTER_MURABITO_GENERATE_ARTICLE_WEEDS | 0 | 2 (1) | 0 | absent |  |
| packun_firebreath | clone of mario/WEAPON_KIND_MARIO_FIREBALL | none | 3 | 0 (0) | 0 | no param file | Life: param param_firebreath/life, param param_firebreath/life, param param_firebreath/life, param param_firebreath/life |
| pacman_firehydrant | rewritten | FIGHTER_PACMAN_GENERATE_ARTICLE_FIREHYDRANT | 1 | 0 (0) | 0 | present |  |
| pacman_trampoline | rewritten | FIGHTER_PACMAN_GENERATE_ARTICLE_TRAMPOLINE | 1 | 2 (2) | 0 | present |  |
| palutena_autoaimbullet | rewritten | FIGHTER_PALUTENA_GENERATE_ARTICLE_AUTOAIMBULLET | 2 | 2 (2) | 0 | present |  |
| palutena_explosiveflame | rewritten | FIGHTER_PALUTENA_GENERATE_ARTICLE_EXPLOSIVEFLAME | 3 | 0 (0) | 1 | present |  |
| palutena_godwing | vanilla | FIGHTER_PALUTENA_GENERATE_ARTICLE_GODWING | 0 | 12 (4) | 1 | present |  |
| palutena_reflectionboard | rewritten | FIGHTER_PALUTENA_GENERATE_ARTICLE_REFLECTIONBOARD | 2 | 2 (2) | 0 | present |  |
| palutena_meteor | clone of ryu/WEAPON_KIND_RYU_HADOKEN | none | 3 | 0 (0) | 0 | absent | Acceleration X: literal 0, literal 0; Acceleration Y: literal 0, literal 0; Life: param param_meteor/life_s, param param_meteor/life_s; Settling speed X: computed `speed_x (reads param_meteor.speed_s)`, computed `speed_x (reads param_meteor.speed_s)`; Settling speed Y: computed `-speed_y (reads param_meteor.speed_s)`, computed `-speed_y (reads param_meteor.speed_s)`; Size: literal 2.65; Speed X: computed `speed_x (reads param_meteor.speed_s)`, computed `speed_x (reads param_meteor.speed_s)`; Speed Y: computed `-speed_y (reads param_meteor.speed_s)`, computed `-speed_y (reads param_meteor.speed_s)` |
| peach_kassar | vanilla | FIGHTER_PEACH_GENERATE_ARTICLE_KASSAR | 0 | 5 (2) | 0 | absent |  |
| peach_kinopio | vanilla | FIGHTER_PEACH_GENERATE_ARTICLE_KINOPIO | 0 | 19 (0) | 0 | absent |  |
| peach_kinopiospore | rewritten | FIGHTER_PEACH_GENERATE_ARTICLE_KINOPIOSPORE | 2 | 12 (12) | 0 | present |  |
| peach_peachdaikon | vanilla | FIGHTER_PEACH_GENERATE_ARTICLE_PEACHDAIKON | 0 | 1 (1) | 0 | present |  |
| pfushigisou_leafcutter | vanilla | FIGHTER_PFUSHIGISOU_GENERATE_ARTICLE_LEAFCUTTER | 0 | 2 (2) | 0 | present |  |
| pfushigisou_seed | rewritten | FIGHTER_PFUSHIGISOU_GENERATE_ARTICLE_SEED | 9 | 5 (5) | 0 | present | Acceleration X: literal 0; Gravity: literal 0.102; Speed X: literal 0, computed `speed_x`; Speed Y: literal 0, computed `speed_y` |
| pfushigisou_vine | vanilla | FIGHTER_PFUSHIGISOU_GENERATE_ARTICLE_VINE | 0 | 4 (2) | 0 | present |  |
| pichu_dengekidama | rewritten | FIGHTER_PICHU_GENERATE_ARTICLE_DENGEKIDAMA | 2 | 0 (0) | 0 | present |  |
| pichu_kaminari | rewritten | FIGHTER_PICHU_GENERATE_ARTICLE_KAMINARI | 1 | 0 (0) | 0 | present |  |
| pikachu_kaminari | rewritten | FIGHTER_PIKACHU_GENERATE_ARTICLE_KAMINARI | 1 | 0 (0) | 0 | present |  |
| pikachu_specialupdummy | vanilla | FIGHTER_PIKACHU_GENERATE_ARTICLE_SPECIALUPDUMMY | 0 | 0 (0) | 3 | absent |  |
| plizardon_rock | clone of link/WEAPON_KIND_LINK_BOOMERANG | none | 0 | 0 (0) | 0 | no param file |  |
| plizardon_rockstone | clone of sheik/WEAPON_KIND_SHEIK_NEEDLE | none | 1 | 0 (0) | 0 | no param file | Acceleration X: computed `-speed_x * lr * 0.05`; Acceleration Y: computed `-speed_y * 0.05`; Life: computed `life`, computed `life`, computed `life`, computed `life`; Speed X: computed `speed_x`; Speed Y: computed `speed_y` |
| purin_disarmingvoice | clone of koopajr/WEAPON_KIND_KOOPAJR_CANNONBALL | none | 2 | 0 (0) | 0 | absent | Acceleration X: literal 0; Acceleration Y: literal 0; Settling speed X: computed `speed_including_momentum`; Settling speed Y: literal 0; Size: literal 0.001; Speed X: computed `speed_including_momentum`; Speed Y: literal 0 |
| richter_axe | rewritten | FIGHTER_RICHTER_GENERATE_ARTICLE_AXE | 3 | 0 (0) | 0 | present |  |
| richter_whip | rewritten | FIGHTER_RICHTER_GENERATE_ARTICLE_WHIP | 40 | 0 (0) | 0 | present |  |
| ridley_breath | rewritten | FIGHTER_RIDLEY_GENERATE_ARTICLE_BREATH | 1 | 0 (0) | 0 | present |  |
| rockman_airshooter | rewritten | FIGHTER_ROCKMAN_GENERATE_ARTICLE_AIRSHOOTER | 1 | 1 (1) | 0 | present | Acceleration X: literal 0; Acceleration Y: param param_airshooter/accel; Life: param param_airshooter/life, param param_airshooter/life; Max speed X: literal 0, literal 0; Max speed Y: param param_airshooter/speed, param param_airshooter/limit_speed; Speed X: literal 0; Speed Y: param param_airshooter/speed |
| rockman_chargeshot | rewritten | FIGHTER_ROCKMAN_GENERATE_ARTICLE_CHARGESHOT | 1 | 2 (2) | 0 | present | Acceleration X: literal 0; Acceleration Y: literal 0; Life: computed `life`, computed `life`; Settling speed X: literal -1; Settling speed Y: literal -1; Size: computed `scale`; Speed X: computed `speed`; Speed Y: literal 0 |
| rockman_crashbomb | vanilla | FIGHTER_ROCKMAN_GENERATE_ARTICLE_CRASHBOMB | 0 | 4 (0) | 0 | present |  |
| rockman_hardknuckle | rewritten | FIGHTER_ROCKMAN_GENERATE_ARTICLE_HARDKNUCKLE | 4 | 1 (1) | 0 | present |  |
| rockman_leafshield | rewritten | FIGHTER_ROCKMAN_GENERATE_ARTICLE_LEAFSHIELD | 6 | 2 (0) | 0 | present |  |
| rockman_metalblade | vanilla | FIGHTER_ROCKMAN_GENERATE_ARTICLE_METALBLADE | 0 | 0 (0) | 1 | present |  |
| rockman_rockbuster | rewritten | FIGHTER_ROCKMAN_GENERATE_ARTICLE_ROCKBUSTER | 1 | 3 (0) | 0 | present |  |
| sonic_gimmickjump | rewritten | FIGHTER_SONIC_GENERATE_ARTICLE_GIMMICKJUMP | 1 | 0 (0) | 0 | present |  |
| toonlink_boomerang | rewritten | FIGHTER_TOONLINK_GENERATE_ARTICLE_BOOMERANG | 2 | 0 (0) | 0 | present |  |
| toonlink_hookshot | vanilla | FIGHTER_TOONLINK_GENERATE_ARTICLE_HOOKSHOT | 0 | 6 (1) | 0 | present |  |
| toonlink_hookshothand | vanilla | FIGHTER_TOONLINK_GENERATE_ARTICLE_HOOKSHOTHAND | 0 | 6 (1) | 0 | absent |  |
| wario_wariobike | vanilla | FIGHTER_WARIO_GENERATE_ARTICLE_WARIOBIKE | 0 | 0 (0) | 1 | present |  |
| younglink_boomerang | rewritten | FIGHTER_YOUNGLINK_GENERATE_ARTICLE_BOOMERANG | 3 | 4 (2) | 0 | present |  |
| younglink_bowarrow | rewritten | FIGHTER_YOUNGLINK_GENERATE_ARTICLE_BOWARROW | 1 | 0 (0) | 0 | present |  |
| younglink_hookshot | vanilla | FIGHTER_YOUNGLINK_GENERATE_ARTICLE_HOOKSHOT | 0 | 16 (4) | 0 | present |  |
| younglink_hookshothand | vanilla | FIGHTER_YOUNGLINK_GENERATE_ARTICLE_HOOKSHOTHAND | 0 | 16 (4) | 0 | absent |  |

### Unresolved ids (scripts) (636)

- `vendor/hdr/fighters/brave/src/acmd/specials/special_n.rs:39` brave/game_specialair2 `*FIGHTER_BRAVE_GENERATE_ARTICLE_FIREBALL` (generate_article) — environment: brave is not in the local dump, so its vanilla `fireball` is not listed
- `vendor/hdr/fighters/brave/src/acmd/specials/special_hi.rs:47` brave/game_specialairhi1 `*FIGHTER_BRAVE_GENERATE_ARTICLE_TORNADO` (generate_article) — environment: brave is not in the local dump, so its vanilla `tornado` is not listed
- `vendor/hdr/fighters/brave/src/acmd/specials/special_hi.rs:77` brave/game_specialairhi2 `*FIGHTER_BRAVE_GENERATE_ARTICLE_TORNADO` (generate_article) — environment: brave is not in the local dump, so its vanilla `tornado` is not listed
- `vendor/hdr/fighters/brave/src/acmd/specials/special_hi.rs:116` brave/game_specialairhi3 `*FIGHTER_BRAVE_GENERATE_ARTICLE_TORNADO` (generate_article) — environment: brave is not in the local dump, so its vanilla `tornado` is not listed
- `vendor/hdr/fighters/brave/src/acmd/specials/special_lw.rs:107` brave/game_specialairlw14 `*FIGHTER_BRAVE_GENERATE_ARTICLE_SLEEP` (generate_article) — environment: brave is not in the local dump, so its vanilla `sleep` is not listed
- `vendor/hdr/fighters/brave/src/acmd/specials/special_lw.rs:18` brave/game_specialairlw8 `*FIGHTER_BRAVE_GENERATE_ARTICLE_CRASH` (generate_article) — environment: brave is not in the local dump, so its vanilla `crash` is not listed
- `vendor/hdr/fighters/brave/src/acmd/specials/special_n.rs:12` brave/game_specialairn1 `*FIGHTER_BRAVE_GENERATE_ARTICLE_FIREBALL` (generate_article) — environment: brave is not in the local dump, so its vanilla `fireball` is not listed
- `vendor/hdr/fighters/brave/src/acmd/specials/special_n.rs:63` brave/game_specialairn3 `*FIGHTER_BRAVE_GENERATE_ARTICLE_FIREBALL` (generate_article) — environment: brave is not in the local dump, so its vanilla `fireball` is not listed
- `vendor/hdr/fighters/brave/src/acmd/specials/special_hi.rs:47` brave/game_specialhi1 `*FIGHTER_BRAVE_GENERATE_ARTICLE_TORNADO` (generate_article) — environment: brave is not in the local dump, so its vanilla `tornado` is not listed
- `vendor/hdr/fighters/brave/src/acmd/specials/special_hi.rs:77` brave/game_specialhi2 `*FIGHTER_BRAVE_GENERATE_ARTICLE_TORNADO` (generate_article) — environment: brave is not in the local dump, so its vanilla `tornado` is not listed
- `vendor/hdr/fighters/brave/src/acmd/specials/special_hi.rs:116` brave/game_specialhi3 `*FIGHTER_BRAVE_GENERATE_ARTICLE_TORNADO` (generate_article) — environment: brave is not in the local dump, so its vanilla `tornado` is not listed
- `vendor/hdr/fighters/brave/src/acmd/specials/special_lw.rs:107` brave/game_speciallw14 `*FIGHTER_BRAVE_GENERATE_ARTICLE_SLEEP` (generate_article) — environment: brave is not in the local dump, so its vanilla `sleep` is not listed
- `vendor/hdr/fighters/brave/src/acmd/specials/special_lw.rs:18` brave/game_speciallw8 `*FIGHTER_BRAVE_GENERATE_ARTICLE_CRASH` (generate_article) — environment: brave is not in the local dump, so its vanilla `crash` is not listed
- `vendor/hdr/fighters/brave/src/acmd/specials/special_n.rs:12` brave/game_specialn1 `*FIGHTER_BRAVE_GENERATE_ARTICLE_FIREBALL` (generate_article) — environment: brave is not in the local dump, so its vanilla `fireball` is not listed
- `vendor/hdr/fighters/brave/src/acmd/specials/special_n.rs:39` brave/game_specialn2 `*FIGHTER_BRAVE_GENERATE_ARTICLE_FIREBALL` (generate_article) — environment: brave is not in the local dump, so its vanilla `fireball` is not listed
- `vendor/hdr/fighters/brave/src/acmd/specials/special_n.rs:63` brave/game_specialn3 `*FIGHTER_BRAVE_GENERATE_ARTICLE_FIREBALL` (generate_article) — environment: brave is not in the local dump, so its vanilla `fireball` is not listed
- `vendor/hdr/fighters/cloud/src/acmd/specials/specialn.rs:24` cloud/game_specialairn `*FIGHTER_CLOUD_GENERATE_ARTICLE_WAVE` (generate_article) — cloud has no `wave` folder in the dump and no clone binds this name
- `vendor/hdr/fighters/cloud/src/acmd/specials/specialn.rs:97` cloud/game_specialairn_lb `*FIGHTER_CLOUD_GENERATE_ARTICLE_WAVE` (generate_article) — cloud has no `wave` folder in the dump and no clone binds this name
- `vendor/hdr/fighters/cloud/src/acmd/specials/specialn.rs:24` cloud/game_specialn `*FIGHTER_CLOUD_GENERATE_ARTICLE_WAVE` (generate_article) — cloud has no `wave` folder in the dump and no clone binds this name
- `vendor/hdr/fighters/cloud/src/acmd/specials/specialn.rs:97` cloud/game_specialn_lb `*FIGHTER_CLOUD_GENERATE_ARTICLE_WAVE` (generate_article) — cloud has no `wave` folder in the dump and no clone binds this name
- `vendor/hdr/fighters/dolly/src/acmd/specials/super_special.rs:16` dolly/game_superspecial `*FIGHTER_DOLLY_GENERATE_ARTICLE_BURST` (generate_article) — environment: dolly is not in the local dump, so its vanilla `burst` is not listed
- `vendor/hdr/fighters/dolly/src/acmd/specials/super_special.rs:214` dolly/game_superspecial2start `*FIGHTER_DOLLY_GENERATE_ARTICLE_FIRE` (generate_article) — environment: dolly is not in the local dump, so its vanilla `fire` is not listed
- `vendor/hdr/fighters/dolly/src/acmd/specials/super_special.rs:44` dolly/game_superspecialtriple `*FIGHTER_DOLLY_GENERATE_ARTICLE_BURST` (generate_article) — environment: dolly is not in the local dump, so its vanilla `burst` is not listed
- `vendor/hdr/fighters/dolly/src/acmd/specials/super_special.rs:44` dolly/game_superspecialtriple `*FIGHTER_DOLLY_GENERATE_ARTICLE_BURST` (generate_article) — environment: dolly is not in the local dump, so its vanilla `burst` is not listed
- `vendor/hdr/fighters/dolly/src/acmd/specials/super_special.rs:44` dolly/game_superspecialtriple `*FIGHTER_DOLLY_GENERATE_ARTICLE_BURST` (generate_article) — environment: dolly is not in the local dump, so its vanilla `burst` is not listed
- `vendor/hdr/fighters/eflame/src/acmd/ground.rs:30` eflame/game_attack12 `*FIGHTER_EFLAME_GENERATE_ARTICLE_ESWORD` (add_motion_partial) — environment: eflame is not in the local dump, so its vanilla `esword` is not listed
- `vendor/hdr/fighters/eflame/src/acmd/ground.rs:59` eflame/game_attack12 `*FIGHTER_EFLAME_GENERATE_ARTICLE_ESWORD` (add_motion_partial) — environment: eflame is not in the local dump, so its vanilla `esword` is not listed
- `vendor/hdr/fighters/eflame/src/acmd/ground.rs:121` eflame/game_attack13 `*FIGHTER_EFLAME_GENERATE_ARTICLE_ESWORD` (add_motion_partial) — environment: eflame is not in the local dump, so its vanilla `esword` is not listed
- `vendor/hdr/fighters/eflame/src/acmd/ground.rs:160` eflame/game_attack13 `*FIGHTER_EFLAME_GENERATE_ARTICLE_ESWORD` (add_motion_partial) — environment: eflame is not in the local dump, so its vanilla `esword` is not listed
- `vendor/hdr/fighters/eflame/src/acmd/aerials.rs:212` eflame/game_attackairb `*FIGHTER_EFLAME_GENERATE_ARTICLE_ESWORD` (add_motion_partial) — environment: eflame is not in the local dump, so its vanilla `esword` is not listed
- `vendor/hdr/fighters/eflame/src/acmd/aerials.rs:232` eflame/game_attackairb `*FIGHTER_EFLAME_GENERATE_ARTICLE_ESWORD` (add_motion_partial) — environment: eflame is not in the local dump, so its vanilla `esword` is not listed
- `vendor/hdr/fighters/eflame/src/acmd/aerials.rs:113` eflame/game_attackairf `*FIGHTER_EFLAME_GENERATE_ARTICLE_ESWORD` (add_motion_partial) — environment: eflame is not in the local dump, so its vanilla `esword` is not listed
- `vendor/hdr/fighters/eflame/src/acmd/aerials.rs:135` eflame/game_attackairf `*FIGHTER_EFLAME_GENERATE_ARTICLE_ESWORD` (add_motion_partial) — environment: eflame is not in the local dump, so its vanilla `esword` is not listed
- `vendor/hdr/fighters/eflame/src/acmd/aerials.rs:303` eflame/game_attackairhi `*FIGHTER_EFLAME_GENERATE_ARTICLE_ESWORD` (add_motion_partial) — environment: eflame is not in the local dump, so its vanilla `esword` is not listed
- `vendor/hdr/fighters/eflame/src/acmd/aerials.rs:323` eflame/game_attackairhi `*FIGHTER_EFLAME_GENERATE_ARTICLE_ESWORD` (add_motion_partial) — environment: eflame is not in the local dump, so its vanilla `esword` is not listed
- `vendor/hdr/fighters/eflame/src/acmd/aerials.rs:406` eflame/game_attackairlw `*FIGHTER_EFLAME_GENERATE_ARTICLE_ESWORD` (add_motion_partial) — environment: eflame is not in the local dump, so its vanilla `esword` is not listed
- `vendor/hdr/fighters/eflame/src/acmd/aerials.rs:426` eflame/game_attackairlw `*FIGHTER_EFLAME_GENERATE_ARTICLE_ESWORD` (add_motion_partial) — environment: eflame is not in the local dump, so its vanilla `esword` is not listed
- `vendor/hdr/fighters/eflame/src/acmd/aerials.rs:13` eflame/game_attackairn `*FIGHTER_EFLAME_GENERATE_ARTICLE_ESWORD` (add_motion_partial) — environment: eflame is not in the local dump, so its vanilla `esword` is not listed
- `vendor/hdr/fighters/eflame/src/acmd/aerials.rs:41` eflame/game_attackairn `*FIGHTER_EFLAME_GENERATE_ARTICLE_ESWORD` (add_motion_partial) — environment: eflame is not in the local dump, so its vanilla `esword` is not listed
- `vendor/hdr/fighters/eflame/src/acmd/ground.rs:221` eflame/game_attackdash `*FIGHTER_EFLAME_GENERATE_ARTICLE_ESWORD` (add_motion_partial) — environment: eflame is not in the local dump, so its vanilla `esword` is not listed
- `vendor/hdr/fighters/eflame/src/acmd/ground.rs:241` eflame/game_attackdash `*FIGHTER_EFLAME_GENERATE_ARTICLE_ESWORD` (add_motion_partial) — environment: eflame is not in the local dump, so its vanilla `esword` is not listed
- `vendor/hdr/fighters/eflame/src/acmd/tilts.rs:94` eflame/game_attackhi3 `*FIGHTER_EFLAME_GENERATE_ARTICLE_ESWORD` (add_motion_partial) — environment: eflame is not in the local dump, so its vanilla `esword` is not listed
- `vendor/hdr/fighters/eflame/src/acmd/tilts.rs:114` eflame/game_attackhi3 `*FIGHTER_EFLAME_GENERATE_ARTICLE_ESWORD` (add_motion_partial) — environment: eflame is not in the local dump, so its vanilla `esword` is not listed
- `vendor/hdr/fighters/eflame/src/acmd/smashes.rs:123` eflame/game_attackhi4 `*FIGHTER_EFLAME_GENERATE_ARTICLE_ESWORD` (add_motion_partial) — environment: eflame is not in the local dump, so its vanilla `esword` is not listed
- `vendor/hdr/fighters/eflame/src/acmd/smashes.rs:178` eflame/game_attackhi4 `*FIGHTER_EFLAME_GENERATE_ARTICLE_ESWORD` (add_motion_partial) — environment: eflame is not in the local dump, so its vanilla `esword` is not listed
- `vendor/hdr/fighters/eflame/src/acmd/tilts.rs:184` eflame/game_attacklw3 `*FIGHTER_EFLAME_GENERATE_ARTICLE_ESWORD` (add_motion_partial) — environment: eflame is not in the local dump, so its vanilla `esword` is not listed
- `vendor/hdr/fighters/eflame/src/acmd/tilts.rs:205` eflame/game_attacklw3 `*FIGHTER_EFLAME_GENERATE_ARTICLE_ESWORD` (add_motion_partial) — environment: eflame is not in the local dump, so its vanilla `esword` is not listed
- `vendor/hdr/fighters/eflame/src/acmd/smashes.rs:259` eflame/game_attacklw4 `*FIGHTER_EFLAME_GENERATE_ARTICLE_ESWORD` (add_motion_partial) — environment: eflame is not in the local dump, so its vanilla `esword` is not listed
- `vendor/hdr/fighters/eflame/src/acmd/smashes.rs:304` eflame/game_attacklw4 `*FIGHTER_EFLAME_GENERATE_ARTICLE_ESWORD` (add_motion_partial) — environment: eflame is not in the local dump, so its vanilla `esword` is not listed
- `vendor/hdr/fighters/eflame/src/acmd/tilts.rs:13` eflame/game_attacks3 `*FIGHTER_EFLAME_GENERATE_ARTICLE_ESWORD` (add_motion_partial) — environment: eflame is not in the local dump, so its vanilla `esword` is not listed
- `vendor/hdr/fighters/eflame/src/acmd/tilts.rs:33` eflame/game_attacks3 `*FIGHTER_EFLAME_GENERATE_ARTICLE_ESWORD` (add_motion_partial) — environment: eflame is not in the local dump, so its vanilla `esword` is not listed
- `vendor/hdr/fighters/eflame/src/acmd/smashes.rs:10` eflame/game_attacks4 `*FIGHTER_EFLAME_GENERATE_ARTICLE_ESWORD` (add_motion_partial) — environment: eflame is not in the local dump, so its vanilla `esword` is not listed
- `vendor/hdr/fighters/eflame/src/acmd/smashes.rs:55` eflame/game_attacks4 `*FIGHTER_EFLAME_GENERATE_ARTICLE_ESWORD` (add_motion_partial) — environment: eflame is not in the local dump, so its vanilla `esword` is not listed
- `vendor/hdr/fighters/eflame/src/acmd/specials.rs:69` eflame/game_specialairhijump `*FIGHTER_EFLAME_GENERATE_ARTICLE_ESWORD` (add_motion_partial) — environment: eflame is not in the local dump, so its vanilla `esword` is not listed
- `vendor/hdr/fighters/eflame/src/acmd/specials.rs:82` eflame/game_specialairhijump `*FIGHTER_EFLAME_GENERATE_ARTICLE_ESWORD` (add_motion_partial) — environment: eflame is not in the local dump, so its vanilla `esword` is not listed
- `vendor/hdr/fighters/eflame/src/acmd/specials.rs:9` eflame/game_specialairs `*FIGHTER_EFLAME_GENERATE_ARTICLE_ESWORD` (add_motion_partial) — environment: eflame is not in the local dump, so its vanilla `esword` is not listed
- `vendor/hdr/fighters/eflame/src/acmd/specials.rs:22` eflame/game_specialairs `*FIGHTER_EFLAME_GENERATE_ARTICLE_ESWORD` (shoot) — environment: eflame is not in the local dump, so its vanilla `esword` is not listed
- `vendor/hdr/fighters/eflame/src/acmd/specials.rs:9` eflame/game_specialairsflick `*FIGHTER_EFLAME_GENERATE_ARTICLE_ESWORD` (add_motion_partial) — environment: eflame is not in the local dump, so its vanilla `esword` is not listed
- `vendor/hdr/fighters/eflame/src/acmd/specials.rs:22` eflame/game_specialairsflick `*FIGHTER_EFLAME_GENERATE_ARTICLE_ESWORD` (shoot) — environment: eflame is not in the local dump, so its vanilla `esword` is not listed
- `vendor/hdr/fighters/eflame/src/acmd/specials.rs:9` eflame/game_specials `*FIGHTER_EFLAME_GENERATE_ARTICLE_ESWORD` (add_motion_partial) — environment: eflame is not in the local dump, so its vanilla `esword` is not listed
- `vendor/hdr/fighters/eflame/src/acmd/specials.rs:22` eflame/game_specials `*FIGHTER_EFLAME_GENERATE_ARTICLE_ESWORD` (shoot) — environment: eflame is not in the local dump, so its vanilla `esword` is not listed
- `vendor/hdr/fighters/eflame/src/acmd/specials.rs:9` eflame/game_specialsflick `*FIGHTER_EFLAME_GENERATE_ARTICLE_ESWORD` (add_motion_partial) — environment: eflame is not in the local dump, so its vanilla `esword` is not listed
- `vendor/hdr/fighters/eflame/src/acmd/specials.rs:22` eflame/game_specialsflick `*FIGHTER_EFLAME_GENERATE_ARTICLE_ESWORD` (shoot) — environment: eflame is not in the local dump, so its vanilla `esword` is not listed
- `vendor/hdr/fighters/eflame/src/acmd/throws.rs:98` eflame/game_throwlw `*FIGHTER_EFLAME_GENERATE_ARTICLE_ESWORD` (add_motion_partial) — environment: eflame is not in the local dump, so its vanilla `esword` is not listed
- `vendor/hdr/fighters/eflame/src/acmd/throws.rs:126` eflame/game_throwlw `*FIGHTER_EFLAME_GENERATE_ARTICLE_ESWORD` (add_motion_partial) — environment: eflame is not in the local dump, so its vanilla `esword` is not listed
- `vendor/hdr/fighters/elight/src/acmd/ground.rs:31` elight/game_attack12 `*FIGHTER_ELIGHT_GENERATE_ARTICLE_ESWORD` (add_motion_partial) — environment: elight is not in the local dump, so its vanilla `esword` is not listed
- `vendor/hdr/fighters/elight/src/acmd/ground.rs:62` elight/game_attack12 `*FIGHTER_ELIGHT_GENERATE_ARTICLE_ESWORD` (add_motion_partial) — environment: elight is not in the local dump, so its vanilla `esword` is not listed
- `vendor/hdr/fighters/elight/src/acmd/ground.rs:117` elight/game_attack13 `*FIGHTER_ELIGHT_GENERATE_ARTICLE_ESWORD` (add_motion_partial) — environment: elight is not in the local dump, so its vanilla `esword` is not listed
- `vendor/hdr/fighters/elight/src/acmd/ground.rs:156` elight/game_attack13 `*FIGHTER_ELIGHT_GENERATE_ARTICLE_ESWORD` (add_motion_partial) — environment: elight is not in the local dump, so its vanilla `esword` is not listed
- `vendor/hdr/fighters/elight/src/acmd/aerials.rs:198` elight/game_attackairb `*FIGHTER_ELIGHT_GENERATE_ARTICLE_ESWORD` (add_motion_partial) — environment: elight is not in the local dump, so its vanilla `esword` is not listed
- `vendor/hdr/fighters/elight/src/acmd/aerials.rs:215` elight/game_attackairb `*FIGHTER_ELIGHT_GENERATE_ARTICLE_ESWORD` (add_motion_partial) — environment: elight is not in the local dump, so its vanilla `esword` is not listed
- `vendor/hdr/fighters/elight/src/acmd/aerials.rs:121` elight/game_attackairf `*FIGHTER_ELIGHT_GENERATE_ARTICLE_ESWORD` (add_motion_partial) — environment: elight is not in the local dump, so its vanilla `esword` is not listed
- `vendor/hdr/fighters/elight/src/acmd/aerials.rs:137` elight/game_attackairf `*FIGHTER_ELIGHT_GENERATE_ARTICLE_ESWORD` (add_motion_partial) — environment: elight is not in the local dump, so its vanilla `esword` is not listed
- `vendor/hdr/fighters/elight/src/acmd/aerials.rs:276` elight/game_attackairhi `*FIGHTER_ELIGHT_GENERATE_ARTICLE_ESWORD` (add_motion_partial) — environment: elight is not in the local dump, so its vanilla `esword` is not listed
- `vendor/hdr/fighters/elight/src/acmd/aerials.rs:300` elight/game_attackairhi `*FIGHTER_ELIGHT_GENERATE_ARTICLE_ESWORD` (add_motion_partial) — environment: elight is not in the local dump, so its vanilla `esword` is not listed
- `vendor/hdr/fighters/elight/src/acmd/aerials.rs:367` elight/game_attackairlw `*FIGHTER_ELIGHT_GENERATE_ARTICLE_ESWORD` (add_motion_partial) — environment: elight is not in the local dump, so its vanilla `esword` is not listed
- `vendor/hdr/fighters/elight/src/acmd/aerials.rs:383` elight/game_attackairlw `*FIGHTER_ELIGHT_GENERATE_ARTICLE_ESWORD` (add_motion_partial) — environment: elight is not in the local dump, so its vanilla `esword` is not listed
- `vendor/hdr/fighters/elight/src/acmd/aerials.rs:17` elight/game_attackairn `*FIGHTER_ELIGHT_GENERATE_ARTICLE_ESWORD` (add_motion_partial) — environment: elight is not in the local dump, so its vanilla `esword` is not listed
- `vendor/hdr/fighters/elight/src/acmd/aerials.rs:45` elight/game_attackairn `*FIGHTER_ELIGHT_GENERATE_ARTICLE_ESWORD` (add_motion_partial) — environment: elight is not in the local dump, so its vanilla `esword` is not listed
- `vendor/hdr/fighters/elight/src/acmd/ground.rs:218` elight/game_attackdash `*FIGHTER_ELIGHT_GENERATE_ARTICLE_ESWORD` (add_motion_partial) — environment: elight is not in the local dump, so its vanilla `esword` is not listed
- `vendor/hdr/fighters/elight/src/acmd/ground.rs:241` elight/game_attackdash `*FIGHTER_ELIGHT_GENERATE_ARTICLE_ESWORD` (add_motion_partial) — environment: elight is not in the local dump, so its vanilla `esword` is not listed
- `vendor/hdr/fighters/elight/src/acmd/tilts.rs:98` elight/game_attackhi3 `*FIGHTER_ELIGHT_GENERATE_ARTICLE_ESWORD` (add_motion_partial) — environment: elight is not in the local dump, so its vanilla `esword` is not listed
- `vendor/hdr/fighters/elight/src/acmd/tilts.rs:125` elight/game_attackhi3 `*FIGHTER_ELIGHT_GENERATE_ARTICLE_ESWORD` (add_motion_partial) — environment: elight is not in the local dump, so its vanilla `esword` is not listed
- `vendor/hdr/fighters/elight/src/acmd/smashes.rs:111` elight/game_attackhi4 `*FIGHTER_ELIGHT_GENERATE_ARTICLE_ESWORD` (add_motion_partial) — environment: elight is not in the local dump, so its vanilla `esword` is not listed
- `vendor/hdr/fighters/elight/src/acmd/smashes.rs:188` elight/game_attackhi4 `*FIGHTER_ELIGHT_GENERATE_ARTICLE_ESWORD` (add_motion_partial) — environment: elight is not in the local dump, so its vanilla `esword` is not listed
- `vendor/hdr/fighters/elight/src/acmd/tilts.rs:178` elight/game_attacklw3 `*FIGHTER_ELIGHT_GENERATE_ARTICLE_ESWORD` (add_motion_partial) — environment: elight is not in the local dump, so its vanilla `esword` is not listed
- `vendor/hdr/fighters/elight/src/acmd/tilts.rs:208` elight/game_attacklw3 `*FIGHTER_ELIGHT_GENERATE_ARTICLE_ESWORD` (add_motion_partial) — environment: elight is not in the local dump, so its vanilla `esword` is not listed
- `vendor/hdr/fighters/elight/src/acmd/smashes.rs:241` elight/game_attacklw4 `*FIGHTER_ELIGHT_GENERATE_ARTICLE_ESWORD` (add_motion_partial) — environment: elight is not in the local dump, so its vanilla `esword` is not listed
- `vendor/hdr/fighters/elight/src/acmd/smashes.rs:289` elight/game_attacklw4 `*FIGHTER_ELIGHT_GENERATE_ARTICLE_ESWORD` (add_motion_partial) — environment: elight is not in the local dump, so its vanilla `esword` is not listed
- `vendor/hdr/fighters/elight/src/acmd/tilts.rs:11` elight/game_attacks3 `*FIGHTER_ELIGHT_GENERATE_ARTICLE_ESWORD` (add_motion_partial) — environment: elight is not in the local dump, so its vanilla `esword` is not listed
- `vendor/hdr/fighters/elight/src/acmd/tilts.rs:39` elight/game_attacks3 `*FIGHTER_ELIGHT_GENERATE_ARTICLE_ESWORD` (add_motion_partial) — environment: elight is not in the local dump, so its vanilla `esword` is not listed
- `vendor/hdr/fighters/elight/src/acmd/smashes.rs:10` elight/game_attacks4 `*FIGHTER_ELIGHT_GENERATE_ARTICLE_ESWORD` (add_motion_partial) — environment: elight is not in the local dump, so its vanilla `esword` is not listed
- `vendor/hdr/fighters/elight/src/acmd/smashes.rs:46` elight/game_attacks4 `*FIGHTER_ELIGHT_GENERATE_ARTICLE_ESWORD` (add_motion_partial) — environment: elight is not in the local dump, so its vanilla `esword` is not listed
- `vendor/hdr/fighters/elight/src/acmd/specials.rs:1083` elight/game_specialairhistart `*FIGHTER_ELIGHT_GENERATE_ARTICLE_ESWORD` (add_motion_partial) — environment: elight is not in the local dump, so its vanilla `esword` is not listed
- `vendor/hdr/fighters/elight/src/acmd/specials.rs:43` elight/game_specialairn `*FIGHTER_ELIGHT_GENERATE_ARTICLE_ESWORD` (add_motion_partial) — environment: elight is not in the local dump, so its vanilla `esword` is not listed
- `vendor/hdr/fighters/elight/src/acmd/specials.rs:159` elight/game_specialairn `*FIGHTER_ELIGHT_GENERATE_ARTICLE_ESWORD` (add_motion_partial) — environment: elight is not in the local dump, so its vanilla `esword` is not listed
- `vendor/hdr/fighters/elight/src/acmd/specials.rs:708` elight/game_specialairn2 `*FIGHTER_ELIGHT_GENERATE_ARTICLE_ESWORD` (add_motion_partial) — environment: elight is not in the local dump, so its vanilla `esword` is not listed
- `vendor/hdr/fighters/elight/src/acmd/specials.rs:886` elight/game_specialairn2 `*FIGHTER_ELIGHT_GENERATE_ARTICLE_ESWORD` (add_motion_partial) — environment: elight is not in the local dump, so its vanilla `esword` is not listed
- `vendor/hdr/fighters/elight/src/acmd/specials.rs:1026` elight/game_specialairs `*FIGHTER_ELIGHT_GENERATE_ARTICLE_BUNSHIN` (generate_article) — environment: elight is not in the local dump, so its vanilla `bunshin` is not listed
- `vendor/hdr/fighters/elight/src/acmd/specials.rs:1043` elight/game_specialairsend `*FIGHTER_ELIGHT_GENERATE_ARTICLE_ESWORD` (add_motion_partial) — environment: elight is not in the local dump, so its vanilla `esword` is not listed
- `vendor/hdr/fighters/elight/src/acmd/specials.rs:1058` elight/game_specialairsend `*FIGHTER_ELIGHT_GENERATE_ARTICLE_ESWORD` (add_motion_partial) — environment: elight is not in the local dump, so its vanilla `esword` is not listed
- `vendor/hdr/fighters/elight/src/acmd/specials.rs:1083` elight/game_specialhistart `*FIGHTER_ELIGHT_GENERATE_ARTICLE_ESWORD` (add_motion_partial) — environment: elight is not in the local dump, so its vanilla `esword` is not listed
- `vendor/hdr/fighters/elight/src/acmd/specials.rs:43` elight/game_specialn `*FIGHTER_ELIGHT_GENERATE_ARTICLE_ESWORD` (add_motion_partial) — environment: elight is not in the local dump, so its vanilla `esword` is not listed
- `vendor/hdr/fighters/elight/src/acmd/specials.rs:159` elight/game_specialn `*FIGHTER_ELIGHT_GENERATE_ARTICLE_ESWORD` (add_motion_partial) — environment: elight is not in the local dump, so its vanilla `esword` is not listed
- `vendor/hdr/fighters/elight/src/acmd/specials.rs:368` elight/game_specialn2 `*FIGHTER_ELIGHT_GENERATE_ARTICLE_ESWORD` (add_motion_partial) — environment: elight is not in the local dump, so its vanilla `esword` is not listed
- `vendor/hdr/fighters/elight/src/acmd/specials.rs:558` elight/game_specialn2 `*FIGHTER_ELIGHT_GENERATE_ARTICLE_ESWORD` (add_motion_partial) — environment: elight is not in the local dump, so its vanilla `esword` is not listed
- `vendor/hdr/fighters/elight/src/acmd/specials.rs:1026` elight/game_specials `*FIGHTER_ELIGHT_GENERATE_ARTICLE_BUNSHIN` (generate_article) — environment: elight is not in the local dump, so its vanilla `bunshin` is not listed
- `vendor/hdr/fighters/elight/src/acmd/specials.rs:1043` elight/game_specialsend `*FIGHTER_ELIGHT_GENERATE_ARTICLE_ESWORD` (add_motion_partial) — environment: elight is not in the local dump, so its vanilla `esword` is not listed
- `vendor/hdr/fighters/elight/src/acmd/specials.rs:1058` elight/game_specialsend `*FIGHTER_ELIGHT_GENERATE_ARTICLE_ESWORD` (add_motion_partial) — environment: elight is not in the local dump, so its vanilla `esword` is not listed
- `vendor/hdr/fighters/elight/src/acmd/throws.rs:69` elight/game_throwlw `*FIGHTER_ELIGHT_GENERATE_ARTICLE_ESWORD` (add_motion_partial) — environment: elight is not in the local dump, so its vanilla `esword` is not listed
- `vendor/hdr/fighters/elight/src/acmd/throws.rs:69` elight/game_throwlw `*FIGHTER_ELIGHT_GENERATE_ARTICLE_ESWORD` (add_motion_partial) — environment: elight is not in the local dump, so its vanilla `esword` is not listed
- `vendor/hdr/fighters/falco/src/acmd/specials.rs:76` falco/game_specialairnloop `*FIGHTER_FALCO_GENERATE_ARTICLE_BLASTER_BULLET` (generate_article) — environment: falco is not in the local dump, so its vanilla `blaster_bullet` is not listed
- `vendor/hdr/fighters/falco/src/acmd/specials.rs:78` falco/game_specialairnloop `*FIGHTER_FALCO_GENERATE_ARTICLE_BLASTER` (change_motion) — environment: falco is not in the local dump, so its vanilla `blaster` is not listed
- `vendor/hdr/fighters/falco/src/acmd/specials.rs:57` falco/game_specialairnstart `*FIGHTER_FALCO_GENERATE_ARTICLE_BLASTER` (generate_article) — environment: falco is not in the local dump, so its vanilla `blaster` is not listed
- `vendor/hdr/fighters/falco/src/acmd/specials.rs:59` falco/game_specialairnstart `*FIGHTER_FALCO_GENERATE_ARTICLE_BLASTER` (change_motion) — environment: falco is not in the local dump, so its vanilla `blaster` is not listed
- `vendor/hdr/fighters/falco/src/acmd/specials.rs:120` falco/game_specialairs `*FIGHTER_FALCO_GENERATE_ARTICLE_ILLUSION` (generate_article) — environment: falco is not in the local dump, so its vanilla `illusion` is not listed
- `vendor/hdr/fighters/falco/src/acmd/specials.rs:120` falco/game_specialairs `*FIGHTER_FALCO_GENERATE_ARTICLE_ILLUSION` (generate_article) — environment: falco is not in the local dump, so its vanilla `illusion` is not listed
- `vendor/hdr/fighters/falco/src/acmd/specials.rs:37` falco/game_specialnloop `*FIGHTER_FALCO_GENERATE_ARTICLE_BLASTER_BULLET` (generate_article) — environment: falco is not in the local dump, so its vanilla `blaster_bullet` is not listed
- `vendor/hdr/fighters/falco/src/acmd/specials.rs:39` falco/game_specialnloop `*FIGHTER_FALCO_GENERATE_ARTICLE_BLASTER` (change_motion) — environment: falco is not in the local dump, so its vanilla `blaster` is not listed
- `vendor/hdr/fighters/falco/src/acmd/specials.rs:10` falco/game_specialnstart `*FIGHTER_FALCO_GENERATE_ARTICLE_BLASTER` (generate_article) — environment: falco is not in the local dump, so its vanilla `blaster` is not listed
- `vendor/hdr/fighters/falco/src/acmd/specials.rs:12` falco/game_specialnstart `*FIGHTER_FALCO_GENERATE_ARTICLE_BLASTER` (change_motion) — environment: falco is not in the local dump, so its vanilla `blaster` is not listed
- `vendor/hdr/fighters/falco/src/acmd/specials.rs:120` falco/game_specials `*FIGHTER_FALCO_GENERATE_ARTICLE_ILLUSION` (generate_article) — environment: falco is not in the local dump, so its vanilla `illusion` is not listed
- `vendor/hdr/fighters/falco/src/acmd/specials.rs:120` falco/game_specials `*FIGHTER_FALCO_GENERATE_ARTICLE_ILLUSION` (generate_article) — environment: falco is not in the local dump, so its vanilla `illusion` is not listed
- `vendor/hdr/fighters/falco/src/acmd/throws.rs:149` falco/game_throwb `*FIGHTER_FOX_GENERATE_ARTICLE_BLASTER` (generate_article) — another fighter's id (fox's blaster)
- `vendor/hdr/fighters/falco/src/acmd/throws.rs:153` falco/game_throwb `*FIGHTER_FOX_GENERATE_ARTICLE_BLASTER_BULLET` (generate_article) — another fighter's id (fox's blaster_bullet)
- `vendor/hdr/fighters/falco/src/acmd/throws.rs:157` falco/game_throwb `*FIGHTER_FOX_GENERATE_ARTICLE_BLASTER` (set_visibility_whole) — another fighter's id (fox's blaster)
- `vendor/hdr/fighters/falco/src/acmd/throws.rs:245` falco/game_throwhi `*FIGHTER_FOX_GENERATE_ARTICLE_BLASTER` (generate_article) — another fighter's id (fox's blaster)
- `vendor/hdr/fighters/falco/src/acmd/throws.rs:249` falco/game_throwhi `*FIGHTER_FOX_GENERATE_ARTICLE_BLASTER_BULLET` (generate_article) — another fighter's id (fox's blaster_bullet)
- `vendor/hdr/fighters/falco/src/acmd/throws.rs:253` falco/game_throwhi `*FIGHTER_FOX_GENERATE_ARTICLE_BLASTER` (set_visibility_whole) — another fighter's id (fox's blaster)
- `vendor/hdr/fighters/falco/src/acmd/throws.rs:315` falco/game_throwlw `*FIGHTER_FOX_GENERATE_ARTICLE_BLASTER` (generate_article) — another fighter's id (fox's blaster)
- `vendor/hdr/fighters/falco/src/acmd/throws.rs:319` falco/game_throwlw `*FIGHTER_FOX_GENERATE_ARTICLE_BLASTER_BULLET` (generate_article) — another fighter's id (fox's blaster_bullet)
- `vendor/hdr/fighters/falco/src/acmd/throws.rs:333` falco/game_throwlw `*FIGHTER_FOX_GENERATE_ARTICLE_BLASTER` (set_visibility_whole) — another fighter's id (fox's blaster)
- `vendor/hdr/fighters/fox/src/acmd/throws.rs:104` fox/game_throwb `*FIGHTER_FOX_GENERATE_ARTICLE_BLASTER` (change_motion) — environment: fox is not in the local dump, so its vanilla `blaster` is not listed
- `vendor/hdr/fighters/fox/src/acmd/throws.rs:109` fox/game_throwb `*FIGHTER_FOX_GENERATE_ARTICLE_BLASTER_BULLET` (generate_article) — environment: fox is not in the local dump, so its vanilla `blaster_bullet` is not listed
- `vendor/hdr/fighters/fox/src/acmd/throws.rs:109` fox/game_throwb `*FIGHTER_FOX_GENERATE_ARTICLE_BLASTER_BULLET` (generate_article) — environment: fox is not in the local dump, so its vanilla `blaster_bullet` is not listed
- `vendor/hdr/fighters/fox/src/acmd/throws.rs:109` fox/game_throwb `*FIGHTER_FOX_GENERATE_ARTICLE_BLASTER_BULLET` (generate_article) — environment: fox is not in the local dump, so its vanilla `blaster_bullet` is not listed
- `vendor/hdr/fighters/fox/src/acmd/throws.rs:122` fox/game_throwb `*FIGHTER_FOX_GENERATE_ARTICLE_BLASTER` (change_motion) — environment: fox is not in the local dump, so its vanilla `blaster` is not listed
- `vendor/hdr/fighters/fox/src/acmd/throws.rs:153` fox/game_throwhi `*FIGHTER_FOX_GENERATE_ARTICLE_BLASTER` (change_motion) — environment: fox is not in the local dump, so its vanilla `blaster` is not listed
- `vendor/hdr/fighters/fox/src/acmd/throws.rs:158` fox/game_throwhi `*FIGHTER_FOX_GENERATE_ARTICLE_BLASTER_BULLET` (generate_article) — environment: fox is not in the local dump, so its vanilla `blaster_bullet` is not listed
- `vendor/hdr/fighters/fox/src/acmd/throws.rs:158` fox/game_throwhi `*FIGHTER_FOX_GENERATE_ARTICLE_BLASTER_BULLET` (generate_article) — environment: fox is not in the local dump, so its vanilla `blaster_bullet` is not listed
- `vendor/hdr/fighters/fox/src/acmd/throws.rs:158` fox/game_throwhi `*FIGHTER_FOX_GENERATE_ARTICLE_BLASTER_BULLET` (generate_article) — environment: fox is not in the local dump, so its vanilla `blaster_bullet` is not listed
- `vendor/hdr/fighters/fox/src/acmd/throws.rs:172` fox/game_throwhi `*FIGHTER_FOX_GENERATE_ARTICLE_BLASTER` (change_motion) — environment: fox is not in the local dump, so its vanilla `blaster` is not listed
- `vendor/hdr/fighters/fox/src/acmd/specials.rs:17` fox/gamespecialairs `*FIGHTER_FOX_GENERATE_ARTICLE_ILLUSION` (generate_article) — environment: fox is not in the local dump, so its vanilla `illusion` is not listed
- `vendor/hdr/fighters/fox/src/acmd/specials.rs:17` fox/gamespecialairs `*FIGHTER_FOX_GENERATE_ARTICLE_ILLUSION` (generate_article) — environment: fox is not in the local dump, so its vanilla `illusion` is not listed
- `vendor/hdr/fighters/fox/src/acmd/specials.rs:17` fox/gamespecials `*FIGHTER_FOX_GENERATE_ARTICLE_ILLUSION` (generate_article) — environment: fox is not in the local dump, so its vanilla `illusion` is not listed
- `vendor/hdr/fighters/fox/src/acmd/specials.rs:17` fox/gamespecials `*FIGHTER_FOX_GENERATE_ARTICLE_ILLUSION` (generate_article) — environment: fox is not in the local dump, so its vanilla `illusion` is not listed
- `vendor/hdr/fighters/gamewatch/src/acmd/ground.rs:96` gamewatch/game_attack100end `*FIGHTER_GAMEWATCH_GENERATE_ARTICLE_NORMAL_WEAPON` (change_motion) — environment: gamewatch is not in the local dump, so its vanilla `normal_weapon` is not listed
- `vendor/hdr/fighters/gamewatch/src/acmd/ground.rs:9` gamewatch/game_attack11 `*FIGHTER_GAMEWATCH_GENERATE_ARTICLE_NORMAL_WEAPON` (generate_article) — environment: gamewatch is not in the local dump, so its vanilla `normal_weapon` is not listed
- `vendor/hdr/fighters/gamewatch/src/acmd/ground.rs:10` gamewatch/game_attack11 `*FIGHTER_GAMEWATCH_GENERATE_ARTICLE_NORMAL_WEAPON` (change_motion) — environment: gamewatch is not in the local dump, so its vanilla `normal_weapon` is not listed
- `vendor/hdr/fighters/gamewatch/src/acmd/ground.rs:39` gamewatch/game_attack12 `*FIGHTER_GAMEWATCH_GENERATE_ARTICLE_NORMAL_WEAPON` (change_motion) — environment: gamewatch is not in the local dump, so its vanilla `normal_weapon` is not listed
- `vendor/hdr/fighters/gamewatch/src/acmd/aerials.rs:143` gamewatch/game_attackairb `*FIGHTER_GAMEWATCH_GENERATE_ARTICLE_NORMAL_WEAPON` (generate_article) — environment: gamewatch is not in the local dump, so its vanilla `normal_weapon` is not listed
- `vendor/hdr/fighters/gamewatch/src/acmd/aerials.rs:144` gamewatch/game_attackairb `*FIGHTER_GAMEWATCH_GENERATE_ARTICLE_NORMAL_WEAPON` (change_motion) — environment: gamewatch is not in the local dump, so its vanilla `normal_weapon` is not listed
- `vendor/hdr/fighters/gamewatch/src/acmd/aerials.rs:143` gamewatch/game_attackairb `*FIGHTER_GAMEWATCH_GENERATE_ARTICLE_NORMAL_WEAPON` (generate_article) — environment: gamewatch is not in the local dump, so its vanilla `normal_weapon` is not listed
- `vendor/hdr/fighters/gamewatch/src/acmd/aerials.rs:143` gamewatch/game_attackairb `*FIGHTER_GAMEWATCH_GENERATE_ARTICLE_NORMAL_WEAPON` (generate_article) — environment: gamewatch is not in the local dump, so its vanilla `normal_weapon` is not listed
- `vendor/hdr/fighters/gamewatch/src/acmd/aerials.rs:143` gamewatch/game_attackairb `*FIGHTER_GAMEWATCH_GENERATE_ARTICLE_NORMAL_WEAPON` (generate_article) — environment: gamewatch is not in the local dump, so its vanilla `normal_weapon` is not listed
- `vendor/hdr/fighters/gamewatch/src/acmd/aerials.rs:189` gamewatch/game_attackairb `*FIGHTER_GAMEWATCH_GENERATE_ARTICLE_NORMAL_WEAPON` (remove_exist) — environment: gamewatch is not in the local dump, so its vanilla `normal_weapon` is not listed
- `vendor/hdr/fighters/gamewatch/src/acmd/aerials.rs:81` gamewatch/game_attackairf `*FIGHTER_GAMEWATCH_GENERATE_ARTICLE_NORMAL_WEAPON` (generate_article) — environment: gamewatch is not in the local dump, so its vanilla `normal_weapon` is not listed
- `vendor/hdr/fighters/gamewatch/src/acmd/aerials.rs:82` gamewatch/game_attackairf `*FIGHTER_GAMEWATCH_GENERATE_ARTICLE_NORMAL_WEAPON` (change_motion) — environment: gamewatch is not in the local dump, so its vanilla `normal_weapon` is not listed
- `vendor/hdr/fighters/gamewatch/src/acmd/aerials.rs:110` gamewatch/game_attackairf `*FIGHTER_GAMEWATCH_GENERATE_ARTICLE_NORMAL_WEAPON` (remove_exist) — environment: gamewatch is not in the local dump, so its vanilla `normal_weapon` is not listed
- `vendor/hdr/fighters/gamewatch/src/acmd/aerials.rs:212` gamewatch/game_attackairhi `*FIGHTER_GAMEWATCH_GENERATE_ARTICLE_BREATH` (generate_article) — environment: gamewatch is not in the local dump, so its vanilla `breath` is not listed
- `vendor/hdr/fighters/gamewatch/src/acmd/aerials.rs:220` gamewatch/game_attackairhi `*FIGHTER_GAMEWATCH_GENERATE_ARTICLE_BREATH` (remove_exist) — environment: gamewatch is not in the local dump, so its vanilla `breath` is not listed
- `vendor/hdr/fighters/gamewatch/src/acmd/aerials.rs:8` gamewatch/game_attackairn `*FIGHTER_GAMEWATCH_GENERATE_ARTICLE_NORMAL_WEAPON` (generate_article) — environment: gamewatch is not in the local dump, so its vanilla `normal_weapon` is not listed
- `vendor/hdr/fighters/gamewatch/src/acmd/aerials.rs:9` gamewatch/game_attackairn `*FIGHTER_GAMEWATCH_GENERATE_ARTICLE_NORMAL_WEAPON` (change_motion) — environment: gamewatch is not in the local dump, so its vanilla `normal_weapon` is not listed
- `vendor/hdr/fighters/gamewatch/src/acmd/aerials.rs:8` gamewatch/game_attackairn `*FIGHTER_GAMEWATCH_GENERATE_ARTICLE_NORMAL_WEAPON` (generate_article) — environment: gamewatch is not in the local dump, so its vanilla `normal_weapon` is not listed
- `vendor/hdr/fighters/gamewatch/src/acmd/aerials.rs:8` gamewatch/game_attackairn `*FIGHTER_GAMEWATCH_GENERATE_ARTICLE_NORMAL_WEAPON` (generate_article) — environment: gamewatch is not in the local dump, so its vanilla `normal_weapon` is not listed
- `vendor/hdr/fighters/gamewatch/src/acmd/tilts.rs:80` gamewatch/game_attacklw3 `*FIGHTER_GAMEWATCH_GENERATE_ARTICLE_NORMAL_WEAPON` (generate_article) — environment: gamewatch is not in the local dump, so its vanilla `normal_weapon` is not listed
- `vendor/hdr/fighters/gamewatch/src/acmd/tilts.rs:81` gamewatch/game_attacklw3 `*FIGHTER_GAMEWATCH_GENERATE_ARTICLE_NORMAL_WEAPON` (change_motion) — environment: gamewatch is not in the local dump, so its vanilla `normal_weapon` is not listed
- `vendor/hdr/fighters/gamewatch/src/acmd/tilts.rs:85` gamewatch/game_attacklw3 `*FIGHTER_GAMEWATCH_GENERATE_ARTICLE_NORMAL_WEAPON` (set_rate) — environment: gamewatch is not in the local dump, so its vanilla `normal_weapon` is not listed
- `vendor/hdr/fighters/gamewatch/src/acmd/tilts.rs:92` gamewatch/game_attacklw3 `*FIGHTER_GAMEWATCH_GENERATE_ARTICLE_NORMAL_WEAPON` (set_rate) — environment: gamewatch is not in the local dump, so its vanilla `normal_weapon` is not listed
- `vendor/hdr/fighters/gamewatch/src/acmd/tilts.rs:99` gamewatch/game_attacklw3 `*FIGHTER_GAMEWATCH_GENERATE_ARTICLE_NORMAL_WEAPON` (set_rate) — environment: gamewatch is not in the local dump, so its vanilla `normal_weapon` is not listed
- `vendor/hdr/fighters/gamewatch/src/acmd/tilts.rs:105` gamewatch/game_attacklw3 `*FIGHTER_GAMEWATCH_GENERATE_ARTICLE_NORMAL_WEAPON` (remove) — environment: gamewatch is not in the local dump, so its vanilla `normal_weapon` is not listed
- `vendor/hdr/fighters/gamewatch/src/acmd/tilts.rs:9` gamewatch/game_attacks3 `*FIGHTER_GAMEWATCH_GENERATE_ARTICLE_NORMAL_WEAPON` (generate_article) — environment: gamewatch is not in the local dump, so its vanilla `normal_weapon` is not listed
- `vendor/hdr/fighters/gamewatch/src/acmd/tilts.rs:10` gamewatch/game_attacks3 `*FIGHTER_GAMEWATCH_GENERATE_ARTICLE_NORMAL_WEAPON` (change_motion) — environment: gamewatch is not in the local dump, so its vanilla `normal_weapon` is not listed
- `vendor/hdr/fighters/gamewatch/src/acmd/tilts.rs:25` gamewatch/game_attacks3 `*FIGHTER_GAMEWATCH_GENERATE_ARTICLE_NORMAL_WEAPON` (remove) — environment: gamewatch is not in the local dump, so its vanilla `normal_weapon` is not listed
- `vendor/hdr/fighters/gamewatch/src/acmd/aerials.rs:197` gamewatch/game_landingairb `*FIGHTER_GAMEWATCH_GENERATE_ARTICLE_NORMAL_WEAPON` (change_motion) — environment: gamewatch is not in the local dump, so its vanilla `normal_weapon` is not listed
- `vendor/hdr/fighters/gamewatch/src/acmd/aerials.rs:118` gamewatch/game_landingairf `*FIGHTER_GAMEWATCH_GENERATE_ARTICLE_NORMAL_WEAPON` (remove_exist) — environment: gamewatch is not in the local dump, so its vanilla `normal_weapon` is not listed
- `vendor/hdr/fighters/gamewatch/src/acmd/specials/mod.rs:128` gamewatch/game_specialhiopen `*FIGHTER_GAMEWATCH_GENERATE_ARTICLE_PARACHUTE` (generate_article) — environment: gamewatch is not in the local dump, so its vanilla `parachute` is not listed
- `vendor/hdr/fighters/gamewatch/src/acmd/specials/mod.rs:129` gamewatch/game_specialhiopen `*FIGHTER_GAMEWATCH_GENERATE_ARTICLE_PARACHUTE` (change_motion) — environment: gamewatch is not in the local dump, so its vanilla `parachute` is not listed
- `vendor/hdr/fighters/gamewatch/src/acmd/specials/mod.rs:142` gamewatch/game_specialhiopen `*FIGHTER_GAMEWATCH_GENERATE_ARTICLE_PARACHUTE` (remove_exist) — environment: gamewatch is not in the local dump, so its vanilla `parachute` is not listed
- `vendor/hdr/fighters/ganon/src/acmd/tilts.rs:119` ganon/game_attackhi3 `*FIGHTER_GANON_GENERATE_ARTICLE_SWORD` (remove_exist) — environment: ganon is not in the local dump, so its vanilla `sword` is not listed
- `vendor/hdr/fighters/ganon/src/acmd/tilts.rs:120` ganon/game_attackhi3 `*FIGHTER_GANON_GENERATE_ARTICLE_SWORD` (generate_article) — environment: ganon is not in the local dump, so its vanilla `sword` is not listed
- `vendor/hdr/fighters/ganon/src/acmd/tilts.rs:119` ganon/game_attackhi3 `*FIGHTER_GANON_GENERATE_ARTICLE_SWORD` (remove_exist) — environment: ganon is not in the local dump, so its vanilla `sword` is not listed
- `vendor/hdr/fighters/ganon/src/acmd/smashes.rs:70` ganon/game_attackhi4 `*FIGHTER_GANON_GENERATE_ARTICLE_SWORD` (remove_exist) — environment: ganon is not in the local dump, so its vanilla `sword` is not listed
- `vendor/hdr/fighters/ganon/src/acmd/smashes.rs:71` ganon/game_attackhi4 `*FIGHTER_GANON_GENERATE_ARTICLE_SWORD` (generate_article) — environment: ganon is not in the local dump, so its vanilla `sword` is not listed
- `vendor/hdr/fighters/ganon/src/acmd/smashes.rs:71` ganon/game_attackhi4 `*FIGHTER_GANON_GENERATE_ARTICLE_SWORD` (generate_article) — environment: ganon is not in the local dump, so its vanilla `sword` is not listed
- `vendor/hdr/fighters/ganon/src/acmd/smashes.rs:71` ganon/game_attackhi4 `*FIGHTER_GANON_GENERATE_ARTICLE_SWORD` (generate_article) — environment: ganon is not in the local dump, so its vanilla `sword` is not listed
- `vendor/hdr/fighters/ganon/src/acmd/smashes.rs:134` ganon/game_attacklw4 `*FIGHTER_GANON_GENERATE_ARTICLE_SWORD` (remove_exist) — environment: ganon is not in the local dump, so its vanilla `sword` is not listed
- `vendor/hdr/fighters/ganon/src/acmd/smashes.rs:135` ganon/game_attacklw4 `*FIGHTER_GANON_GENERATE_ARTICLE_SWORD` (generate_article) — environment: ganon is not in the local dump, so its vanilla `sword` is not listed
- `vendor/hdr/fighters/ganon/src/acmd/smashes.rs:135` ganon/game_attacklw4 `*FIGHTER_GANON_GENERATE_ARTICLE_SWORD` (generate_article) — environment: ganon is not in the local dump, so its vanilla `sword` is not listed
- `vendor/hdr/fighters/ganon/src/acmd/smashes.rs:135` ganon/game_attacklw4 `*FIGHTER_GANON_GENERATE_ARTICLE_SWORD` (generate_article) — environment: ganon is not in the local dump, so its vanilla `sword` is not listed
- `vendor/hdr/fighters/ganon/src/acmd/smashes.rs:7` ganon/game_attacks4 `*FIGHTER_GANON_GENERATE_ARTICLE_SWORD` (remove_exist) — environment: ganon is not in the local dump, so its vanilla `sword` is not listed
- `vendor/hdr/fighters/ganon/src/acmd/smashes.rs:8` ganon/game_attacks4 `*FIGHTER_GANON_GENERATE_ARTICLE_SWORD` (generate_article) — environment: ganon is not in the local dump, so its vanilla `sword` is not listed
- `vendor/hdr/fighters/ganon/src/acmd/smashes.rs:8` ganon/game_attacks4 `*FIGHTER_GANON_GENERATE_ARTICLE_SWORD` (generate_article) — environment: ganon is not in the local dump, so its vanilla `sword` is not listed
- `vendor/hdr/fighters/ganon/src/acmd/smashes.rs:8` ganon/game_attacks4 `*FIGHTER_GANON_GENERATE_ARTICLE_SWORD` (generate_article) — environment: ganon is not in the local dump, so its vanilla `sword` is not listed
- `vendor/hdr/fighters/ike/src/acmd/other.rs:160` ike/game_appeallwl `*FIGHTER_IKE_GENERATE_ARTICLE_SWORD` (generate_article) — environment: ike is not in the local dump, so its vanilla `sword` is not listed
- `vendor/hdr/fighters/ike/src/acmd/other.rs:161` ike/game_appeallwl `*FIGHTER_IKE_GENERATE_ARTICLE_SWORD` (change_motion) — environment: ike is not in the local dump, so its vanilla `sword` is not listed
- `vendor/hdr/fighters/ike/src/acmd/other.rs:160` ike/game_appeallwr `*FIGHTER_IKE_GENERATE_ARTICLE_SWORD` (generate_article) — environment: ike is not in the local dump, so its vanilla `sword` is not listed
- `vendor/hdr/fighters/ike/src/acmd/other.rs:161` ike/game_appeallwr `*FIGHTER_IKE_GENERATE_ARTICLE_SWORD` (change_motion) — environment: ike is not in the local dump, so its vanilla `sword` is not listed
- `vendor/hdr/fighters/ike/src/acmd/specials.rs:452` ike/game_specialairhi2 `*FIGHTER_IKE_GENERATE_ARTICLE_SWORD` (generate_article) — environment: ike is not in the local dump, so its vanilla `sword` is not listed
- `vendor/hdr/fighters/ike/src/acmd/specials.rs:489` ike/game_specialairhi2 `*FIGHTER_IKE_GENERATE_ARTICLE_SWORD` (remove_exist) — environment: ike is not in the local dump, so its vanilla `sword` is not listed
- `vendor/hdr/fighters/ike/src/acmd/specials.rs:389` ike/game_specialhi2 `*FIGHTER_IKE_GENERATE_ARTICLE_SWORD` (generate_article) — environment: ike is not in the local dump, so its vanilla `sword` is not listed
- `vendor/hdr/fighters/ike/src/acmd/specials.rs:424` ike/game_specialhi2 `*FIGHTER_IKE_GENERATE_ARTICLE_SWORD` (remove_exist) — environment: ike is not in the local dump, so its vanilla `sword` is not listed
- `vendor/hdr/fighters/inkling/src/acmd/ground.rs:92` inkling/game_attackdash `*FIGHTER_INKLING_GENERATE_ARTICLE_SQUID` (generate_article) — environment: inkling is not in the local dump, so its vanilla `squid` is not listed
- `vendor/hdr/fighters/inkling/src/acmd/ground.rs:94` inkling/game_attackdash `*FIGHTER_INKLING_GENERATE_ARTICLE_SQUID` (set_visibility_whole) — environment: inkling is not in the local dump, so its vanilla `squid` is not listed
- `vendor/hdr/fighters/inkling/src/acmd/ground.rs:101` inkling/game_attackdash `*FIGHTER_INKLING_GENERATE_ARTICLE_SQUID` (change_motion) — environment: inkling is not in the local dump, so its vanilla `squid` is not listed
- `vendor/hdr/fighters/inkling/src/acmd/ground.rs:104` inkling/game_attackdash `*FIGHTER_INKLING_GENERATE_ARTICLE_SQUID` (change_motion) — environment: inkling is not in the local dump, so its vanilla `squid` is not listed
- `vendor/hdr/fighters/inkling/src/acmd/ground.rs:110` inkling/game_attackdash `*FIGHTER_INKLING_GENERATE_ARTICLE_SQUID` (set_visibility_whole) — environment: inkling is not in the local dump, so its vanilla `squid` is not listed
- `vendor/hdr/fighters/inkling/src/acmd/ground.rs:94` inkling/game_attackdash `*FIGHTER_INKLING_GENERATE_ARTICLE_SQUID` (set_visibility_whole) — environment: inkling is not in the local dump, so its vanilla `squid` is not listed
- `vendor/hdr/fighters/inkling/src/acmd/smashes.rs:49` inkling/game_attackhi4 `*FIGHTER_INKLING_GENERATE_ARTICLE_BLASTER` (generate_article) — environment: inkling is not in the local dump, so its vanilla `blaster` is not listed
- `vendor/hdr/fighters/inkling/src/acmd/smashes.rs:50` inkling/game_attackhi4 `*FIGHTER_INKLING_GENERATE_ARTICLE_BLASTER` (change_motion) — environment: inkling is not in the local dump, so its vanilla `blaster` is not listed
- `vendor/hdr/fighters/inkling/src/acmd/smashes.rs:56` inkling/game_attackhi4 `*FIGHTER_INKLING_GENERATE_ARTICLE_BLASTER` (change_motion) — environment: inkling is not in the local dump, so its vanilla `blaster` is not listed
- `vendor/hdr/fighters/inkling/src/acmd/smashes.rs:91` inkling/game_attackhi4charge `*FIGHTER_INKLING_GENERATE_ARTICLE_BLASTER` (change_motion) — environment: inkling is not in the local dump, so its vanilla `blaster` is not listed
- `vendor/hdr/fighters/inkling/src/acmd/smashes.rs:99` inkling/game_attacklw4 `*FIGHTER_INKLING_GENERATE_ARTICLE_SLOSHER` (generate_article) — environment: inkling is not in the local dump, so its vanilla `slosher` is not listed
- `vendor/hdr/fighters/inkling/src/acmd/smashes.rs:100` inkling/game_attacklw4 `*FIGHTER_INKLING_GENERATE_ARTICLE_SLOSHER` (change_motion) — environment: inkling is not in the local dump, so its vanilla `slosher` is not listed
- `vendor/hdr/fighters/inkling/src/acmd/smashes.rs:109` inkling/game_attacklw4 `*FIGHTER_INKLING_GENERATE_ARTICLE_SLOSHER` (change_motion) — environment: inkling is not in the local dump, so its vanilla `slosher` is not listed
- `vendor/hdr/fighters/inkling/src/acmd/smashes.rs:7` inkling/game_attacks4 `*FIGHTER_INKLING_GENERATE_ARTICLE_BRUSH` (generate_article) — environment: inkling is not in the local dump, so its vanilla `brush` is not listed
- `vendor/hdr/fighters/inkling/src/acmd/smashes.rs:8` inkling/game_attacks4 `*FIGHTER_INKLING_GENERATE_ARTICLE_BRUSH` (change_motion) — environment: inkling is not in the local dump, so its vanilla `brush` is not listed
- `vendor/hdr/fighters/inkling/src/acmd/smashes.rs:16` inkling/game_attacks4 `*FIGHTER_INKLING_GENERATE_ARTICLE_BRUSH` (change_motion) — environment: inkling is not in the local dump, so its vanilla `brush` is not listed
- `vendor/hdr/fighters/inkling/src/acmd/smashes.rs:41` inkling/game_attacks4charge `*FIGHTER_INKLING_GENERATE_ARTICLE_BRUSH` (change_motion) — environment: inkling is not in the local dump, so its vanilla `brush` is not listed
- `vendor/hdr/fighters/inkling/src/acmd/throws.rs:24` inkling/game_catchdash `*FIGHTER_INKLING_GENERATE_ARTICLE_SQUID` (generate_article) — environment: inkling is not in the local dump, so its vanilla `squid` is not listed
- `vendor/hdr/fighters/inkling/src/acmd/throws.rs:29` inkling/game_catchdash `*FIGHTER_INKLING_GENERATE_ARTICLE_SQUID` (change_motion) — environment: inkling is not in the local dump, so its vanilla `squid` is not listed
- `vendor/hdr/fighters/inkling/src/acmd/throws.rs:30` inkling/game_catchdash `*FIGHTER_INKLING_GENERATE_ARTICLE_SQUID` (set_frame) — environment: inkling is not in the local dump, so its vanilla `squid` is not listed
- `vendor/hdr/fighters/inkling/src/acmd/throws.rs:31` inkling/game_catchdash `*FIGHTER_INKLING_GENERATE_ARTICLE_SQUID` (set_rate) — environment: inkling is not in the local dump, so its vanilla `squid` is not listed
- `vendor/hdr/fighters/inkling/src/acmd/throws.rs:32` inkling/game_catchdash `*FIGHTER_INKLING_GENERATE_ARTICLE_SQUID` (set_visibility_whole) — environment: inkling is not in the local dump, so its vanilla `squid` is not listed
- `vendor/hdr/fighters/inkling/src/acmd/throws.rs:39` inkling/game_catchdash `*FIGHTER_INKLING_GENERATE_ARTICLE_SQUID` (change_motion) — environment: inkling is not in the local dump, so its vanilla `squid` is not listed
- `vendor/hdr/fighters/inkling/src/acmd/throws.rs:42` inkling/game_catchdash `*FIGHTER_INKLING_GENERATE_ARTICLE_SQUID` (change_motion) — environment: inkling is not in the local dump, so its vanilla `squid` is not listed
- `vendor/hdr/fighters/inkling/src/acmd/throws.rs:48` inkling/game_catchdash `*FIGHTER_INKLING_GENERATE_ARTICLE_SQUID` (set_visibility_whole) — environment: inkling is not in the local dump, so its vanilla `squid` is not listed
- `vendor/hdr/fighters/inkling/src/acmd/throws.rs:32` inkling/game_catchdash `*FIGHTER_INKLING_GENERATE_ARTICLE_SQUID` (set_visibility_whole) — environment: inkling is not in the local dump, so its vanilla `squid` is not listed
- `vendor/hdr/fighters/inkling/src/acmd/throws.rs:72` inkling/game_catchturn `*FIGHTER_INKLING_GENERATE_ARTICLE_SQUID` (generate_article) — environment: inkling is not in the local dump, so its vanilla `squid` is not listed
- `vendor/hdr/fighters/inkling/src/acmd/throws.rs:77` inkling/game_catchturn `*FIGHTER_INKLING_GENERATE_ARTICLE_SQUID` (change_motion) — environment: inkling is not in the local dump, so its vanilla `squid` is not listed
- `vendor/hdr/fighters/inkling/src/acmd/throws.rs:78` inkling/game_catchturn `*FIGHTER_INKLING_GENERATE_ARTICLE_SQUID` (set_frame) — environment: inkling is not in the local dump, so its vanilla `squid` is not listed
- `vendor/hdr/fighters/inkling/src/acmd/throws.rs:79` inkling/game_catchturn `*FIGHTER_INKLING_GENERATE_ARTICLE_SQUID` (set_rate) — environment: inkling is not in the local dump, so its vanilla `squid` is not listed
- `vendor/hdr/fighters/inkling/src/acmd/throws.rs:80` inkling/game_catchturn `*FIGHTER_INKLING_GENERATE_ARTICLE_SQUID` (set_visibility_whole) — environment: inkling is not in the local dump, so its vanilla `squid` is not listed
- `vendor/hdr/fighters/inkling/src/acmd/throws.rs:87` inkling/game_catchturn `*FIGHTER_INKLING_GENERATE_ARTICLE_SQUID` (change_motion) — environment: inkling is not in the local dump, so its vanilla `squid` is not listed
- `vendor/hdr/fighters/inkling/src/acmd/throws.rs:90` inkling/game_catchturn `*FIGHTER_INKLING_GENERATE_ARTICLE_SQUID` (change_motion) — environment: inkling is not in the local dump, so its vanilla `squid` is not listed
- `vendor/hdr/fighters/inkling/src/acmd/throws.rs:96` inkling/game_catchturn `*FIGHTER_INKLING_GENERATE_ARTICLE_SQUID` (set_visibility_whole) — environment: inkling is not in the local dump, so its vanilla `squid` is not listed
- `vendor/hdr/fighters/inkling/src/acmd/throws.rs:80` inkling/game_catchturn `*FIGHTER_INKLING_GENERATE_ARTICLE_SQUID` (set_visibility_whole) — environment: inkling is not in the local dump, so its vanilla `squid` is not listed
- `vendor/hdr/fighters/inkling/src/acmd/other.rs:59` inkling/game_escapeair `*FIGHTER_INKLING_GENERATE_ARTICLE_SQUID` (generate_article) — environment: inkling is not in the local dump, so its vanilla `squid` is not listed
- `vendor/hdr/fighters/inkling/src/acmd/other.rs:65` inkling/game_escapeair `*FIGHTER_INKLING_GENERATE_ARTICLE_SQUID` (change_motion) — environment: inkling is not in the local dump, so its vanilla `squid` is not listed
- `vendor/hdr/fighters/inkling/src/acmd/other.rs:67` inkling/game_escapeair `*FIGHTER_INKLING_GENERATE_ARTICLE_SQUID` (set_frame) — environment: inkling is not in the local dump, so its vanilla `squid` is not listed
- `vendor/hdr/fighters/inkling/src/acmd/other.rs:68` inkling/game_escapeair `*FIGHTER_INKLING_GENERATE_ARTICLE_SQUID` (set_rate) — environment: inkling is not in the local dump, so its vanilla `squid` is not listed
- `vendor/hdr/fighters/inkling/src/acmd/other.rs:69` inkling/game_escapeair `*FIGHTER_INKLING_GENERATE_ARTICLE_SQUID` (set_visibility_whole) — environment: inkling is not in the local dump, so its vanilla `squid` is not listed
- `vendor/hdr/fighters/inkling/src/acmd/other.rs:78` inkling/game_escapeair `*FIGHTER_INKLING_GENERATE_ARTICLE_SQUID` (set_visibility_whole) — environment: inkling is not in the local dump, so its vanilla `squid` is not listed
- `vendor/hdr/fighters/inkling/src/acmd/other.rs:94` inkling/game_escapeair `*FIGHTER_INKLING_GENERATE_ARTICLE_SQUID` (set_visibility_whole) — environment: inkling is not in the local dump, so its vanilla `squid` is not listed
- `vendor/hdr/fighters/inkling/src/acmd/other.rs:107` inkling/game_escapeairslide `*FIGHTER_INKLING_GENERATE_ARTICLE_SQUID` (generate_article) — environment: inkling is not in the local dump, so its vanilla `squid` is not listed
- `vendor/hdr/fighters/inkling/src/acmd/other.rs:113` inkling/game_escapeairslide `*FIGHTER_INKLING_GENERATE_ARTICLE_SQUID` (change_motion) — environment: inkling is not in the local dump, so its vanilla `squid` is not listed
- `vendor/hdr/fighters/inkling/src/acmd/other.rs:115` inkling/game_escapeairslide `*FIGHTER_INKLING_GENERATE_ARTICLE_SQUID` (set_frame) — environment: inkling is not in the local dump, so its vanilla `squid` is not listed
- `vendor/hdr/fighters/inkling/src/acmd/other.rs:116` inkling/game_escapeairslide `*FIGHTER_INKLING_GENERATE_ARTICLE_SQUID` (set_rate) — environment: inkling is not in the local dump, so its vanilla `squid` is not listed
- `vendor/hdr/fighters/inkling/src/acmd/other.rs:117` inkling/game_escapeairslide `*FIGHTER_INKLING_GENERATE_ARTICLE_SQUID` (set_visibility_whole) — environment: inkling is not in the local dump, so its vanilla `squid` is not listed
- `vendor/hdr/fighters/inkling/src/acmd/other.rs:126` inkling/game_escapeairslide `*FIGHTER_INKLING_GENERATE_ARTICLE_SQUID` (set_visibility_whole) — environment: inkling is not in the local dump, so its vanilla `squid` is not listed
- `vendor/hdr/fighters/inkling/src/acmd/other.rs:143` inkling/game_escapeairslide `*FIGHTER_INKLING_GENERATE_ARTICLE_SQUID` (set_visibility_whole) — environment: inkling is not in the local dump, so its vanilla `squid` is not listed
- `vendor/hdr/fighters/inkling/src/acmd/specials.rs:70` inkling/game_specialairhistart `*FIGHTER_INKLING_GENERATE_ARTICLE_SQUID` (set_visibility_whole) — environment: inkling is not in the local dump, so its vanilla `squid` is not listed
- `vendor/hdr/fighters/inkling/src/acmd/specials.rs:70` inkling/game_specialhistart `*FIGHTER_INKLING_GENERATE_ARTICLE_SQUID` (set_visibility_whole) — environment: inkling is not in the local dump, so its vanilla `squid` is not listed
- `vendor/hdr/fighters/inkling/src/acmd/throws.rs:147` inkling/game_throwb `*FIGHTER_INKLING_GENERATE_ARTICLE_SQUID` (generate_article) — environment: inkling is not in the local dump, so its vanilla `squid` is not listed
- `vendor/hdr/fighters/inkling/src/acmd/throws.rs:151` inkling/game_throwb `*FIGHTER_INKLING_GENERATE_ARTICLE_SQUID` (change_motion) — environment: inkling is not in the local dump, so its vanilla `squid` is not listed
- `vendor/hdr/fighters/inkling/src/acmd/throws.rs:152` inkling/game_throwb `*FIGHTER_INKLING_GENERATE_ARTICLE_SQUID` (set_frame) — environment: inkling is not in the local dump, so its vanilla `squid` is not listed
- `vendor/hdr/fighters/inkling/src/acmd/throws.rs:153` inkling/game_throwb `*FIGHTER_INKLING_GENERATE_ARTICLE_SQUID` (set_rate) — environment: inkling is not in the local dump, so its vanilla `squid` is not listed
- `vendor/hdr/fighters/inkling/src/acmd/throws.rs:154` inkling/game_throwb `*FIGHTER_INKLING_GENERATE_ARTICLE_SQUID` (set_visibility_whole) — environment: inkling is not in the local dump, so its vanilla `squid` is not listed
- `vendor/hdr/fighters/inkling/src/acmd/throws.rs:162` inkling/game_throwb `*FIGHTER_INKLING_GENERATE_ARTICLE_SQUID` (change_motion) — environment: inkling is not in the local dump, so its vanilla `squid` is not listed
- `vendor/hdr/fighters/inkling/src/acmd/throws.rs:165` inkling/game_throwb `*FIGHTER_INKLING_GENERATE_ARTICLE_SQUID` (change_motion) — environment: inkling is not in the local dump, so its vanilla `squid` is not listed
- `vendor/hdr/fighters/inkling/src/acmd/throws.rs:176` inkling/game_throwb `*FIGHTER_INKLING_GENERATE_ARTICLE_SQUID` (set_visibility_whole) — environment: inkling is not in the local dump, so its vanilla `squid` is not listed
- `vendor/hdr/fighters/inkling/src/acmd/throws.rs:154` inkling/game_throwb `*FIGHTER_INKLING_GENERATE_ARTICLE_SQUID` (set_visibility_whole) — environment: inkling is not in the local dump, so its vanilla `squid` is not listed
- `vendor/hdr/fighters/jack/src/acmd/specials/special_hi.rs:57` jack/game_specialairhi `*FIGHTER_JACK_GENERATE_ARTICLE_WIREROPE` (generate_article) — jack has no `wirerope` folder in the dump and no clone binds this name
- `vendor/hdr/fighters/jack/src/acmd/specials/special_hi.rs:79` jack/game_specialairhi `*FIGHTER_JACK_GENERATE_ARTICLE_WIREROPE` (change_status) — jack has no `wirerope` folder in the dump and no clone binds this name
- `vendor/hdr/fighters/jack/src/acmd/specials/special_hi.rs:118` jack/game_specialairhi `*FIGHTER_JACK_GENERATE_ARTICLE_WIREROPE` (change_status) — jack has no `wirerope` folder in the dump and no clone binds this name
- `vendor/hdr/fighters/jack/src/acmd/specials/special_hi.rs:193` jack/game_specialairhib `*FIGHTER_JACK_GENERATE_ARTICLE_WING` (change_motion) — jack has no `wing` folder in the dump and no clone binds this name
- `vendor/hdr/fighters/jack/src/acmd/specials/special_hi.rs:193` jack/game_specialairhif `*FIGHTER_JACK_GENERATE_ARTICLE_WING` (change_motion) — jack has no `wing` folder in the dump and no clone binds this name
- `vendor/hdr/fighters/jack/src/acmd/specials/special_hi.rs:7` jack/game_specialhi `*FIGHTER_JACK_GENERATE_ARTICLE_WIREROPE` (generate_article) — jack has no `wirerope` folder in the dump and no clone binds this name
- `vendor/hdr/fighters/jack/src/acmd/specials/special_hi.rs:8` jack/game_specialhi `*FIGHTER_JACK_GENERATE_ARTICLE_WIREROPE` (change_motion) — jack has no `wirerope` folder in the dump and no clone binds this name
- `vendor/hdr/fighters/jack/src/acmd/specials/special_hi.rs:16` jack/game_specialhi `*FIGHTER_JACK_GENERATE_ARTICLE_WIREROPE` (change_status) — jack has no `wirerope` folder in the dump and no clone binds this name
- `vendor/hdr/fighters/jack/src/acmd/specials/special_hi.rs:46` jack/game_specialhi `*FIGHTER_JACK_GENERATE_ARTICLE_WIREROPE` (change_status) — jack has no `wirerope` folder in the dump and no clone binds this name
- `vendor/hdr/fighters/jack/src/acmd/specials/special_hi.rs:162` jack/game_specialhithrow `*FIGHTER_JACK_GENERATE_ARTICLE_WIREROPE` (generate_article) — jack has no `wirerope` folder in the dump and no clone binds this name
- `vendor/hdr/fighters/jack/src/acmd/specials/special_hi.rs:163` jack/game_specialhithrow `*FIGHTER_JACK_GENERATE_ARTICLE_WIREROPE` (change_motion) — jack has no `wirerope` folder in the dump and no clone binds this name
- `vendor/hdr/fighters/jack/src/acmd/specials/special_hi.rs:179` jack/game_specialhithrow `*FIGHTER_JACK_GENERATE_ARTICLE_WIREROPE` (remove_exist) — jack has no `wirerope` folder in the dump and no clone binds this name
- `vendor/hdr/fighters/kamui/src/acmd/specials.rs:294` kamui/game_specialswallend `0` (change_motion) — a number, which depends on the build
- `vendor/hdr/fighters/ken/src/acmd/specials/special_n.rs:76` ken/game_specialairn `*FIGHTER_KEN_GENERATE_ARTICLE_HADOKEN` (generate_article) — environment: ken is not in the local dump, so its vanilla `hadoken` is not listed
- `vendor/hdr/fighters/ken/src/acmd/specials/special_n.rs:26` ken/game_specialn `*FIGHTER_KEN_GENERATE_ARTICLE_HADOKEN` (generate_article) — environment: ken is not in the local dump, so its vanilla `hadoken` is not listed
- `vendor/hdr/fighters/kirby/src/acmd/copy/diddy.rs:37` kirby/game_diddyspecialairncancel `*FIGHTER_DIDDY_GENERATE_ARTICLE_GUN` (generate_article) — another fighter's id (diddy's gun)
- `vendor/hdr/fighters/kirby/src/acmd/copy/diddy.rs:42` kirby/game_diddyspecialairncancel `*FIGHTER_DIDDY_GENERATE_ARTICLE_GUN` (set_visibility_whole) — another fighter's id (diddy's gun)
- `vendor/hdr/fighters/kirby/src/acmd/copy/diddy.rs:8` kirby/game_diddyspecialncancel `*FIGHTER_DIDDY_GENERATE_ARTICLE_GUN` (generate_article) — another fighter's id (diddy's gun)
- `vendor/hdr/fighters/kirby/src/acmd/copy/diddy.rs:13` kirby/game_diddyspecialncancel `*FIGHTER_DIDDY_GENERATE_ARTICLE_GUN` (set_visibility_whole) — another fighter's id (diddy's gun)
- `vendor/hdr/fighters/kirby/src/acmd/copy/edge.rs:30` kirby/game_edgespecialairnstart `*FIGHTER_EDGE_GENERATE_ARTICLE_FIRE` (generate_article) — another fighter's id (edge's fire)
- `vendor/hdr/fighters/kirby/src/acmd/copy/edge.rs:30` kirby/game_edgespecialnstart `*FIGHTER_EDGE_GENERATE_ARTICLE_FIRE` (generate_article) — another fighter's id (edge's fire)
- `vendor/hdr/fighters/kirby/src/acmd/copy/krool.rs:8` kirby/game_kroolspecialairnfire `*FIGHTER_KROOL_GENERATE_ARTICLE_BLUNDERBUSS` (set_rate) — another fighter's id (krool's blunderbuss)
- `vendor/hdr/fighters/kirby/src/acmd/copy/krool.rs:14` kirby/game_kroolspecialairnfire `*FIGHTER_KROOL_GENERATE_ARTICLE_BLUNDERBUSS` (set_rate) — another fighter's id (krool's blunderbuss)
- `vendor/hdr/fighters/kirby/src/acmd/copy/krool.rs:27` kirby/game_kroolspecialairnfire `*FIGHTER_KROOL_GENERATE_ARTICLE_BLUNDERBUSS` (set_rate) — another fighter's id (krool's blunderbuss)
- `vendor/hdr/fighters/kirby/src/acmd/copy/krool.rs:31` kirby/game_kroolspecialairnfire `*FIGHTER_KROOL_GENERATE_ARTICLE_BLUNDERBUSS` (set_rate) — another fighter's id (krool's blunderbuss)
- `vendor/hdr/fighters/kirby/src/acmd/copy/krool.rs:14` kirby/game_kroolspecialairnfire `*FIGHTER_KROOL_GENERATE_ARTICLE_BLUNDERBUSS` (set_rate) — another fighter's id (krool's blunderbuss)
- `vendor/hdr/fighters/kirby/src/acmd/copy/krool.rs:44` kirby/game_kroolspecialairnfire `*FIGHTER_KROOL_GENERATE_ARTICLE_BLUNDERBUSS` (set_rate) — another fighter's id (krool's blunderbuss)
- `vendor/hdr/fighters/kirby/src/acmd/copy/krool.rs:14` kirby/game_kroolspecialairnfire `*FIGHTER_KROOL_GENERATE_ARTICLE_BLUNDERBUSS` (set_rate) — another fighter's id (krool's blunderbuss)
- `vendor/hdr/fighters/kirby/src/acmd/copy/krool.rs:220` kirby/game_kroolspecialairnfireb `*FIGHTER_KROOL_GENERATE_ARTICLE_BLUNDERBUSS` (set_rate) — another fighter's id (krool's blunderbuss)
- `vendor/hdr/fighters/kirby/src/acmd/copy/krool.rs:226` kirby/game_kroolspecialairnfireb `*FIGHTER_KROOL_GENERATE_ARTICLE_BLUNDERBUSS` (set_rate) — another fighter's id (krool's blunderbuss)
- `vendor/hdr/fighters/kirby/src/acmd/copy/krool.rs:8` kirby/game_kroolspecialnfire `*FIGHTER_KROOL_GENERATE_ARTICLE_BLUNDERBUSS` (set_rate) — another fighter's id (krool's blunderbuss)
- `vendor/hdr/fighters/kirby/src/acmd/copy/krool.rs:14` kirby/game_kroolspecialnfire `*FIGHTER_KROOL_GENERATE_ARTICLE_BLUNDERBUSS` (set_rate) — another fighter's id (krool's blunderbuss)
- `vendor/hdr/fighters/kirby/src/acmd/copy/krool.rs:27` kirby/game_kroolspecialnfire `*FIGHTER_KROOL_GENERATE_ARTICLE_BLUNDERBUSS` (set_rate) — another fighter's id (krool's blunderbuss)
- `vendor/hdr/fighters/kirby/src/acmd/copy/krool.rs:31` kirby/game_kroolspecialnfire `*FIGHTER_KROOL_GENERATE_ARTICLE_BLUNDERBUSS` (set_rate) — another fighter's id (krool's blunderbuss)
- `vendor/hdr/fighters/kirby/src/acmd/copy/krool.rs:14` kirby/game_kroolspecialnfire `*FIGHTER_KROOL_GENERATE_ARTICLE_BLUNDERBUSS` (set_rate) — another fighter's id (krool's blunderbuss)
- `vendor/hdr/fighters/kirby/src/acmd/copy/krool.rs:44` kirby/game_kroolspecialnfire `*FIGHTER_KROOL_GENERATE_ARTICLE_BLUNDERBUSS` (set_rate) — another fighter's id (krool's blunderbuss)
- `vendor/hdr/fighters/kirby/src/acmd/copy/krool.rs:14` kirby/game_kroolspecialnfire `*FIGHTER_KROOL_GENERATE_ARTICLE_BLUNDERBUSS` (set_rate) — another fighter's id (krool's blunderbuss)
- `vendor/hdr/fighters/kirby/src/acmd/copy/krool.rs:220` kirby/game_kroolspecialnfireb `*FIGHTER_KROOL_GENERATE_ARTICLE_BLUNDERBUSS` (set_rate) — another fighter's id (krool's blunderbuss)
- `vendor/hdr/fighters/kirby/src/acmd/copy/krool.rs:226` kirby/game_kroolspecialnfireb `*FIGHTER_KROOL_GENERATE_ARTICLE_BLUNDERBUSS` (set_rate) — another fighter's id (krool's blunderbuss)
- `vendor/hdr/fighters/kirby/src/acmd/copy/lucario.rs:8` kirby/game_lucariospecialairnstart `*FIGHTER_LUCARIO_GENERATE_ARTICLE_AURABALL` (generate_article) — another fighter's id (lucario's auraball)
- `vendor/hdr/fighters/kirby/src/acmd/copy/lucario.rs:8` kirby/game_lucariospecialnstart `*FIGHTER_LUCARIO_GENERATE_ARTICLE_AURABALL` (generate_article) — another fighter's id (lucario's auraball)
- `vendor/hdr/fighters/kirby/src/acmd/copy/luigi.rs:8` kirby/game_luigispecialairn `*FIGHTER_LUIGI_GENERATE_ARTICLE_FIREBALL` (generate_article) — another fighter's id (luigi's fireball)
- `vendor/hdr/fighters/kirby/src/acmd/copy/luigi.rs:8` kirby/game_luigispecialn `*FIGHTER_LUIGI_GENERATE_ARTICLE_FIREBALL` (generate_article) — another fighter's id (luigi's fireball)
- `vendor/hdr/fighters/kirby/src/acmd/copy/mariod.rs:11` kirby/game_mariodspecialairn `*FIGHTER_MARIOD_GENERATE_ARTICLE_DRCAPSULE` (generate_article) — another fighter's id (mariod's drcapsule)
- `vendor/hdr/fighters/kirby/src/acmd/copy/mariod.rs:11` kirby/game_mariodspecialn `*FIGHTER_MARIOD_GENERATE_ARTICLE_DRCAPSULE` (generate_article) — another fighter's id (mariod's drcapsule)
- `vendor/hdr/fighters/kirby/src/acmd/copy/mario.rs:11` kirby/game_mariospecialairn `*FIGHTER_MARIO_GENERATE_ARTICLE_FIREBALL` (generate_article) — another fighter's id (mario's fireball)
- `vendor/hdr/fighters/kirby/src/acmd/copy/mario.rs:11` kirby/game_mariospecialn `*FIGHTER_MARIO_GENERATE_ARTICLE_FIREBALL` (generate_article) — another fighter's id (mario's fireball)
- `vendor/hdr/fighters/kirby/src/acmd/copy/mewtwo.rs:7` kirby/game_mewtwospecialairnstart `*FIGHTER_MEWTWO_GENERATE_ARTICLE_SHADOWBALL` (generate_article) — another fighter's id (mewtwo's shadowball)
- `vendor/hdr/fighters/kirby/src/acmd/copy/mewtwo.rs:7` kirby/game_mewtwospecialnstart `*FIGHTER_MEWTWO_GENERATE_ARTICLE_SHADOWBALL` (generate_article) — another fighter's id (mewtwo's shadowball)
- `vendor/hdr/fighters/kirby/src/acmd/copy/miifighter.rs:7` kirby/game_miifighterspecialairn1 `*FIGHTER_MIIFIGHTER_GENERATE_ARTICLE_IRONBALL` (generate_article_enable) — another fighter's id (miifighter's ironball)
- `vendor/hdr/fighters/kirby/src/acmd/copy/miifighter.rs:19` kirby/game_miifighterspecialairn1 `*FIGHTER_MIIFIGHTER_GENERATE_ARTICLE_IRONBALL` (shoot_exist) — another fighter's id (miifighter's ironball)
- `vendor/hdr/fighters/kirby/src/acmd/copy/miifighter.rs:59` kirby/game_miifighterspecialairn1bowl `*FIGHTER_MIIFIGHTER_GENERATE_ARTICLE_IRONBALL` (shoot_exist) — another fighter's id (miifighter's ironball)
- `vendor/hdr/fighters/kirby/src/acmd/copy/miifighter.rs:7` kirby/game_miifighterspecialn1 `*FIGHTER_MIIFIGHTER_GENERATE_ARTICLE_IRONBALL` (generate_article_enable) — another fighter's id (miifighter's ironball)
- `vendor/hdr/fighters/kirby/src/acmd/copy/miifighter.rs:19` kirby/game_miifighterspecialn1 `*FIGHTER_MIIFIGHTER_GENERATE_ARTICLE_IRONBALL` (shoot_exist) — another fighter's id (miifighter's ironball)
- `vendor/hdr/fighters/kirby/src/acmd/copy/miifighter.rs:59` kirby/game_miifighterspecialn1bowl `*FIGHTER_MIIFIGHTER_GENERATE_ARTICLE_IRONBALL` (shoot_exist) — another fighter's id (miifighter's ironball)
- `vendor/hdr/fighters/kirby/src/acmd/copy/miigunner.rs:8` kirby/game_miigunnerspecialairn1firemax `*FIGHTER_MIIGUNNER_GENERATE_ARTICLE_GUNNERCHARGE` (shoot_exist) — another fighter's id (miigunner's gunnercharge)
- `vendor/hdr/fighters/kirby/src/acmd/copy/miigunner.rs:45` kirby/game_miigunnerspecialairn1neon `*FIGHTER_MIIGUNNER_GENERATE_ARTICLE_GUNNERCHARGE` (remove_exist) — another fighter's id (miigunner's gunnercharge)
- `vendor/hdr/fighters/kirby/src/acmd/copy/miigunner.rs:8` kirby/game_miigunnerspecialn1firemax `*FIGHTER_MIIGUNNER_GENERATE_ARTICLE_GUNNERCHARGE` (shoot_exist) — another fighter's id (miigunner's gunnercharge)
- `vendor/hdr/fighters/kirby/src/acmd/copy/miigunner.rs:45` kirby/game_miigunnerspecialn1neon `*FIGHTER_MIIGUNNER_GENERATE_ARTICLE_GUNNERCHARGE` (remove_exist) — another fighter's id (miigunner's gunnercharge)
- `vendor/hdr/fighters/kirby/src/acmd/specials.rs:277` kirby/game_specialairhi `*FIGHTER_KIRBY_GENERATE_ARTICLE_FINALCUTTER` (generate_article) — environment: kirby is not in the local dump, so its vanilla `finalcutter` is not listed
- `vendor/hdr/fighters/kirby/src/acmd/specials.rs:278` kirby/game_specialairhi `*FIGHTER_KIRBY_GENERATE_ARTICLE_FINALCUTTER` (change_motion) — environment: kirby is not in the local dump, so its vanilla `finalcutter` is not listed
- `vendor/hdr/fighters/kirby/src/acmd/specials.rs:279` kirby/game_specialairhi `*FIGHTER_KIRBY_GENERATE_ARTICLE_FINALCUTTER` (get_article) — environment: kirby is not in the local dump, so its vanilla `finalcutter` is not listed
- `vendor/hdr/fighters/kirby/src/acmd/specials.rs:315` kirby/game_specialairhi2 `*FIGHTER_KIRBY_GENERATE_ARTICLE_FINALCUTTER` (change_motion) — environment: kirby is not in the local dump, so its vanilla `finalcutter` is not listed
- `vendor/hdr/fighters/kirby/src/acmd/specials.rs:480` kirby/game_specialairhih `*FIGHTER_KIRBY_GENERATE_ARTICLE_FINALCUTTER` (generate_article) — environment: kirby is not in the local dump, so its vanilla `finalcutter` is not listed
- `vendor/hdr/fighters/kirby/src/acmd/specials.rs:481` kirby/game_specialairhih `*FIGHTER_KIRBY_GENERATE_ARTICLE_FINALCUTTER` (change_motion) — environment: kirby is not in the local dump, so its vanilla `finalcutter` is not listed
- `vendor/hdr/fighters/kirby/src/acmd/specials.rs:482` kirby/game_specialairhih `*FIGHTER_KIRBY_GENERATE_ARTICLE_FINALCUTTER` (get_article) — environment: kirby is not in the local dump, so its vanilla `finalcutter` is not listed
- `vendor/hdr/fighters/kirby/src/acmd/specials.rs:505` kirby/game_specialairhih `*FIGHTER_KIRBY_GENERATE_ARTICLE_FINALCUTTER` (remove_exist) — environment: kirby is not in the local dump, so its vanilla `finalcutter` is not listed
- `vendor/hdr/fighters/kirby/src/acmd/specials.rs:616` kirby/game_specialairlw `*FIGHTER_KIRBY_GENERATE_ARTICLE_STONE` (change_motion) — environment: kirby is not in the local dump, so its vanilla `stone` is not listed
- `vendor/hdr/fighters/kirby/src/acmd/specials.rs:631` kirby/game_specialairlw `*FIGHTER_KIRBY_GENERATE_ARTICLE_STONE` (change_motion) — environment: kirby is not in the local dump, so its vanilla `stone` is not listed
- `vendor/hdr/fighters/kirby/src/acmd/specials.rs:734` kirby/game_specialairlw2 `*FIGHTER_KIRBY_GENERATE_ARTICLE_STONE` (change_motion) — environment: kirby is not in the local dump, so its vanilla `stone` is not listed
- `vendor/hdr/fighters/kirby/src/acmd/specials.rs:194` kirby/game_specialairs `*FIGHTER_KIRBY_GENERATE_ARTICLE_HAMMER` (remove_exist) — environment: kirby is not in the local dump, so its vanilla `hammer` is not listed
- `vendor/hdr/fighters/kirby/src/acmd/specials.rs:224` kirby/game_specialairss `*FIGHTER_KIRBY_GENERATE_ARTICLE_HAMMER` (remove_exist) — environment: kirby is not in the local dump, so its vanilla `hammer` is not listed
- `vendor/hdr/fighters/kirby/src/acmd/specials.rs:164` kirby/game_specialairsstart `*FIGHTER_KIRBY_GENERATE_ARTICLE_HAMMER` (generate_article) — environment: kirby is not in the local dump, so its vanilla `hammer` is not listed
- `vendor/hdr/fighters/kirby/src/acmd/specials.rs:232` kirby/game_specialhi `*FIGHTER_KIRBY_GENERATE_ARTICLE_FINALCUTTER` (generate_article) — environment: kirby is not in the local dump, so its vanilla `finalcutter` is not listed
- `vendor/hdr/fighters/kirby/src/acmd/specials.rs:233` kirby/game_specialhi `*FIGHTER_KIRBY_GENERATE_ARTICLE_FINALCUTTER` (change_motion) — environment: kirby is not in the local dump, so its vanilla `finalcutter` is not listed
- `vendor/hdr/fighters/kirby/src/acmd/specials.rs:234` kirby/game_specialhi `*FIGHTER_KIRBY_GENERATE_ARTICLE_FINALCUTTER` (get_article) — environment: kirby is not in the local dump, so its vanilla `finalcutter` is not listed
- `vendor/hdr/fighters/kirby/src/acmd/specials.rs:376` kirby/game_specialhih `*FIGHTER_KIRBY_GENERATE_ARTICLE_FINALCUTTER` (generate_article) — environment: kirby is not in the local dump, so its vanilla `finalcutter` is not listed
- `vendor/hdr/fighters/kirby/src/acmd/specials.rs:377` kirby/game_specialhih `*FIGHTER_KIRBY_GENERATE_ARTICLE_FINALCUTTER` (change_motion) — environment: kirby is not in the local dump, so its vanilla `finalcutter` is not listed
- `vendor/hdr/fighters/kirby/src/acmd/specials.rs:398` kirby/game_specialhih `*FIGHTER_KIRBY_GENERATE_ARTICLE_FINALCUTTER` (remove_exist) — environment: kirby is not in the local dump, so its vanilla `finalcutter` is not listed
- `vendor/hdr/fighters/kirby/src/acmd/specials.rs:587` kirby/game_speciallw `*FIGHTER_KIRBY_GENERATE_ARTICLE_STONE` (change_motion) — environment: kirby is not in the local dump, so its vanilla `stone` is not listed
- `vendor/hdr/fighters/kirby/src/acmd/specials.rs:598` kirby/game_speciallw `*FIGHTER_KIRBY_GENERATE_ARTICLE_STONE` (change_motion) — environment: kirby is not in the local dump, so its vanilla `stone` is not listed
- `vendor/hdr/fighters/kirby/src/acmd/specials.rs:722` kirby/game_speciallw2 `*FIGHTER_KIRBY_GENERATE_ARTICLE_STONE` (change_motion) — environment: kirby is not in the local dump, so its vanilla `stone` is not listed
- `vendor/hdr/fighters/kirby/src/acmd/specials.rs:701` kirby/game_speciallwtoair `*FIGHTER_KIRBY_GENERATE_ARTICLE_STONE` (change_motion) — environment: kirby is not in the local dump, so its vanilla `stone` is not listed
- `vendor/hdr/fighters/kirby/src/acmd/specials.rs:684` kirby/game_speciallwtoground `*FIGHTER_KIRBY_GENERATE_ARTICLE_STONE` (change_motion) — environment: kirby is not in the local dump, so its vanilla `stone` is not listed
- `vendor/hdr/fighters/kirby/src/acmd/specials.rs:105` kirby/game_specials `*FIGHTER_KIRBY_GENERATE_ARTICLE_HAMMER` (remove_exist) — environment: kirby is not in the local dump, so its vanilla `hammer` is not listed
- `vendor/hdr/fighters/kirby/src/acmd/specials.rs:155` kirby/game_specialsmax `*FIGHTER_KIRBY_GENERATE_ARTICLE_HAMMER` (remove_exist) — environment: kirby is not in the local dump, so its vanilla `hammer` is not listed
- `vendor/hdr/fighters/kirby/src/acmd/specials.rs:129` kirby/game_specialss `*FIGHTER_KIRBY_GENERATE_ARTICLE_HAMMER` (remove_exist) — environment: kirby is not in the local dump, so its vanilla `hammer` is not listed
- `vendor/hdr/fighters/kirby/src/acmd/specials.rs:81` kirby/game_specialsstart `*FIGHTER_KIRBY_GENERATE_ARTICLE_HAMMER` (generate_article) — environment: kirby is not in the local dump, so its vanilla `hammer` is not listed
- `vendor/hdr/fighters/kirby/src/acmd/copy/trail.rs:8` kirby/game_trailspecialairn2 `*FIGHTER_TRAIL_GENERATE_ARTICLE_FLOWER` (remove_exist) — another fighter's id (trail's flower)
- `vendor/hdr/fighters/kirby/src/acmd/copy/trail.rs:9` kirby/game_trailspecialairn2 `*FIGHTER_TRAIL_GENERATE_ARTICLE_FLOWER` (generate_article) — another fighter's id (trail's flower)
- `vendor/hdr/fighters/kirby/src/acmd/copy/trail.rs:10` kirby/game_trailspecialairn2 `*FIGHTER_TRAIL_GENERATE_ARTICLE_FLOWER` (change_motion) — another fighter's id (trail's flower)
- `vendor/hdr/fighters/kirby/src/acmd/copy/trail.rs:8` kirby/game_trailspecialn2 `*FIGHTER_TRAIL_GENERATE_ARTICLE_FLOWER` (remove_exist) — another fighter's id (trail's flower)
- `vendor/hdr/fighters/kirby/src/acmd/copy/trail.rs:9` kirby/game_trailspecialn2 `*FIGHTER_TRAIL_GENERATE_ARTICLE_FLOWER` (generate_article) — another fighter's id (trail's flower)
- `vendor/hdr/fighters/kirby/src/acmd/copy/trail.rs:10` kirby/game_trailspecialn2 `*FIGHTER_TRAIL_GENERATE_ARTICLE_FLOWER` (change_motion) — another fighter's id (trail's flower)
- `vendor/hdr/fighters/koopajr/src/acmd/specials.rs:134` koopajr/game_specialairlw `*FIGHTER_KOOPAJR_GENERATE_ARTICLE_MECHAKOOPA` (generate_article) — koopajr has no `mechakoopa` folder in the dump and no clone binds this name
- `vendor/hdr/fighters/koopajr/src/acmd/specials.rs:134` koopajr/game_speciallw `*FIGHTER_KOOPAJR_GENERATE_ARTICLE_MECHAKOOPA` (generate_article) — koopajr has no `mechakoopa` folder in the dump and no clone binds this name
- `vendor/hdr/fighters/master/src/acmd/aerials.rs:145` master/game_attackairb `*FIGHTER_MASTER_GENERATE_ARTICLE_SPEAR` (generate_article) — environment: master is not in the local dump, so its vanilla `spear` is not listed
- `vendor/hdr/fighters/master/src/acmd/aerials.rs:177` master/game_attackairb `*FIGHTER_MASTER_GENERATE_ARTICLE_SPEAR` (remove_exist) — environment: master is not in the local dump, so its vanilla `spear` is not listed
- `vendor/hdr/fighters/master/src/acmd/aerials.rs:88` master/game_attackairf `*FIGHTER_MASTER_GENERATE_ARTICLE_SPEAR` (generate_article) — environment: master is not in the local dump, so its vanilla `spear` is not listed
- `vendor/hdr/fighters/master/src/acmd/aerials.rs:88` master/game_attackairf `*FIGHTER_MASTER_GENERATE_ARTICLE_SPEAR` (generate_article) — environment: master is not in the local dump, so its vanilla `spear` is not listed
- `vendor/hdr/fighters/master/src/acmd/aerials.rs:120` master/game_attackairf `*FIGHTER_MASTER_GENERATE_ARTICLE_SPEAR` (remove_exist) — environment: master is not in the local dump, so its vanilla `spear` is not listed
- `vendor/hdr/fighters/master/src/acmd/aerials.rs:208` master/game_attackairhi `*FIGHTER_MASTER_GENERATE_ARTICLE_SWORD` (generate_article) — environment: master is not in the local dump, so its vanilla `sword` is not listed
- `vendor/hdr/fighters/master/src/acmd/aerials.rs:209` master/game_attackairhi `*FIGHTER_MASTER_GENERATE_ARTICLE_SWORD` (change_motion) — environment: master is not in the local dump, so its vanilla `sword` is not listed
- `vendor/hdr/fighters/master/src/acmd/aerials.rs:218` master/game_attackairhi `*FIGHTER_MASTER_GENERATE_ARTICLE_SWORD` (set_rate) — environment: master is not in the local dump, so its vanilla `sword` is not listed
- `vendor/hdr/fighters/master/src/acmd/aerials.rs:237` master/game_attackairhi `*FIGHTER_MASTER_GENERATE_ARTICLE_SWORD` (set_rate) — environment: master is not in the local dump, so its vanilla `sword` is not listed
- `vendor/hdr/fighters/master/src/acmd/aerials.rs:248` master/game_attackairhi `*FIGHTER_MASTER_GENERATE_ARTICLE_SWORD` (remove_exist) — environment: master is not in the local dump, so its vanilla `sword` is not listed
- `vendor/hdr/fighters/master/src/acmd/aerials.rs:271` master/game_attackairlw `*FIGHTER_MASTER_GENERATE_ARTICLE_AXE` (generate_article) — environment: master is not in the local dump, so its vanilla `axe` is not listed
- `vendor/hdr/fighters/master/src/acmd/aerials.rs:272` master/game_attackairlw `*FIGHTER_MASTER_GENERATE_ARTICLE_BOW` (change_motion) — environment: master is not in the local dump, so its vanilla `bow` is not listed
- `vendor/hdr/fighters/master/src/acmd/aerials.rs:295` master/game_attackairlw `*FIGHTER_MASTER_GENERATE_ARTICLE_AXE` (remove_exist) — environment: master is not in the local dump, so its vanilla `axe` is not listed
- `vendor/hdr/fighters/master/src/acmd/aerials.rs:7` master/game_attackairn `*FIGHTER_MASTER_GENERATE_ARTICLE_BOW` (generate_article) — environment: master is not in the local dump, so its vanilla `bow` is not listed
- `vendor/hdr/fighters/master/src/acmd/aerials.rs:8` master/game_attackairn `*FIGHTER_MASTER_GENERATE_ARTICLE_BOW` (change_motion) — environment: master is not in the local dump, so its vanilla `bow` is not listed
- `vendor/hdr/fighters/master/src/acmd/aerials.rs:14` master/game_attackairn `*FIGHTER_MASTER_GENERATE_ARTICLE_BOW` (set_rate) — environment: master is not in the local dump, so its vanilla `bow` is not listed
- `vendor/hdr/fighters/master/src/acmd/aerials.rs:27` master/game_attackairn `*FIGHTER_MASTER_GENERATE_ARTICLE_BOW` (set_rate) — environment: master is not in the local dump, so its vanilla `bow` is not listed
- `vendor/hdr/fighters/master/src/acmd/aerials.rs:33` master/game_attackairn `*FIGHTER_MASTER_GENERATE_ARTICLE_BOW` (set_rate) — environment: master is not in the local dump, so its vanilla `bow` is not listed
- `vendor/hdr/fighters/master/src/acmd/aerials.rs:47` master/game_attackairn `*FIGHTER_MASTER_GENERATE_ARTICLE_BOW` (remove_exist) — environment: master is not in the local dump, so its vanilla `bow` is not listed
- `vendor/hdr/fighters/master/src/acmd/smashes.rs:255` master/game_attackhi4 `*FIGHTER_MASTER_GENERATE_ARTICLE_SWORD` (generate_article) — environment: master is not in the local dump, so its vanilla `sword` is not listed
- `vendor/hdr/fighters/master/src/acmd/smashes.rs:256` master/game_attackhi4 `*FIGHTER_MASTER_GENERATE_ARTICLE_SWORD` (change_motion) — environment: master is not in the local dump, so its vanilla `sword` is not listed
- `vendor/hdr/fighters/master/src/acmd/smashes.rs:263` master/game_attackhi4 `*FIGHTER_MASTER_GENERATE_ARTICLE_SWORD` (change_motion) — environment: master is not in the local dump, so its vanilla `sword` is not listed
- `vendor/hdr/fighters/master/src/acmd/smashes.rs:298` master/game_attackhi4 `*FIGHTER_MASTER_GENERATE_ARTICLE_SWORD` (remove_exist) — environment: master is not in the local dump, so its vanilla `sword` is not listed
- `vendor/hdr/fighters/master/src/acmd/tilts.rs:145` master/game_attacklw3 `*FIGHTER_MASTER_GENERATE_ARTICLE_SWORD` (generate_article) — environment: master is not in the local dump, so its vanilla `sword` is not listed
- `vendor/hdr/fighters/master/src/acmd/tilts.rs:146` master/game_attacklw3 `*FIGHTER_MASTER_GENERATE_ARTICLE_SWORD` (change_motion) — environment: master is not in the local dump, so its vanilla `sword` is not listed
- `vendor/hdr/fighters/master/src/acmd/tilts.rs:159` master/game_attacklw3 `*FIGHTER_MASTER_GENERATE_ARTICLE_SWORD` (remove_exist) — environment: master is not in the local dump, so its vanilla `sword` is not listed
- `vendor/hdr/fighters/master/src/acmd/smashes.rs:306` master/game_attacklw4 `*FIGHTER_MASTER_GENERATE_ARTICLE_AXE` (generate_article) — environment: master is not in the local dump, so its vanilla `axe` is not listed
- `vendor/hdr/fighters/master/src/acmd/smashes.rs:307` master/game_attacklw4 `*FIGHTER_MASTER_GENERATE_ARTICLE_AXE` (change_motion) — environment: master is not in the local dump, so its vanilla `axe` is not listed
- `vendor/hdr/fighters/master/src/acmd/smashes.rs:314` master/game_attacklw4 `*FIGHTER_MASTER_GENERATE_ARTICLE_AXE` (change_motion) — environment: master is not in the local dump, so its vanilla `axe` is not listed
- `vendor/hdr/fighters/master/src/acmd/smashes.rs:341` master/game_attacklw4 `*FIGHTER_MASTER_GENERATE_ARTICLE_AXE` (remove_exist) — environment: master is not in the local dump, so its vanilla `axe` is not listed
- `vendor/hdr/fighters/master/src/acmd/smashes.rs:39` master/game_attacks4 `*FIGHTER_MASTER_GENERATE_ARTICLE_SPEAR` (generate_article) — environment: master is not in the local dump, so its vanilla `spear` is not listed
- `vendor/hdr/fighters/master/src/acmd/smashes.rs:127` master/game_attacks4 `*FIGHTER_MASTER_GENERATE_ARTICLE_SPEAR` (remove_exist) — environment: master is not in the local dump, so its vanilla `spear` is not listed
- `vendor/hdr/fighters/master/src/acmd/smashes.rs:39` master/game_attacks4hi `*FIGHTER_MASTER_GENERATE_ARTICLE_SPEAR` (generate_article) — environment: master is not in the local dump, so its vanilla `spear` is not listed
- `vendor/hdr/fighters/master/src/acmd/smashes.rs:127` master/game_attacks4hi `*FIGHTER_MASTER_GENERATE_ARTICLE_SPEAR` (remove_exist) — environment: master is not in the local dump, so its vanilla `spear` is not listed
- `vendor/hdr/fighters/master/src/acmd/smashes.rs:39` master/game_attacks4lw `*FIGHTER_MASTER_GENERATE_ARTICLE_SPEAR` (generate_article) — environment: master is not in the local dump, so its vanilla `spear` is not listed
- `vendor/hdr/fighters/master/src/acmd/smashes.rs:127` master/game_attacks4lw `*FIGHTER_MASTER_GENERATE_ARTICLE_SPEAR` (remove_exist) — environment: master is not in the local dump, so its vanilla `spear` is not listed
- `vendor/hdr/fighters/master/src/acmd/aerials.rs:79` master/game_landingairn `*FIGHTER_MASTER_GENERATE_ARTICLE_BOW` (change_motion) — environment: master is not in the local dump, so its vanilla `bow` is not listed
- `vendor/hdr/fighters/master/src/acmd/specials.rs:510` master/game_specialairhi `*FIGHTER_MASTER_GENERATE_ARTICLE_SWORD` (generate_article) — environment: master is not in the local dump, so its vanilla `sword` is not listed
- `vendor/hdr/fighters/master/src/acmd/specials.rs:511` master/game_specialairhi `*FIGHTER_MASTER_GENERATE_ARTICLE_SWORD` (change_motion) — environment: master is not in the local dump, so its vanilla `sword` is not listed
- `vendor/hdr/fighters/master/src/acmd/specials.rs:537` master/game_specialairhi `*FIGHTER_MASTER_GENERATE_ARTICLE_SWORD` (change_status) — environment: master is not in the local dump, so its vanilla `sword` is not listed
- `vendor/hdr/fighters/master/src/acmd/specials.rs:578` master/game_specialairhi `*FIGHTER_MASTER_GENERATE_ARTICLE_SWORD` (change_status) — environment: master is not in the local dump, so its vanilla `sword` is not listed
- `vendor/hdr/fighters/master/src/acmd/specials.rs:597` master/game_specialairhiovertake `*FIGHTER_MASTER_GENERATE_ARTICLE_SWORD` (generate_article) — environment: master is not in the local dump, so its vanilla `sword` is not listed
- `vendor/hdr/fighters/master/src/acmd/specials.rs:598` master/game_specialairhiovertake `*FIGHTER_MASTER_GENERATE_ARTICLE_SWORD` (change_motion) — environment: master is not in the local dump, so its vanilla `sword` is not listed
- `vendor/hdr/fighters/master/src/acmd/specials.rs:647` master/game_specialairlw `*FIGHTER_MASTER_GENERATE_ARTICLE_AXE` (set_rate) — environment: master is not in the local dump, so its vanilla `axe` is not listed
- `vendor/hdr/fighters/master/src/acmd/specials.rs:648` master/game_specialairlw `*FIGHTER_MASTER_GENERATE_ARTICLE_AXE` (set_flag) — environment: master is not in the local dump, so its vanilla `axe` is not listed
- `vendor/hdr/fighters/master/src/acmd/specials.rs:652` master/game_specialairlw `*FIGHTER_MASTER_GENERATE_ARTICLE_AXE` (set_flag) — environment: master is not in the local dump, so its vanilla `axe` is not listed
- `vendor/hdr/fighters/master/src/acmd/specials.rs:656` master/game_specialairlw `*FIGHTER_MASTER_GENERATE_ARTICLE_AXE` (remove_exist) — environment: master is not in the local dump, so its vanilla `axe` is not listed
- `vendor/hdr/fighters/master/src/acmd/specials.rs:703` master/game_specialairlwhit `*FIGHTER_MASTER_GENERATE_ARTICLE_AXE` (remove_exist) — environment: master is not in the local dump, so its vanilla `axe` is not listed
- `vendor/hdr/fighters/master/src/acmd/specials.rs:38` master/game_specialairn `*FIGHTER_MASTER_GENERATE_ARTICLE_ARROW1` (shoot_exist) — environment: master is not in the local dump, so its vanilla `arrow1` is not listed
- `vendor/hdr/fighters/master/src/acmd/specials.rs:42` master/game_specialairn `*FIGHTER_MASTER_GENERATE_ARTICLE_BOW` (remove_exist) — environment: master is not in the local dump, so its vanilla `bow` is not listed
- `vendor/hdr/fighters/master/src/acmd/specials.rs:10` master/game_specialairnstart `*FIGHTER_MASTER_GENERATE_ARTICLE_BOW` (remove_exist) — environment: master is not in the local dump, so its vanilla `bow` is not listed
- `vendor/hdr/fighters/master/src/acmd/specials.rs:12` master/game_specialairnstart `*FIGHTER_MASTER_GENERATE_ARTICLE_BOW` (generate_article) — environment: master is not in the local dump, so its vanilla `bow` is not listed
- `vendor/hdr/fighters/master/src/acmd/specials.rs:14` master/game_specialairnstart `*FIGHTER_MASTER_GENERATE_ARTICLE_ARROW1` (generate_article) — environment: master is not in the local dump, so its vanilla `arrow1` is not listed
- `vendor/hdr/fighters/master/src/acmd/specials.rs:331` master/game_specialairsf `*FIGHTER_MASTER_GENERATE_ARTICLE_SPEAR` (remove_exist) — environment: master is not in the local dump, so its vanilla `spear` is not listed
- `vendor/hdr/fighters/master/src/acmd/specials.rs:405` master/game_specialairsfdash `*FIGHTER_MASTER_GENERATE_ARTICLE_SPEAR` (remove_exist) — environment: master is not in the local dump, so its vanilla `spear` is not listed
- `vendor/hdr/fighters/master/src/acmd/specials.rs:52` master/game_specialairsstart `*FIGHTER_MASTER_GENERATE_ARTICLE_SPEAR` (generate_article) — environment: master is not in the local dump, so its vanilla `spear` is not listed
- `vendor/hdr/fighters/master/src/acmd/specials.rs:453` master/game_specialhi `*FIGHTER_MASTER_GENERATE_ARTICLE_SWORD` (generate_article) — environment: master is not in the local dump, so its vanilla `sword` is not listed
- `vendor/hdr/fighters/master/src/acmd/specials.rs:457` master/game_specialhi `*FIGHTER_MASTER_GENERATE_ARTICLE_SWORD` (change_motion) — environment: master is not in the local dump, so its vanilla `sword` is not listed
- `vendor/hdr/fighters/master/src/acmd/specials.rs:471` master/game_specialhi `*FIGHTER_MASTER_GENERATE_ARTICLE_SWORD` (change_status) — environment: master is not in the local dump, so its vanilla `sword` is not listed
- `vendor/hdr/fighters/master/src/acmd/specials.rs:499` master/game_specialhi `*FIGHTER_MASTER_GENERATE_ARTICLE_SWORD` (change_status) — environment: master is not in the local dump, so its vanilla `sword` is not listed
- `vendor/hdr/fighters/master/src/acmd/specials.rs:647` master/game_speciallw `*FIGHTER_MASTER_GENERATE_ARTICLE_AXE` (set_rate) — environment: master is not in the local dump, so its vanilla `axe` is not listed
- `vendor/hdr/fighters/master/src/acmd/specials.rs:648` master/game_speciallw `*FIGHTER_MASTER_GENERATE_ARTICLE_AXE` (set_flag) — environment: master is not in the local dump, so its vanilla `axe` is not listed
- `vendor/hdr/fighters/master/src/acmd/specials.rs:652` master/game_speciallw `*FIGHTER_MASTER_GENERATE_ARTICLE_AXE` (set_flag) — environment: master is not in the local dump, so its vanilla `axe` is not listed
- `vendor/hdr/fighters/master/src/acmd/specials.rs:656` master/game_speciallw `*FIGHTER_MASTER_GENERATE_ARTICLE_AXE` (remove_exist) — environment: master is not in the local dump, so its vanilla `axe` is not listed
- `vendor/hdr/fighters/master/src/acmd/specials.rs:703` master/game_speciallwhit `*FIGHTER_MASTER_GENERATE_ARTICLE_AXE` (remove_exist) — environment: master is not in the local dump, so its vanilla `axe` is not listed
- `vendor/hdr/fighters/master/src/acmd/specials.rs:38` master/game_specialn `*FIGHTER_MASTER_GENERATE_ARTICLE_ARROW1` (shoot_exist) — environment: master is not in the local dump, so its vanilla `arrow1` is not listed
- `vendor/hdr/fighters/master/src/acmd/specials.rs:42` master/game_specialn `*FIGHTER_MASTER_GENERATE_ARTICLE_BOW` (remove_exist) — environment: master is not in the local dump, so its vanilla `bow` is not listed
- `vendor/hdr/fighters/master/src/acmd/specials.rs:10` master/game_specialnstart `*FIGHTER_MASTER_GENERATE_ARTICLE_BOW` (remove_exist) — environment: master is not in the local dump, so its vanilla `bow` is not listed
- `vendor/hdr/fighters/master/src/acmd/specials.rs:12` master/game_specialnstart `*FIGHTER_MASTER_GENERATE_ARTICLE_BOW` (generate_article) — environment: master is not in the local dump, so its vanilla `bow` is not listed
- `vendor/hdr/fighters/master/src/acmd/specials.rs:14` master/game_specialnstart `*FIGHTER_MASTER_GENERATE_ARTICLE_ARROW1` (generate_article) — environment: master is not in the local dump, so its vanilla `arrow1` is not listed
- `vendor/hdr/fighters/master/src/acmd/specials.rs:135` master/game_specialsf `*FIGHTER_MASTER_GENERATE_ARTICLE_SPEAR` (remove_exist) — environment: master is not in the local dump, so its vanilla `spear` is not listed
- `vendor/hdr/fighters/master/src/acmd/specials.rs:222` master/game_specialsfdash `*FIGHTER_MASTER_GENERATE_ARTICLE_SPEAR` (remove_exist) — environment: master is not in the local dump, so its vanilla `spear` is not listed
- `vendor/hdr/fighters/master/src/acmd/specials.rs:52` master/game_specialsstart `*FIGHTER_MASTER_GENERATE_ARTICLE_SPEAR` (generate_article) — environment: master is not in the local dump, so its vanilla `spear` is not listed
- `vendor/hdr/fighters/master/src/acmd/throws.rs:93` master/game_throwb `*FIGHTER_MASTER_GENERATE_ARTICLE_SWORD` (generate_article) — environment: master is not in the local dump, so its vanilla `sword` is not listed
- `vendor/hdr/fighters/master/src/acmd/throws.rs:94` master/game_throwb `*FIGHTER_MASTER_GENERATE_ARTICLE_SWORD` (change_motion) — environment: master is not in the local dump, so its vanilla `sword` is not listed
- `vendor/hdr/fighters/master/src/acmd/throws.rs:127` master/game_throwb `*FIGHTER_MASTER_GENERATE_ARTICLE_SWORD` (set_rate) — environment: master is not in the local dump, so its vanilla `sword` is not listed
- `vendor/hdr/fighters/master/src/acmd/throws.rs:127` master/game_throwb `*FIGHTER_MASTER_GENERATE_ARTICLE_SWORD` (set_rate) — environment: master is not in the local dump, so its vanilla `sword` is not listed
- `vendor/hdr/fighters/master/src/acmd/throws.rs:137` master/game_throwb `*FIGHTER_MASTER_GENERATE_ARTICLE_SWORD` (remove_exist) — environment: master is not in the local dump, so its vanilla `sword` is not listed
- `vendor/hdr/fighters/master/src/acmd/throws.rs:55` master/game_throwf `*FIGHTER_MASTER_GENERATE_ARTICLE_SWORD` (generate_article) — environment: master is not in the local dump, so its vanilla `sword` is not listed
- `vendor/hdr/fighters/master/src/acmd/throws.rs:56` master/game_throwf `*FIGHTER_MASTER_GENERATE_ARTICLE_SWORD` (change_motion) — environment: master is not in the local dump, so its vanilla `sword` is not listed
- `vendor/hdr/fighters/master/src/acmd/throws.rs:75` master/game_throwf `*FIGHTER_MASTER_GENERATE_ARTICLE_SWORD` (set_rate) — environment: master is not in the local dump, so its vanilla `sword` is not listed
- `vendor/hdr/fighters/master/src/acmd/throws.rs:75` master/game_throwf `*FIGHTER_MASTER_GENERATE_ARTICLE_SWORD` (set_rate) — environment: master is not in the local dump, so its vanilla `sword` is not listed
- `vendor/hdr/fighters/master/src/acmd/throws.rs:85` master/game_throwf `*FIGHTER_MASTER_GENERATE_ARTICLE_SWORD` (remove_exist) — environment: master is not in the local dump, so its vanilla `sword` is not listed
- `vendor/hdr/fighters/master/src/acmd/throws.rs:145` master/game_throwlw `*FIGHTER_MASTER_GENERATE_ARTICLE_SWORD` (generate_article) — environment: master is not in the local dump, so its vanilla `sword` is not listed
- `vendor/hdr/fighters/master/src/acmd/throws.rs:146` master/game_throwlw `*FIGHTER_MASTER_GENERATE_ARTICLE_SWORD` (change_motion) — environment: master is not in the local dump, so its vanilla `sword` is not listed
- `vendor/hdr/fighters/master/src/acmd/throws.rs:165` master/game_throwlw `*FIGHTER_MASTER_GENERATE_ARTICLE_SWORD` (set_rate) — environment: master is not in the local dump, so its vanilla `sword` is not listed
- `vendor/hdr/fighters/master/src/acmd/throws.rs:165` master/game_throwlw `*FIGHTER_MASTER_GENERATE_ARTICLE_SWORD` (set_rate) — environment: master is not in the local dump, so its vanilla `sword` is not listed
- `vendor/hdr/fighters/master/src/acmd/throws.rs:175` master/game_throwlw `*FIGHTER_MASTER_GENERATE_ARTICLE_SWORD` (remove_exist) — environment: master is not in the local dump, so its vanilla `sword` is not listed
- `vendor/hdr/fighters/metaknight/src/acmd/specials/special_lw.rs:91` metaknight/game_specialairlw `*FIGHTER_METAKNIGHT_GENERATE_ARTICLE_MANTLE` (change_motion) — environment: metaknight is not in the local dump, so its vanilla `mantle` is not listed
- `vendor/hdr/fighters/metaknight/src/acmd/specials/special_lw.rs:308` metaknight/game_specialairlwb `*FIGHTER_METAKNIGHT_GENERATE_ARTICLE_MANTLE` (change_motion) — environment: metaknight is not in the local dump, so its vanilla `mantle` is not listed
- `vendor/hdr/fighters/metaknight/src/acmd/specials/special_lw.rs:195` metaknight/game_specialairlwf `*FIGHTER_METAKNIGHT_GENERATE_ARTICLE_MANTLE` (change_motion) — environment: metaknight is not in the local dump, so its vanilla `mantle` is not listed
- `vendor/hdr/fighters/metaknight/src/acmd/specials/special_lw.rs:12` metaknight/game_speciallw `*FIGHTER_METAKNIGHT_GENERATE_ARTICLE_MANTLE` (change_motion) — environment: metaknight is not in the local dump, so its vanilla `mantle` is not listed
- `vendor/hdr/fighters/metaknight/src/acmd/specials/special_lw.rs:227` metaknight/game_speciallwb `*FIGHTER_METAKNIGHT_GENERATE_ARTICLE_MANTLE` (change_motion) — environment: metaknight is not in the local dump, so its vanilla `mantle` is not listed
- `vendor/hdr/fighters/metaknight/src/acmd/specials/special_lw.rs:120` metaknight/game_speciallwf `*FIGHTER_METAKNIGHT_GENERATE_ARTICLE_MANTLE` (change_motion) — environment: metaknight is not in the local dump, so its vanilla `mantle` is not listed
- `vendor/hdr/fighters/miifighter/src/acmd/specials/special_n/special_n1.rs:11` miifighter/game_specialairn1 `*FIGHTER_MIIFIGHTER_GENERATE_ARTICLE_IRONBALL` (generate_article_enable) — miifighter has no `ironball` folder in the dump and no clone binds this name
- `vendor/hdr/fighters/miifighter/src/acmd/specials/special_n/special_n1.rs:23` miifighter/game_specialairn1 `*FIGHTER_MIIFIGHTER_GENERATE_ARTICLE_IRONBALL` (shoot_exist) — miifighter has no `ironball` folder in the dump and no clone binds this name
- `vendor/hdr/fighters/miifighter/src/acmd/specials/special_n/special_n1.rs:63` miifighter/game_specialairn1bowl `*FIGHTER_MIIFIGHTER_GENERATE_ARTICLE_IRONBALL` (shoot_exist) — miifighter has no `ironball` folder in the dump and no clone binds this name
- `vendor/hdr/fighters/miifighter/src/acmd/specials/special_n/special_n1.rs:11` miifighter/game_specialn1 `*FIGHTER_MIIFIGHTER_GENERATE_ARTICLE_IRONBALL` (generate_article_enable) — miifighter has no `ironball` folder in the dump and no clone binds this name
- `vendor/hdr/fighters/miifighter/src/acmd/specials/special_n/special_n1.rs:23` miifighter/game_specialn1 `*FIGHTER_MIIFIGHTER_GENERATE_ARTICLE_IRONBALL` (shoot_exist) — miifighter has no `ironball` folder in the dump and no clone binds this name
- `vendor/hdr/fighters/miifighter/src/acmd/specials/special_n/special_n1.rs:63` miifighter/game_specialn1bowl `*FIGHTER_MIIFIGHTER_GENERATE_ARTICLE_IRONBALL` (shoot_exist) — miifighter has no `ironball` folder in the dump and no clone binds this name
- `vendor/hdr/fighters/miigunner/src/acmd/aerials/attack_air_f.rs:17` miigunner/game_attackairf `*FIGHTER_MIIGUNNER_GENERATE_ARTICLE_ATTACKAIRF_BULLET` (generate_article) — environment: miigunner is not in the local dump, so its vanilla `attackairf_bullet` is not listed
- `vendor/hdr/fighters/miigunner/src/acmd/aerials/attack_air_f.rs:59` miigunner/game_attackairfboost `*FIGHTER_MIIGUNNER_GENERATE_ARTICLE_ATTACKAIRF_BULLET` (generate_article) — environment: miigunner is not in the local dump, so its vanilla `attackairf_bullet` is not listed
- `vendor/hdr/fighters/miigunner/src/acmd/specials/special_hi.rs:21` miigunner/game_specialairhi1 `*FIGHTER_MIIGUNNER_GENERATE_ARTICLE_BOTTOMSHOOT` (generate_article) — environment: miigunner is not in the local dump, so its vanilla `bottomshoot` is not listed
- `vendor/hdr/fighters/miigunner/src/acmd/specials/special_lw.rs:56` miigunner/game_specialairlw2 `*FIGHTER_MIIGUNNER_GENERATE_ARTICLE_GROUNDBOMB` (generate_article) — environment: miigunner is not in the local dump, so its vanilla `groundbomb` is not listed
- `vendor/hdr/fighters/miigunner/src/acmd/specials/special_n.rs:26` miigunner/game_specialairn1firemax `*FIGHTER_MIIGUNNER_GENERATE_ARTICLE_GUNNERCHARGE` (shoot_exist) — environment: miigunner is not in the local dump, so its vanilla `gunnercharge` is not listed
- `vendor/hdr/fighters/miigunner/src/acmd/specials/special_n.rs:63` miigunner/game_specialairn1neon `*FIGHTER_MIIGUNNER_GENERATE_ARTICLE_GUNNERCHARGE` (remove_exist) — environment: miigunner is not in the local dump, so its vanilla `gunnercharge` is not listed
- `vendor/hdr/fighters/miigunner/src/acmd/specials/special_n.rs:12` miigunner/game_specialairn1start `*FIGHTER_MIIGUNNER_GENERATE_ARTICLE_GUNNERCHARGE` (generate_article_enable) — environment: miigunner is not in the local dump, so its vanilla `gunnercharge` is not listed
- `vendor/hdr/fighters/miigunner/src/acmd/specials/special_n.rs:155` miigunner/game_specialairn2loop `*FIGHTER_MIIGUNNER_GENERATE_ARTICLE_RAPIDSHOT_BULLET` (generate_article) — environment: miigunner is not in the local dump, so its vanilla `rapidshot_bullet` is not listed
- `vendor/hdr/fighters/miigunner/src/acmd/specials/special_n.rs:195` miigunner/game_specialairn3end `*FIGHTER_MIIGUNNER_GENERATE_ARTICLE_GRENADELAUNCHER` (generate_article_enable) — environment: miigunner is not in the local dump, so its vanilla `grenadelauncher` is not listed
- `vendor/hdr/fighters/miigunner/src/acmd/specials/special_s.rs:15` miigunner/game_specialairs1 `*FIGHTER_MIIGUNNER_GENERATE_ARTICLE_FLAMEPILLAR` (generate_article) — environment: miigunner is not in the local dump, so its vanilla `flamepillar` is not listed
- `vendor/hdr/fighters/miigunner/src/acmd/specials/special_hi.rs:21` miigunner/game_specialhi1 `*FIGHTER_MIIGUNNER_GENERATE_ARTICLE_BOTTOMSHOOT` (generate_article) — environment: miigunner is not in the local dump, so its vanilla `bottomshoot` is not listed
- `vendor/hdr/fighters/miigunner/src/acmd/specials/special_lw.rs:56` miigunner/game_speciallw2 `*FIGHTER_MIIGUNNER_GENERATE_ARTICLE_GROUNDBOMB` (generate_article) — environment: miigunner is not in the local dump, so its vanilla `groundbomb` is not listed
- `vendor/hdr/fighters/miigunner/src/acmd/specials/special_n.rs:26` miigunner/game_specialn1firemax `*FIGHTER_MIIGUNNER_GENERATE_ARTICLE_GUNNERCHARGE` (shoot_exist) — environment: miigunner is not in the local dump, so its vanilla `gunnercharge` is not listed
- `vendor/hdr/fighters/miigunner/src/acmd/specials/special_n.rs:63` miigunner/game_specialn1neon `*FIGHTER_MIIGUNNER_GENERATE_ARTICLE_GUNNERCHARGE` (remove_exist) — environment: miigunner is not in the local dump, so its vanilla `gunnercharge` is not listed
- `vendor/hdr/fighters/miigunner/src/acmd/specials/special_n.rs:12` miigunner/game_specialn1start `*FIGHTER_MIIGUNNER_GENERATE_ARTICLE_GUNNERCHARGE` (generate_article_enable) — environment: miigunner is not in the local dump, so its vanilla `gunnercharge` is not listed
- `vendor/hdr/fighters/miigunner/src/acmd/specials/special_n.rs:155` miigunner/game_specialn2loop `*FIGHTER_MIIGUNNER_GENERATE_ARTICLE_RAPIDSHOT_BULLET` (generate_article) — environment: miigunner is not in the local dump, so its vanilla `rapidshot_bullet` is not listed
- `vendor/hdr/fighters/miigunner/src/acmd/specials/special_n.rs:195` miigunner/game_specialn3end `*FIGHTER_MIIGUNNER_GENERATE_ARTICLE_GRENADELAUNCHER` (generate_article_enable) — environment: miigunner is not in the local dump, so its vanilla `grenadelauncher` is not listed
- `vendor/hdr/fighters/miigunner/src/acmd/specials/special_s.rs:15` miigunner/game_specials1 `*FIGHTER_MIIGUNNER_GENERATE_ARTICLE_FLAMEPILLAR` (generate_article) — environment: miigunner is not in the local dump, so its vanilla `flamepillar` is not listed
- `vendor/hdr/fighters/miigunner/src/acmd/throws.rs:105` miigunner/game_throwb `*FIGHTER_MIIGUNNER_GENERATE_ARTICLE_RAPIDSHOT_BULLET` (generate_article) — environment: miigunner is not in the local dump, so its vanilla `rapidshot_bullet` is not listed
- `vendor/hdr/fighters/miigunner/src/acmd/throws.rs:106` miigunner/game_throwb `*FIGHTER_MIIGUNNER_GENERATE_ARTICLE_RAPIDSHOT_BULLET` (change_motion) — environment: miigunner is not in the local dump, so its vanilla `rapidshot_bullet` is not listed
- `vendor/hdr/fighters/miigunner/src/acmd/throws.rs:163` miigunner/game_throwhi `*FIGHTER_MIIGUNNER_GENERATE_ARTICLE_RAPIDSHOT_BULLET` (generate_article) — environment: miigunner is not in the local dump, so its vanilla `rapidshot_bullet` is not listed
- `vendor/hdr/fighters/miigunner/src/acmd/throws.rs:164` miigunner/game_throwhi `*FIGHTER_MIIGUNNER_GENERATE_ARTICLE_RAPIDSHOT_BULLET` (change_motion) — environment: miigunner is not in the local dump, so its vanilla `rapidshot_bullet` is not listed
- `vendor/hdr/fighters/miigunner/src/acmd/throws.rs:163` miigunner/game_throwhi `*FIGHTER_MIIGUNNER_GENERATE_ARTICLE_RAPIDSHOT_BULLET` (generate_article) — environment: miigunner is not in the local dump, so its vanilla `rapidshot_bullet` is not listed
- `vendor/hdr/fighters/miigunner/src/acmd/throws.rs:173` miigunner/game_throwhi `*FIGHTER_MIIGUNNER_GENERATE_ARTICLE_RAPIDSHOT_BULLET` (change_motion) — environment: miigunner is not in the local dump, so its vanilla `rapidshot_bullet` is not listed
- `vendor/hdr/fighters/ness/src/acmd/smashes.rs:47` ness/game_attackhi4 `*FIGHTER_NESS_GENERATE_ARTICLE_YOYO_HEAD` (set_visibility_whole) — environment: ness is not in the local dump, so its vanilla `yoyo_head` is not listed
- `vendor/hdr/fighters/ness/src/acmd/smashes.rs:48` ness/game_attackhi4 `*FIGHTER_NESS_GENERATE_ARTICLE_YOYO` (set_visibility_whole) — environment: ness is not in the local dump, so its vanilla `yoyo` is not listed
- `vendor/hdr/fighters/ness/src/acmd/smashes.rs:52` ness/game_attackhi4 `*FIGHTER_NESS_GENERATE_ARTICLE_YOYO_HEAD` (shoot) — environment: ness is not in the local dump, so its vanilla `yoyo_head` is not listed
- `vendor/hdr/fighters/ness/src/acmd/smashes.rs:61` ness/game_attackhi4 `*FIGHTER_NESS_GENERATE_ARTICLE_YOYO` (remove_exist) — environment: ness is not in the local dump, so its vanilla `yoyo` is not listed
- `vendor/hdr/fighters/ness/src/acmd/smashes.rs:62` ness/game_attackhi4 `*FIGHTER_NESS_GENERATE_ARTICLE_YOYO_HEAD` (remove_exist) — environment: ness is not in the local dump, so its vanilla `yoyo_head` is not listed
- `vendor/hdr/fighters/ness/src/acmd/smashes.rs:97` ness/game_attacklw4 `*FIGHTER_NESS_GENERATE_ARTICLE_YOYO_HEAD` (set_visibility_whole) — environment: ness is not in the local dump, so its vanilla `yoyo_head` is not listed
- `vendor/hdr/fighters/ness/src/acmd/smashes.rs:98` ness/game_attacklw4 `*FIGHTER_NESS_GENERATE_ARTICLE_YOYO` (set_visibility_whole) — environment: ness is not in the local dump, so its vanilla `yoyo` is not listed
- `vendor/hdr/fighters/ness/src/acmd/smashes.rs:102` ness/game_attacklw4 `*FIGHTER_NESS_GENERATE_ARTICLE_YOYO_HEAD` (shoot) — environment: ness is not in the local dump, so its vanilla `yoyo_head` is not listed
- `vendor/hdr/fighters/ness/src/acmd/smashes.rs:110` ness/game_attacklw4 `*FIGHTER_NESS_GENERATE_ARTICLE_YOYO` (remove_exist) — environment: ness is not in the local dump, so its vanilla `yoyo` is not listed
- `vendor/hdr/fighters/ness/src/acmd/smashes.rs:111` ness/game_attacklw4 `*FIGHTER_NESS_GENERATE_ARTICLE_YOYO_HEAD` (remove_exist) — environment: ness is not in the local dump, so its vanilla `yoyo_head` is not listed
- `vendor/hdr/fighters/ness/src/acmd/specials.rs:52` ness/game_specialairs `*FIGHTER_NESS_GENERATE_ARTICLE_PK_FIRE` (generate_article) — environment: ness is not in the local dump, so its vanilla `pk_fire` is not listed
- `vendor/hdr/fighters/ness/src/acmd/specials.rs:37` ness/game_specials `*FIGHTER_NESS_GENERATE_ARTICLE_PK_FIRE` (generate_article) — environment: ness is not in the local dump, so its vanilla `pk_fire` is not listed
- `vendor/hdr/fighters/palutena/src/acmd/specials/special_n_p.rs:11` palutena/game_specialairnp `articles::palutena::METEOR` (generate_article) — an expression Visionary does not follow
- `vendor/hdr/fighters/palutena/src/acmd/specials/special_n_p.rs:11` palutena/game_specialnp `articles::palutena::METEOR` (generate_article) — an expression Visionary does not follow
- `vendor/hdr/fighters/pikmin/src/acmd/specials.rs:20` pikmin/game_specialnstart `*FIGHTER_PIKMIN_GENERATE_ARTICLE_PIKMIN` (generate_article) — environment: pikmin is not in the local dump, so its vanilla `pikmin` is not listed
- `vendor/hdr/fighters/pit/src/acmd/specials.rs:25` pit/game_specialairnfirehi `*FIGHTER_PIT_GENERATE_ARTICLE_BOWARROW` (shoot) — environment: pit is not in the local dump, so its vanilla `bowarrow` is not listed
- `vendor/hdr/fighters/pit/src/acmd/specials.rs:12` pit/game_specialairnfires `*FIGHTER_PIT_GENERATE_ARTICLE_BOWARROW` (shoot) — environment: pit is not in the local dump, so its vanilla `bowarrow` is not listed
- `vendor/hdr/fighters/plizardon/src/acmd/smashes.rs:8` plizardon/game_attacks4 `articles::plizardon::ROCK` (generate_article) — an expression Visionary does not follow
- `vendor/hdr/fighters/plizardon/src/acmd/smashes.rs:25` plizardon/game_attacks4 `articles::plizardon::ROCK` (get_article) — an expression Visionary does not follow
- `vendor/hdr/fighters/purin/src/acmd/specials.rs:8` purin/game_specialairnstartr `articles::purin::DISARMING_VOICE` (generate_article) — an expression Visionary does not follow
- `vendor/hdr/fighters/purin/src/acmd/specials.rs:8` purin/game_specialnstartr `articles::purin::DISARMING_VOICE` (generate_article) — an expression Visionary does not follow
- `vendor/hdr/fighters/pzenigame/src/acmd/specials.rs:40` pzenigame/game_specialairnshot `*FIGHTER_PZENIGAME_GENERATE_ARTICLE_WATER` (generate_article) — environment: pzenigame is not in the local dump, so its vanilla `water` is not listed
- `vendor/hdr/fighters/pzenigame/src/acmd/specials.rs:40` pzenigame/game_specialnshot `*FIGHTER_PZENIGAME_GENERATE_ARTICLE_WATER` (generate_article) — environment: pzenigame is not in the local dump, so its vanilla `water` is not listed
- `vendor/hdr/fighters/pzenigame/src/acmd/throws.rs:93` pzenigame/game_throwf `*FIGHTER_PZENIGAME_GENERATE_ARTICLE_WATER` (generate_article) — environment: pzenigame is not in the local dump, so its vanilla `water` is not listed
- `vendor/hdr/fighters/reflet/src/acmd/specials.rs:88` reflet/game_specialairhi `*FIGHTER_REFLET_GENERATE_ARTICLE_ELWIND` (generate_article) — environment: reflet is not in the local dump, so its vanilla `elwind` is not listed
- `vendor/hdr/fighters/reflet/src/acmd/specials.rs:124` reflet/game_specialairhi2 `*FIGHTER_REFLET_GENERATE_ARTICLE_ELWIND` (generate_article) — environment: reflet is not in the local dump, so its vanilla `elwind` is not listed
- `vendor/hdr/fighters/reflet/src/acmd/specials.rs:88` reflet/game_specialhi `*FIGHTER_REFLET_GENERATE_ARTICLE_ELWIND` (generate_article) — environment: reflet is not in the local dump, so its vanilla `elwind` is not listed
- `vendor/hdr/fighters/reflet/src/acmd/specials.rs:124` reflet/game_specialhi2 `*FIGHTER_REFLET_GENERATE_ARTICLE_ELWIND` (generate_article) — environment: reflet is not in the local dump, so its vanilla `elwind` is not listed
- `vendor/hdr/fighters/richter/src/acmd/specials.rs:8` richter/game_specialairn `*FIGHTER_SIMON_GENERATE_ARTICLE_AXE` (generate_article) — another fighter's id (simon's axe)
- `vendor/hdr/fighters/richter/src/acmd/specials.rs:12` richter/game_specialairn `*FIGHTER_SIMON_GENERATE_ARTICLE_AXE` (shoot) — another fighter's id (simon's axe)
- `vendor/hdr/fighters/richter/src/acmd/specials.rs:8` richter/game_specialn `*FIGHTER_SIMON_GENERATE_ARTICLE_AXE` (generate_article) — another fighter's id (simon's axe)
- `vendor/hdr/fighters/richter/src/acmd/specials.rs:12` richter/game_specialn `*FIGHTER_SIMON_GENERATE_ARTICLE_AXE` (shoot) — another fighter's id (simon's axe)
- `vendor/hdr/fighters/robot/src/acmd/specials.rs:422` robot/game_specialairlw `*FIGHTER_ROBOT_GENERATE_ARTICLE_GYRO` (set_visibility_whole) — environment: robot is not in the local dump, so its vanilla `gyro` is not listed
- `vendor/hdr/fighters/robot/src/acmd/specials.rs:422` robot/game_speciallw `*FIGHTER_ROBOT_GENERATE_ARTICLE_GYRO` (set_visibility_whole) — environment: robot is not in the local dump, so its vanilla `gyro` is not listed
- `vendor/hdr/fighters/ryu/src/acmd/finals.rs:137` ryu/game_final2 `*FIGHTER_RYU_GENERATE_ARTICLE_SHINKUHADOKEN` (generate_article) — environment: ryu is not in the local dump, so its vanilla `shinkuhadoken` is not listed
- `vendor/hdr/fighters/ryu/src/acmd/finals.rs:137` ryu/game_finalair2 `*FIGHTER_RYU_GENERATE_ARTICLE_SHINKUHADOKEN` (generate_article) — environment: ryu is not in the local dump, so its vanilla `shinkuhadoken` is not listed
- `vendor/hdr/fighters/ryu/src/acmd/specials/special_n.rs:33` ryu/game_specialairn `*FIGHTER_RYU_GENERATE_ARTICLE_HADOKEN` (generate_article) — environment: ryu is not in the local dump, so its vanilla `hadoken` is not listed
- `vendor/hdr/fighters/ryu/src/acmd/specials/special_n.rs:33` ryu/game_specialn `*FIGHTER_RYU_GENERATE_ARTICLE_HADOKEN` (generate_article) — environment: ryu is not in the local dump, so its vanilla `hadoken` is not listed
- `vendor/hdr/fighters/shizue/src/acmd/ground.rs:7` shizue/game_attack11 `*FIGHTER_SHIZUE_GENERATE_ARTICLE_PICOPICOHAMMER` (generate_article) — environment: shizue is not in the local dump, so its vanilla `picopicohammer` is not listed
- `vendor/hdr/fighters/shizue/src/acmd/ground.rs:20` shizue/game_attack11 `*FIGHTER_SHIZUE_GENERATE_ARTICLE_PICOPICOHAMMER` (remove_exist) — environment: shizue is not in the local dump, so its vanilla `picopicohammer` is not listed
- `vendor/hdr/fighters/shizue/src/acmd/aerials.rs:123` shizue/game_attackairb `*FIGHTER_SHIZUE_GENERATE_ARTICLE_SLINGSHOT` (generate_article) — environment: shizue is not in the local dump, so its vanilla `slingshot` is not listed
- `vendor/hdr/fighters/shizue/src/acmd/aerials.rs:8` shizue/game_attackairn `*FIGHTER_SHIZUE_GENERATE_ARTICLE_POMPON` (generate_article) — environment: shizue is not in the local dump, so its vanilla `pompon` is not listed
- `vendor/hdr/fighters/shizue/src/acmd/aerials.rs:8` shizue/game_attackairn `*FIGHTER_SHIZUE_GENERATE_ARTICLE_POMPON` (generate_article) — environment: shizue is not in the local dump, so its vanilla `pompon` is not listed
- `vendor/hdr/fighters/shizue/src/acmd/aerials.rs:11` shizue/game_attackairn `*FIGHTER_SHIZUE_GENERATE_ARTICLE_POMPON` (change_motion) — environment: shizue is not in the local dump, so its vanilla `pompon` is not listed
- `vendor/hdr/fighters/shizue/src/acmd/aerials.rs:41` shizue/game_attackairn `*FIGHTER_SHIZUE_GENERATE_ARTICLE_POMPON` (remove_exist) — environment: shizue is not in the local dump, so its vanilla `pompon` is not listed
- `vendor/hdr/fighters/shizue/src/acmd/tilts.rs:42` shizue/game_attackhi3 `*FIGHTER_SHIZUE_GENERATE_ARTICLE_BROOM` (generate_article) — environment: shizue is not in the local dump, so its vanilla `broom` is not listed
- `vendor/hdr/fighters/shizue/src/acmd/tilts.rs:43` shizue/game_attackhi3 `*FIGHTER_SHIZUE_GENERATE_ARTICLE_BROOM` (change_motion) — environment: shizue is not in the local dump, so its vanilla `broom` is not listed
- `vendor/hdr/fighters/shizue/src/acmd/tilts.rs:59` shizue/game_attackhi3 `*FIGHTER_SHIZUE_GENERATE_ARTICLE_BROOM` (remove) — environment: shizue is not in the local dump, so its vanilla `broom` is not listed
- `vendor/hdr/fighters/shizue/src/acmd/smashes.rs:67` shizue/game_attackhi4 `*FIGHTER_SHIZUE_GENERATE_ARTICLE_TRAFFICSIGN` (generate_article) — environment: shizue is not in the local dump, so its vanilla `trafficsign` is not listed
- `vendor/hdr/fighters/shizue/src/acmd/smashes.rs:68` shizue/game_attackhi4 `*FIGHTER_SHIZUE_GENERATE_ARTICLE_TRAFFICSIGN` (change_motion) — environment: shizue is not in the local dump, so its vanilla `trafficsign` is not listed
- `vendor/hdr/fighters/shizue/src/acmd/tilts.rs:79` shizue/game_attacklw3 `*FIGHTER_SHIZUE_GENERATE_ARTICLE_WEEDS` (generate_article) — environment: shizue is not in the local dump, so its vanilla `weeds` is not listed
- `vendor/hdr/fighters/shizue/src/acmd/tilts.rs:91` shizue/game_attacklw3 `*FIGHTER_SHIZUE_GENERATE_ARTICLE_WEEDS` (remove) — environment: shizue is not in the local dump, so its vanilla `weeds` is not listed
- `vendor/hdr/fighters/shizue/src/acmd/smashes.rs:103` shizue/game_attacklw4 `*FIGHTER_SHIZUE_GENERATE_ARTICLE_BUCKET` (generate_article) — environment: shizue is not in the local dump, so its vanilla `bucket` is not listed
- `vendor/hdr/fighters/shizue/src/acmd/smashes.rs:104` shizue/game_attacklw4 `*FIGHTER_SHIZUE_GENERATE_ARTICLE_BUCKET` (change_motion) — environment: shizue is not in the local dump, so its vanilla `bucket` is not listed
- `vendor/hdr/fighters/shizue/src/acmd/smashes.rs:110` shizue/game_attacklw4 `*FIGHTER_SHIZUE_GENERATE_ARTICLE_BUCKET` (change_motion) — environment: shizue is not in the local dump, so its vanilla `bucket` is not listed
- `vendor/hdr/fighters/shizue/src/acmd/smashes.rs:115` shizue/game_attacklw4 `*FIGHTER_SHIZUE_GENERATE_ARTICLE_BUCKET` (change_motion) — environment: shizue is not in the local dump, so its vanilla `bucket` is not listed
- `vendor/hdr/fighters/shizue/src/acmd/smashes.rs:139` shizue/game_attacklw4 `*FIGHTER_SHIZUE_GENERATE_ARTICLE_BUCKET` (remove) — environment: shizue is not in the local dump, so its vanilla `bucket` is not listed
- `vendor/hdr/fighters/shizue/src/acmd/tilts.rs:7` shizue/game_attacks3 `*FIGHTER_SHIZUE_GENERATE_ARTICLE_UMBRELLA` (generate_article) — environment: shizue is not in the local dump, so its vanilla `umbrella` is not listed
- `vendor/hdr/fighters/shizue/src/acmd/tilts.rs:23` shizue/game_attacks3 `*FIGHTER_SHIZUE_GENERATE_ARTICLE_UMBRELLA` (remove) — environment: shizue is not in the local dump, so its vanilla `umbrella` is not listed
- `vendor/hdr/fighters/shizue/src/acmd/smashes.rs:7` shizue/game_attacks4 `*FIGHTER_SHIZUE_GENERATE_ARTICLE_CRACKER` (generate_article) — environment: shizue is not in the local dump, so its vanilla `cracker` is not listed
- `vendor/hdr/fighters/shizue/src/acmd/smashes.rs:8` shizue/game_attacks4 `*FIGHTER_SHIZUE_GENERATE_ARTICLE_CRACKER` (change_motion) — environment: shizue is not in the local dump, so its vanilla `cracker` is not listed
- `vendor/hdr/fighters/shizue/src/acmd/smashes.rs:14` shizue/game_attacks4 `*FIGHTER_SHIZUE_GENERATE_ARTICLE_CRACKER` (change_motion) — environment: shizue is not in the local dump, so its vanilla `cracker` is not listed
- `vendor/hdr/fighters/shizue/src/acmd/smashes.rs:19` shizue/game_attacks4 `*FIGHTER_SHIZUE_GENERATE_ARTICLE_CRACKER` (change_motion) — environment: shizue is not in the local dump, so its vanilla `cracker` is not listed
- `vendor/hdr/fighters/shizue/src/acmd/smashes.rs:37` shizue/game_attacks4 `*FIGHTER_SHIZUE_GENERATE_ARTICLE_CRACKER` (remove) — environment: shizue is not in the local dump, so its vanilla `cracker` is not listed
- `vendor/hdr/fighters/shizue/src/acmd/throws.rs:25` shizue/game_catch `*FIGHTER_MURABITO_GENERATE_ARTICLE_BUTTERFLYNET` (remove_exist) — another fighter's id (murabito's butterflynet)
- `vendor/hdr/fighters/shizue/src/acmd/specials.rs:114` shizue/game_specialairsstart `*FIGHTER_SHIZUE_GENERATE_ARTICLE_FISHINGROD` (generate_article) — environment: shizue is not in the local dump, so its vanilla `fishingrod` is not listed
- `vendor/hdr/fighters/shizue/src/acmd/specials.rs:61` shizue/game_specialsstart `*FIGHTER_SHIZUE_GENERATE_ARTICLE_FISHINGROD` (generate_article) — environment: shizue is not in the local dump, so its vanilla `fishingrod` is not listed
- `vendor/hdr/fighters/simon/src/acmd/specials.rs:8` simon/game_specialairn `*FIGHTER_SIMON_GENERATE_ARTICLE_AXE` (generate_article) — environment: simon is not in the local dump, so its vanilla `axe` is not listed
- `vendor/hdr/fighters/simon/src/acmd/specials.rs:12` simon/game_specialairn `*FIGHTER_SIMON_GENERATE_ARTICLE_AXE` (shoot) — environment: simon is not in the local dump, so its vanilla `axe` is not listed
- `vendor/hdr/fighters/simon/src/acmd/specials.rs:22` simon/game_specialairs1 `*FIGHTER_SIMON_GENERATE_ARTICLE_CROSS` (generate_article) — environment: simon is not in the local dump, so its vanilla `cross` is not listed
- `vendor/hdr/fighters/simon/src/acmd/specials.rs:31` simon/game_specialairs1 `*FIGHTER_SIMON_GENERATE_ARTICLE_CROSS` (shoot) — environment: simon is not in the local dump, so its vanilla `cross` is not listed
- `vendor/hdr/fighters/simon/src/acmd/specials.rs:8` simon/game_specialn `*FIGHTER_SIMON_GENERATE_ARTICLE_AXE` (generate_article) — environment: simon is not in the local dump, so its vanilla `axe` is not listed
- `vendor/hdr/fighters/simon/src/acmd/specials.rs:12` simon/game_specialn `*FIGHTER_SIMON_GENERATE_ARTICLE_AXE` (shoot) — environment: simon is not in the local dump, so its vanilla `axe` is not listed
- `vendor/hdr/fighters/simon/src/acmd/specials.rs:22` simon/game_specials1 `*FIGHTER_SIMON_GENERATE_ARTICLE_CROSS` (generate_article) — environment: simon is not in the local dump, so its vanilla `cross` is not listed
- `vendor/hdr/fighters/simon/src/acmd/specials.rs:31` simon/game_specials1 `*FIGHTER_SIMON_GENERATE_ARTICLE_CROSS` (shoot) — environment: simon is not in the local dump, so its vanilla `cross` is not listed
- `vendor/hdr/fighters/snake/src/acmd/other.rs:111` snake/game_appealendexplode `*FIGHTER_SNAKE_GENERATE_ARTICLE_CBOX` (shoot) — environment: snake is not in the local dump, so its vanilla `cbox` is not listed
- `vendor/hdr/fighters/snake/src/acmd/other.rs:115` snake/game_appealendexplode `*FIGHTER_SNAKE_GENERATE_ARTICLE_C4_SWITCH` (generate_article) — environment: snake is not in the local dump, so its vanilla `c4_switch` is not listed
- `vendor/hdr/fighters/snake/src/acmd/other.rs:122` snake/game_appealendexplode `*FIGHTER_SNAKE_GENERATE_ARTICLE_C4` (change_status) — environment: snake is not in the local dump, so its vanilla `c4` is not listed
- `vendor/hdr/fighters/snake/src/acmd/other.rs:126` snake/game_appealendexplode `*FIGHTER_SNAKE_GENERATE_ARTICLE_C4_SWITCH` (remove_exist) — environment: snake is not in the local dump, so its vanilla `c4_switch` is not listed
- `vendor/hdr/fighters/snake/src/acmd/smashes.rs:25` snake/game_attacks4 `*FIGHTER_SNAKE_GENERATE_ARTICLE_RPG7` (generate_article) — environment: snake is not in the local dump, so its vanilla `rpg7` is not listed
- `vendor/hdr/fighters/snake/src/acmd/smashes.rs:56` snake/game_attacks4 `*FIGHTER_SNAKE_GENERATE_ARTICLE_RPG7` (remove_exist) — environment: snake is not in the local dump, so its vanilla `rpg7` is not listed
- `vendor/hdr/fighters/snake/src/acmd/smashes.rs:149` snake/game_attacks4s2 `*FIGHTER_SNAKE_GENERATE_ARTICLE_RPG7` (remove_exist) — environment: snake is not in the local dump, so its vanilla `rpg7` is not listed
- `vendor/hdr/fighters/snake/src/acmd/smashes.rs:225` snake/game_attacks4s3 `*FIGHTER_SNAKE_GENERATE_ARTICLE_RPG7` (remove_exist) — environment: snake is not in the local dump, so its vanilla `rpg7` is not listed
- `vendor/hdr/fighters/snake/src/acmd/specials.rs:197` snake/game_specialairhistart `*FIGHTER_SNAKE_GENERATE_ARTICLE_CYPHER` (generate_article) — environment: snake is not in the local dump, so its vanilla `cypher` is not listed
- `vendor/hdr/fighters/snake/src/acmd/specials.rs:212` snake/game_specialairlwblast `*FIGHTER_SNAKE_GENERATE_ARTICLE_C4_SWITCH` (generate_article) — environment: snake is not in the local dump, so its vanilla `c4_switch` is not listed
- `vendor/hdr/fighters/snake/src/acmd/specials.rs:281` snake/game_specialairlwselfstick `*FIGHTER_SNAKE_GENERATE_ARTICLE_C4` (change_status) — environment: snake is not in the local dump, so its vanilla `c4` is not listed
- `vendor/hdr/fighters/snake/src/acmd/specials.rs:12` snake/game_specialairnstart `*FIGHTER_SNAKE_GENERATE_ARTICLE_GRENADE` (generate_article) — environment: snake is not in the local dump, so its vanilla `grenade` is not listed
- `vendor/hdr/fighters/snake/src/acmd/specials.rs:13` snake/game_specialairnstart `*FIGHTER_SNAKE_GENERATE_ARTICLE_GRENADE_PIN` (generate_article) — environment: snake is not in the local dump, so its vanilla `grenade_pin` is not listed
- `vendor/hdr/fighters/snake/src/acmd/specials.rs:15` snake/game_specialairnstart `*FIGHTER_SNAKE_GENERATE_ARTICLE_GRENADE_PIN` (set_visibility_whole) — environment: snake is not in the local dump, so its vanilla `grenade_pin` is not listed
- `vendor/hdr/fighters/snake/src/acmd/specials.rs:25` snake/game_specialairnstart `*FIGHTER_SNAKE_GENERATE_ARTICLE_GRENADE_PIN` (set_visibility_whole) — environment: snake is not in the local dump, so its vanilla `grenade_pin` is not listed
- `vendor/hdr/fighters/snake/src/acmd/specials.rs:51` snake/game_specialairsstart `*FIGHTER_SNAKE_GENERATE_ARTICLE_NIKITA` (generate_article) — environment: snake is not in the local dump, so its vanilla `nikita` is not listed
- `vendor/hdr/fighters/snake/src/acmd/specials.rs:67` snake/game_specialairsstart `*FIGHTER_SNAKE_GENERATE_ARTICLE_NIKITA` (set_flag) — environment: snake is not in the local dump, so its vanilla `nikita` is not listed
- `vendor/hdr/fighters/snake/src/acmd/specials.rs:98` snake/game_specialairsstart `*FIGHTER_SNAKE_GENERATE_ARTICLE_NIKITA` (remove_exist) — environment: snake is not in the local dump, so its vanilla `nikita` is not listed
- `vendor/hdr/fighters/snake/src/acmd/specials.rs:197` snake/game_specialhistart `*FIGHTER_SNAKE_GENERATE_ARTICLE_CYPHER` (generate_article) — environment: snake is not in the local dump, so its vanilla `cypher` is not listed
- `vendor/hdr/fighters/snake/src/acmd/specials.rs:212` snake/game_speciallwblast `*FIGHTER_SNAKE_GENERATE_ARTICLE_C4_SWITCH` (generate_article) — environment: snake is not in the local dump, so its vanilla `c4_switch` is not listed
- `vendor/hdr/fighters/snake/src/acmd/specials.rs:281` snake/game_speciallwselfstick `*FIGHTER_SNAKE_GENERATE_ARTICLE_C4` (change_status) — environment: snake is not in the local dump, so its vanilla `c4` is not listed
- `vendor/hdr/fighters/snake/src/acmd/specials.rs:239` snake/game_speciallwsquatblast `*FIGHTER_SNAKE_GENERATE_ARTICLE_C4_SWITCH` (generate_article) — environment: snake is not in the local dump, so its vanilla `c4_switch` is not listed
- `vendor/hdr/fighters/snake/src/acmd/specials.rs:290` snake/game_speciallwsquatselfstick `*FIGHTER_SNAKE_GENERATE_ARTICLE_C4` (change_status) — environment: snake is not in the local dump, so its vanilla `c4` is not listed
- `vendor/hdr/fighters/snake/src/acmd/specials.rs:12` snake/game_specialnstart `*FIGHTER_SNAKE_GENERATE_ARTICLE_GRENADE` (generate_article) — environment: snake is not in the local dump, so its vanilla `grenade` is not listed
- `vendor/hdr/fighters/snake/src/acmd/specials.rs:13` snake/game_specialnstart `*FIGHTER_SNAKE_GENERATE_ARTICLE_GRENADE_PIN` (generate_article) — environment: snake is not in the local dump, so its vanilla `grenade_pin` is not listed
- `vendor/hdr/fighters/snake/src/acmd/specials.rs:15` snake/game_specialnstart `*FIGHTER_SNAKE_GENERATE_ARTICLE_GRENADE_PIN` (set_visibility_whole) — environment: snake is not in the local dump, so its vanilla `grenade_pin` is not listed
- `vendor/hdr/fighters/snake/src/acmd/specials.rs:25` snake/game_specialnstart `*FIGHTER_SNAKE_GENERATE_ARTICLE_GRENADE_PIN` (set_visibility_whole) — environment: snake is not in the local dump, so its vanilla `grenade_pin` is not listed
- `vendor/hdr/fighters/snake/src/acmd/specials.rs:51` snake/game_specialsstart `*FIGHTER_SNAKE_GENERATE_ARTICLE_NIKITA` (generate_article) — environment: snake is not in the local dump, so its vanilla `nikita` is not listed
- `vendor/hdr/fighters/snake/src/acmd/specials.rs:67` snake/game_specialsstart `*FIGHTER_SNAKE_GENERATE_ARTICLE_NIKITA` (set_flag) — environment: snake is not in the local dump, so its vanilla `nikita` is not listed
- `vendor/hdr/fighters/snake/src/acmd/specials.rs:98` snake/game_specialsstart `*FIGHTER_SNAKE_GENERATE_ARTICLE_NIKITA` (remove_exist) — environment: snake is not in the local dump, so its vanilla `nikita` is not listed
- `vendor/hdr/fighters/szerosuit/src/acmd/aerials.rs:277` szerosuit/game_aircatch `*FIGHTER_SZEROSUIT_GENERATE_ARTICLE_WHIP2` (generate_article) — environment: szerosuit is not in the local dump, so its vanilla `whip2` is not listed
- `vendor/hdr/fighters/szerosuit/src/acmd/aerials.rs:278` szerosuit/game_aircatch `*FIGHTER_SZEROSUIT_GENERATE_ARTICLE_WHIP2` (set_visibility_whole) — environment: szerosuit is not in the local dump, so its vanilla `whip2` is not listed
- `vendor/hdr/fighters/szerosuit/src/acmd/aerials.rs:287` szerosuit/game_aircatch `*FIGHTER_SZEROSUIT_GENERATE_ARTICLE_WHIP2` (set_visibility_whole) — environment: szerosuit is not in the local dump, so its vanilla `whip2` is not listed
- `vendor/hdr/fighters/szerosuit/src/acmd/aerials.rs:288` szerosuit/game_aircatch `*FIGHTER_SZEROSUIT_GENERATE_ARTICLE_WHIP2` (change_status) — environment: szerosuit is not in the local dump, so its vanilla `whip2` is not listed
- `vendor/hdr/fighters/szerosuit/src/acmd/aerials.rs:301` szerosuit/game_aircatch `*FIGHTER_SZEROSUIT_GENERATE_ARTICLE_WHIP2` (change_status_exist) — environment: szerosuit is not in the local dump, so its vanilla `whip2` is not listed
- `vendor/hdr/fighters/szerosuit/src/acmd/aerials.rs:305` szerosuit/game_aircatch `*FIGHTER_SZEROSUIT_GENERATE_ARTICLE_WHIP2` (remove_exist) — environment: szerosuit is not in the local dump, so its vanilla `whip2` is not listed
- `vendor/hdr/fighters/szerosuit/src/acmd/aerials.rs:8` szerosuit/game_attackairn `*FIGHTER_SZEROSUIT_GENERATE_ARTICLE_WHIP` (generate_article) — environment: szerosuit is not in the local dump, so its vanilla `whip` is not listed
- `vendor/hdr/fighters/szerosuit/src/acmd/aerials.rs:9` szerosuit/game_attackairn `*FIGHTER_SZEROSUIT_GENERATE_ARTICLE_WHIP` (change_motion) — environment: szerosuit is not in the local dump, so its vanilla `whip` is not listed
- `vendor/hdr/fighters/szerosuit/src/acmd/aerials.rs:52` szerosuit/game_attackairn `*FIGHTER_SZEROSUIT_GENERATE_ARTICLE_WHIP` (remove_exist) — environment: szerosuit is not in the local dump, so its vanilla `whip` is not listed
- `vendor/hdr/fighters/szerosuit/src/acmd/smashes.rs:78` szerosuit/game_attackhi4 `*FIGHTER_SZEROSUIT_GENERATE_ARTICLE_WHIP` (generate_article) — environment: szerosuit is not in the local dump, so its vanilla `whip` is not listed
- `vendor/hdr/fighters/szerosuit/src/acmd/smashes.rs:79` szerosuit/game_attackhi4 `*FIGHTER_SZEROSUIT_GENERATE_ARTICLE_WHIP` (change_motion) — environment: szerosuit is not in the local dump, so its vanilla `whip` is not listed
- `vendor/hdr/fighters/szerosuit/src/acmd/smashes.rs:117` szerosuit/game_attackhi4 `*FIGHTER_SZEROSUIT_GENERATE_ARTICLE_WHIP` (remove_exist) — environment: szerosuit is not in the local dump, so its vanilla `whip` is not listed
- `vendor/hdr/fighters/szerosuit/src/acmd/throws.rs:54` szerosuit/game_catchdash `*FIGHTER_SZEROSUIT_GENERATE_ARTICLE_WHIP` (generate_article) — environment: szerosuit is not in the local dump, so its vanilla `whip` is not listed
- `vendor/hdr/fighters/szerosuit/src/acmd/throws.rs:55` szerosuit/game_catchdash `*FIGHTER_SZEROSUIT_GENERATE_ARTICLE_WHIP` (change_motion) — environment: szerosuit is not in the local dump, so its vanilla `whip` is not listed
- `vendor/hdr/fighters/szerosuit/src/acmd/throws.rs:77` szerosuit/game_catchdash `*FIGHTER_SZEROSUIT_GENERATE_ARTICLE_WHIP` (remove_exist) — environment: szerosuit is not in the local dump, so its vanilla `whip` is not listed
- `vendor/hdr/fighters/szerosuit/src/acmd/throws.rs:85` szerosuit/game_catchturn `*FIGHTER_SZEROSUIT_GENERATE_ARTICLE_WHIP` (generate_article) — environment: szerosuit is not in the local dump, so its vanilla `whip` is not listed
- `vendor/hdr/fighters/szerosuit/src/acmd/throws.rs:86` szerosuit/game_catchturn `*FIGHTER_SZEROSUIT_GENERATE_ARTICLE_WHIP` (change_motion) — environment: szerosuit is not in the local dump, so its vanilla `whip` is not listed
- `vendor/hdr/fighters/szerosuit/src/acmd/throws.rs:108` szerosuit/game_catchturn `*FIGHTER_SZEROSUIT_GENERATE_ARTICLE_WHIP` (remove_exist) — environment: szerosuit is not in the local dump, so its vanilla `whip` is not listed
- `vendor/hdr/fighters/szerosuit/src/acmd/specials.rs:17` szerosuit/game_specialairnshoot `*FIGHTER_SZEROSUIT_GENERATE_ARTICLE_PARALYZER_BULLET` (generate_article) — environment: szerosuit is not in the local dump, so its vanilla `paralyzer_bullet` is not listed
- `vendor/hdr/fighters/szerosuit/src/acmd/specials.rs:17` szerosuit/game_specialnshoot `*FIGHTER_SZEROSUIT_GENERATE_ARTICLE_PARALYZER_BULLET` (generate_article) — environment: szerosuit is not in the local dump, so its vanilla `paralyzer_bullet` is not listed
- `vendor/hdr/fighters/trail/src/acmd/specials.rs:68` trail/game_specialairn2 `*FIGHTER_TRAIL_GENERATE_ARTICLE_FLOWER` (remove_exist) — environment: trail is not in the local dump, so its vanilla `flower` is not listed
- `vendor/hdr/fighters/trail/src/acmd/specials.rs:69` trail/game_specialairn2 `*FIGHTER_TRAIL_GENERATE_ARTICLE_FLOWER` (generate_article) — environment: trail is not in the local dump, so its vanilla `flower` is not listed
- `vendor/hdr/fighters/trail/src/acmd/specials.rs:70` trail/game_specialairn2 `*FIGHTER_TRAIL_GENERATE_ARTICLE_FLOWER` (change_motion) — environment: trail is not in the local dump, so its vanilla `flower` is not listed
- `vendor/hdr/fighters/trail/src/acmd/specials.rs:40` trail/game_specialairn3 `*FIGHTER_TRAIL_GENERATE_ARTICLE_CLOUD` (generate_article) — environment: trail is not in the local dump, so its vanilla `cloud` is not listed
- `vendor/hdr/fighters/trail/src/acmd/specials.rs:40` trail/game_specialairn3 `*FIGHTER_TRAIL_GENERATE_ARTICLE_CLOUD` (generate_article) — environment: trail is not in the local dump, so its vanilla `cloud` is not listed
- `vendor/hdr/fighters/trail/src/acmd/specials.rs:40` trail/game_specialairn3 `*FIGHTER_TRAIL_GENERATE_ARTICLE_CLOUD` (generate_article) — environment: trail is not in the local dump, so its vanilla `cloud` is not listed
- `vendor/hdr/fighters/trail/src/acmd/specials.rs:68` trail/game_specialn2 `*FIGHTER_TRAIL_GENERATE_ARTICLE_FLOWER` (remove_exist) — environment: trail is not in the local dump, so its vanilla `flower` is not listed
- `vendor/hdr/fighters/trail/src/acmd/specials.rs:69` trail/game_specialn2 `*FIGHTER_TRAIL_GENERATE_ARTICLE_FLOWER` (generate_article) — environment: trail is not in the local dump, so its vanilla `flower` is not listed
- `vendor/hdr/fighters/trail/src/acmd/specials.rs:70` trail/game_specialn2 `*FIGHTER_TRAIL_GENERATE_ARTICLE_FLOWER` (change_motion) — environment: trail is not in the local dump, so its vanilla `flower` is not listed
- `vendor/hdr/fighters/trail/src/acmd/specials.rs:12` trail/game_specialn3 `*FIGHTER_TRAIL_GENERATE_ARTICLE_CLOUD` (generate_article) — environment: trail is not in the local dump, so its vanilla `cloud` is not listed
- `vendor/hdr/fighters/trail/src/acmd/specials.rs:12` trail/game_specialn3 `*FIGHTER_TRAIL_GENERATE_ARTICLE_CLOUD` (generate_article) — environment: trail is not in the local dump, so its vanilla `cloud` is not listed
- `vendor/hdr/fighters/trail/src/acmd/specials.rs:12` trail/game_specialn3 `*FIGHTER_TRAIL_GENERATE_ARTICLE_CLOUD` (generate_article) — environment: trail is not in the local dump, so its vanilla `cloud` is not listed
- `vendor/hdr/fighters/wiifit/src/acmd/specials.rs:40` wiifit/game_specialairhistart `*FIGHTER_WIIFIT_GENERATE_ARTICLE_HULAHOOP` (generate_article) — environment: wiifit is not in the local dump, so its vanilla `hulahoop` is not listed
- `vendor/hdr/fighters/wiifit/src/acmd/specials.rs:60` wiifit/game_specialhiend `*FIGHTER_WIIFIT_GENERATE_ARTICLE_HULAHOOP` (shoot_exist) — environment: wiifit is not in the local dump, so its vanilla `hulahoop` is not listed
- `vendor/hdr/fighters/wiifit/src/acmd/specials.rs:40` wiifit/game_specialhistart `*FIGHTER_WIIFIT_GENERATE_ARTICLE_HULAHOOP` (generate_article) — environment: wiifit is not in the local dump, so its vanilla `hulahoop` is not listed
- `vendor/hdr/fighters/wolf/src/acmd/specials.rs:8` wolf/game_specialairn `*FIGHTER_WOLF_GENERATE_ARTICLE_BLASTER` (generate_article) — environment: wolf is not in the local dump, so its vanilla `blaster` is not listed
- `vendor/hdr/fighters/wolf/src/acmd/specials.rs:11` wolf/game_specialairn `*FIGHTER_WOLF_GENERATE_ARTICLE_BLASTER` (change_motion) — environment: wolf is not in the local dump, so its vanilla `blaster` is not listed
- `vendor/hdr/fighters/wolf/src/acmd/specials.rs:21` wolf/game_specialairn `*FIGHTER_WOLF_GENERATE_ARTICLE_BLASTER_BULLET` (generate_article) — environment: wolf is not in the local dump, so its vanilla `blaster_bullet` is not listed
- `vendor/hdr/fighters/wolf/src/acmd/specials.rs:32` wolf/game_specialairn `*FIGHTER_WOLF_GENERATE_ARTICLE_BLASTER` (change_motion) — environment: wolf is not in the local dump, so its vanilla `blaster` is not listed
- `vendor/hdr/fighters/wolf/src/acmd/specials.rs:36` wolf/game_specialairn `*FIGHTER_WOLF_GENERATE_ARTICLE_BLASTER` (set_visibility_whole) — environment: wolf is not in the local dump, so its vanilla `blaster` is not listed
- `vendor/hdr/fighters/wolf/src/acmd/specials.rs:104` wolf/game_specialairs `*FIGHTER_WOLF_GENERATE_ARTICLE_ILLUSION` (generate_article) — environment: wolf is not in the local dump, so its vanilla `illusion` is not listed
- `vendor/hdr/fighters/wolf/src/acmd/specials.rs:8` wolf/game_specialn `*FIGHTER_WOLF_GENERATE_ARTICLE_BLASTER` (generate_article) — environment: wolf is not in the local dump, so its vanilla `blaster` is not listed
- `vendor/hdr/fighters/wolf/src/acmd/specials.rs:11` wolf/game_specialn `*FIGHTER_WOLF_GENERATE_ARTICLE_BLASTER` (change_motion) — environment: wolf is not in the local dump, so its vanilla `blaster` is not listed
- `vendor/hdr/fighters/wolf/src/acmd/specials.rs:21` wolf/game_specialn `*FIGHTER_WOLF_GENERATE_ARTICLE_BLASTER_BULLET` (generate_article) — environment: wolf is not in the local dump, so its vanilla `blaster_bullet` is not listed
- `vendor/hdr/fighters/wolf/src/acmd/specials.rs:32` wolf/game_specialn `*FIGHTER_WOLF_GENERATE_ARTICLE_BLASTER` (change_motion) — environment: wolf is not in the local dump, so its vanilla `blaster` is not listed
- `vendor/hdr/fighters/wolf/src/acmd/specials.rs:36` wolf/game_specialn `*FIGHTER_WOLF_GENERATE_ARTICLE_BLASTER` (set_visibility_whole) — environment: wolf is not in the local dump, so its vanilla `blaster` is not listed
- `vendor/hdr/fighters/wolf/src/acmd/specials.rs:104` wolf/game_specials `*FIGHTER_WOLF_GENERATE_ARTICLE_ILLUSION` (generate_article) — environment: wolf is not in the local dump, so its vanilla `illusion` is not listed
- `vendor/hdr/fighters/zelda/src/acmd/specials.rs:346` zelda/game_specialairlwattack `*FIGHTER_ZELDA_GENERATE_ARTICLE_PHANTOM` (shoot_exist) — environment: zelda is not in the local dump, so its vanilla `phantom` is not listed
- `vendor/hdr/fighters/zelda/src/acmd/specials.rs:346` zelda/game_speciallwattack `*FIGHTER_ZELDA_GENERATE_ARTICLE_PHANTOM` (shoot_exist) — environment: zelda is not in the local dump, so its vanilla `phantom` is not listed

### Unresolved ids (status code) (167)

- `ports/fighters/bayonetta/src/status/batwithin.rs:17` bayonetta status `*FIGHTER_BAYONETTA_GENERATE_ARTICLE_BAT` (remove_exist) — environment: bayonetta is not in the local dump, so its vanilla `bat` is not listed
- `vendor/hdr/fighters/bayonetta/src/status/batwithin.rs:17` bayonetta status `*FIGHTER_BAYONETTA_GENERATE_ARTICLE_BAT` (remove_exist) — environment: bayonetta is not in the local dump, so its vanilla `bat` is not listed
- `ports/fighters/buddy/src/status/special_n.rs:52` buddy status `*FIGHTER_BUDDY_GENERATE_ARTICLE_PARTNER` (is_exist) — environment: buddy is not in the local dump, so its vanilla `partner` is not listed
- `ports/fighters/buddy/src/status/special_n.rs:53` buddy status `*FIGHTER_BUDDY_GENERATE_ARTICLE_PARTNER` (generate_article) — environment: buddy is not in the local dump, so its vanilla `partner` is not listed
- `ports/fighters/buddy/src/status/special_n.rs:55` buddy status `*FIGHTER_BUDDY_GENERATE_ARTICLE_PARTNER` (change_motion) — environment: buddy is not in the local dump, so its vanilla `partner` is not listed
- `vendor/hdr/fighters/buddy/src/status/special_n.rs:52` buddy status `*FIGHTER_BUDDY_GENERATE_ARTICLE_PARTNER` (is_exist) — environment: buddy is not in the local dump, so its vanilla `partner` is not listed
- `vendor/hdr/fighters/buddy/src/status/special_n.rs:53` buddy status `*FIGHTER_BUDDY_GENERATE_ARTICLE_PARTNER` (generate_article) — environment: buddy is not in the local dump, so its vanilla `partner` is not listed
- `vendor/hdr/fighters/buddy/src/status/special_n.rs:55` buddy status `*FIGHTER_BUDDY_GENERATE_ARTICLE_PARTNER` (change_motion) — environment: buddy is not in the local dump, so its vanilla `partner` is not listed
- `ports/fighters/daisy/src/status/guard_damage.rs:5` daisy status `*FIGHTER_DAISY_GENERATE_ARTICLE_KINOPIO` (is_exist) — environment: daisy is not in the local dump, so its vanilla `kinopio` is not listed
- `ports/fighters/daisy/src/status/guard_damage.rs:6` daisy status `*FIGHTER_DAISY_GENERATE_ARTICLE_KINOPIO` (generate_article) — environment: daisy is not in the local dump, so its vanilla `kinopio` is not listed
- `ports/fighters/daisy/src/status/guard_damage.rs:7` daisy status `*FIGHTER_DAISY_GENERATE_ARTICLE_KINOPIO` (change_status) — environment: daisy is not in the local dump, so its vanilla `kinopio` is not listed
- `ports/fighters/daisy/src/status/guard_damage.rs:8` daisy status `*FIGHTER_DAISY_GENERATE_ARTICLE_KINOPIO` (get_article) — environment: daisy is not in the local dump, so its vanilla `kinopio` is not listed
- `ports/fighters/daisy/src/status/special_hi.rs:62` daisy status `*FIGHTER_DAISY_GENERATE_ARTICLE_KASSAR` (remove_exist) — environment: daisy is not in the local dump, so its vanilla `kassar` is not listed
- `ports/fighters/daisy/src/status/special_n.rs:190` daisy status `*FIGHTER_DAISY_GENERATE_ARTICLE_KASSAR` (remove_exist) — environment: daisy is not in the local dump, so its vanilla `kassar` is not listed
- `vendor/hdr/fighters/daisy/src/status/guard_damage.rs:5` daisy status `*FIGHTER_DAISY_GENERATE_ARTICLE_KINOPIO` (is_exist) — environment: daisy is not in the local dump, so its vanilla `kinopio` is not listed
- `vendor/hdr/fighters/daisy/src/status/guard_damage.rs:6` daisy status `*FIGHTER_DAISY_GENERATE_ARTICLE_KINOPIO` (generate_article) — environment: daisy is not in the local dump, so its vanilla `kinopio` is not listed
- `vendor/hdr/fighters/daisy/src/status/guard_damage.rs:7` daisy status `*FIGHTER_DAISY_GENERATE_ARTICLE_KINOPIO` (change_status) — environment: daisy is not in the local dump, so its vanilla `kinopio` is not listed
- `vendor/hdr/fighters/daisy/src/status/guard_damage.rs:8` daisy status `*FIGHTER_DAISY_GENERATE_ARTICLE_KINOPIO` (get_article) — environment: daisy is not in the local dump, so its vanilla `kinopio` is not listed
- `vendor/hdr/fighters/daisy/src/status/special_hi.rs:62` daisy status `*FIGHTER_DAISY_GENERATE_ARTICLE_KASSAR` (remove_exist) — environment: daisy is not in the local dump, so its vanilla `kassar` is not listed
- `vendor/hdr/fighters/daisy/src/status/special_n.rs:190` daisy status `*FIGHTER_DAISY_GENERATE_ARTICLE_KASSAR` (remove_exist) — environment: daisy is not in the local dump, so its vanilla `kassar` is not listed
- `vendor/hdr/fighters/duckhunt/src/status/special_n.rs:16` duckhunt status `*FIGHTER_DUCKHUNT_GENERATE_ARTICLE_RETICLE` (change_motion) — environment: duckhunt is not in the local dump, so its vanilla `reticle` is not listed
- `vendor/hdr/fighters/duckhunt/src/status/special_n.rs:17` duckhunt status `*FIGHTER_DUCKHUNT_GENERATE_ARTICLE_CAN` (is_exist) — environment: duckhunt is not in the local dump, so its vanilla `can` is not listed
- `vendor/hdr/fighters/duckhunt/src/status/special_s.rs:38` duckhunt status `*FIGHTER_DUCKHUNT_GENERATE_ARTICLE_RETICLE` (change_motion) — environment: duckhunt is not in the local dump, so its vanilla `reticle` is not listed
- `vendor/hdr/fighters/duckhunt/src/status/special_s.rs:39` duckhunt status `*FIGHTER_DUCKHUNT_GENERATE_ARTICLE_CLAY` (is_exist) — environment: duckhunt is not in the local dump, so its vanilla `clay` is not listed
- `vendor/hdr/fighters/elight/src/status/landing.rs:31` elight status `*FIGHTER_ELIGHT_GENERATE_ARTICLE_BUNSHIN` (remove_exist) — environment: elight is not in the local dump, so its vanilla `bunshin` is not listed
- `vendor/hdr/fighters/elight/src/status/special_s_end.rs:65` elight status `*FIGHTER_ELIGHT_GENERATE_ARTICLE_BUNSHIN` (remove_exist) — environment: elight is not in the local dump, so its vanilla `bunshin` is not listed
- `vendor/hdr/fighters/elight/src/status/special_s_forward.rs:144` elight status `*FIGHTER_ELIGHT_GENERATE_ARTICLE_BUNSHIN` (remove_exist) — environment: elight is not in the local dump, so its vanilla `bunshin` is not listed
- `vendor/hdr/fighters/gamewatch/src/status/special_hi.rs:9` gamewatch status `*FIGHTER_GAMEWATCH_GENERATE_ARTICLE_RESCUE` (generate_article) — environment: gamewatch is not in the local dump, so its vanilla `rescue` is not listed
- `vendor/hdr/fighters/gamewatch/src/status/special_hi.rs:11` gamewatch status `*FIGHTER_GAMEWATCH_GENERATE_ARTICLE_RESCUE` (change_motion) — environment: gamewatch is not in the local dump, so its vanilla `rescue` is not listed
- `vendor/hdr/fighters/gamewatch/src/status/special_hi_open.rs:34` gamewatch status `*FIGHTER_GAMEWATCH_GENERATE_ARTICLE_PARACHUTE` (generate_article) — environment: gamewatch is not in the local dump, so its vanilla `parachute` is not listed
- `vendor/hdr/fighters/gamewatch/src/status/special_hi_open.rs:35` gamewatch status `*FIGHTER_GAMEWATCH_GENERATE_ARTICLE_RESCUE` (change_motion) — environment: gamewatch is not in the local dump, so its vanilla `rescue` is not listed
- `vendor/hdr/fighters/gamewatch/src/status/special_hi_open.rs:68` gamewatch status `*FIGHTER_GAMEWATCH_GENERATE_ARTICLE_PARACHUTE` (remove_exist) — environment: gamewatch is not in the local dump, so its vanilla `parachute` is not listed
- `vendor/hdr/fighters/ike/src/status/rebirth.rs:7` ike status `*FIGHTER_IKE_GENERATE_ARTICLE_SWORD` (is_exist) — environment: ike is not in the local dump, so its vanilla `sword` is not listed
- `vendor/hdr/fighters/ike/src/status/rebirth.rs:8` ike status `*FIGHTER_IKE_GENERATE_ARTICLE_SWORD` (remove_exist) — environment: ike is not in the local dump, so its vanilla `sword` is not listed
- `vendor/hdr/fighters/ken/src/status/finals.rs:206` ken status `*FIGHTER_KEN_GENERATE_ARTICLE_SHINRYUKEN` (generate_article) — environment: ken is not in the local dump, so its vanilla `shinryuken` is not listed
- `vendor/hdr/fighters/kirby/src/status/copy/diddy_special_n_cancel.rs:115` kirby status `*FIGHTER_DIDDY_GENERATE_ARTICLE_GUN` (remove_exist) — another fighter's id (diddy's gun)
- `vendor/hdr/fighters/kirby/src/status/copy/diddy_special_n_cancel.rs:180` kirby status `*FIGHTER_DIDDY_GENERATE_ARTICLE_GUN` (remove_exist) — another fighter's id (diddy's gun)
- `vendor/hdr/fighters/kirby/src/status/copy/krool_special_n.rs:6` kirby status `*FIGHTER_KROOL_GENERATE_ARTICLE_BLUNDERBUSS` (generate_article) — another fighter's id (krool's blunderbuss)
- `vendor/hdr/fighters/kirby/src/status/copy/krool_special_n.rs:7` kirby status `*FIGHTER_KROOL_GENERATE_ARTICLE_BLUNDERBUSS` (change_status_exist) — another fighter's id (krool's blunderbuss)
- `vendor/hdr/fighters/kirby/src/status/copy/lucario_special_n.rs:105` kirby status: not a statement Visionary reads: `ArticleModule::change_status(`
- `vendor/hdr/fighters/kirby/src/status/copy/master_special_n.rs:23` kirby status `*FIGHTER_MASTER_GENERATE_ARTICLE_BOW` (is_exist) — another fighter's id (master's bow)
- `vendor/hdr/fighters/kirby/src/status/copy/master_special_n.rs:24` kirby status `*FIGHTER_MASTER_GENERATE_ARTICLE_BOW` (change_motion) — another fighter's id (master's bow)
- `vendor/hdr/fighters/kirby/src/status/copy/master_special_n.rs:132` kirby status `*FIGHTER_MASTER_GENERATE_ARTICLE_BOW` (remove_exist) — another fighter's id (master's bow)
- `vendor/hdr/fighters/kirby/src/status/copy/master_special_n.rs:133` kirby status `*FIGHTER_MASTER_GENERATE_ARTICLE_ARROW1` (remove_exist) — another fighter's id (master's arrow1)
- `vendor/hdr/fighters/kirby/src/status/copy/master_special_n.rs:134` kirby status `*FIGHTER_MASTER_GENERATE_ARTICLE_ARROW2` (remove_exist) — another fighter's id (master's arrow2)
- `vendor/hdr/fighters/kirby/src/status/copy/master_special_n.rs:142` kirby status `*FIGHTER_MASTER_GENERATE_ARTICLE_ARROW1` (is_exist) — another fighter's id (master's arrow1)
- `vendor/hdr/fighters/kirby/src/status/copy/master_special_n.rs:143` kirby status `*FIGHTER_MASTER_GENERATE_ARTICLE_ARROW1` (get_article) — another fighter's id (master's arrow1)
- `vendor/hdr/fighters/kirby/src/status/copy/master_special_n.rs:178` kirby status `*FIGHTER_MASTER_GENERATE_ARTICLE_BOW` (is_exist) — another fighter's id (master's bow)
- `vendor/hdr/fighters/kirby/src/status/copy/master_special_n.rs:179` kirby status `*FIGHTER_MASTER_GENERATE_ARTICLE_BOW` (change_motion) — another fighter's id (master's bow)
- `vendor/hdr/fighters/kirby/src/status/copy/master_special_n.rs:181` kirby status `*FIGHTER_MASTER_GENERATE_ARTICLE_BOW` (set_frame) — another fighter's id (master's bow)
- `vendor/hdr/fighters/kirby/src/status/copy/master_special_n.rs:132` kirby status `*FIGHTER_MASTER_GENERATE_ARTICLE_BOW` (remove_exist) — another fighter's id (master's bow)
- `vendor/hdr/fighters/kirby/src/status/copy/master_special_n.rs:133` kirby status `*FIGHTER_MASTER_GENERATE_ARTICLE_ARROW1` (remove_exist) — another fighter's id (master's arrow1)
- `vendor/hdr/fighters/kirby/src/status/copy/master_special_n.rs:134` kirby status `*FIGHTER_MASTER_GENERATE_ARTICLE_ARROW2` (remove_exist) — another fighter's id (master's arrow2)
- `vendor/hdr/fighters/kirby/src/status/copy/master_special_n.rs:142` kirby status `*FIGHTER_MASTER_GENERATE_ARTICLE_ARROW1` (is_exist) — another fighter's id (master's arrow1)
- `vendor/hdr/fighters/kirby/src/status/copy/master_special_n.rs:143` kirby status `*FIGHTER_MASTER_GENERATE_ARTICLE_ARROW1` (get_article) — another fighter's id (master's arrow1)
- `vendor/hdr/fighters/kirby/src/status/copy/master_special_n.rs:191` kirby status `*FIGHTER_MASTER_GENERATE_ARTICLE_BOW` (is_exist) — another fighter's id (master's bow)
- `vendor/hdr/fighters/kirby/src/status/copy/master_special_n.rs:192` kirby status `*FIGHTER_MASTER_GENERATE_ARTICLE_BOW` (change_motion) — another fighter's id (master's bow)
- `vendor/hdr/fighters/kirby/src/status/copy/master_special_n.rs:132` kirby status `*FIGHTER_MASTER_GENERATE_ARTICLE_BOW` (remove_exist) — another fighter's id (master's bow)
- `vendor/hdr/fighters/kirby/src/status/copy/master_special_n.rs:133` kirby status `*FIGHTER_MASTER_GENERATE_ARTICLE_ARROW1` (remove_exist) — another fighter's id (master's arrow1)
- `vendor/hdr/fighters/kirby/src/status/copy/master_special_n.rs:134` kirby status `*FIGHTER_MASTER_GENERATE_ARTICLE_ARROW2` (remove_exist) — another fighter's id (master's arrow2)
- `vendor/hdr/fighters/kirby/src/status/copy/master_special_n.rs:142` kirby status `*FIGHTER_MASTER_GENERATE_ARTICLE_ARROW1` (is_exist) — another fighter's id (master's arrow1)
- `vendor/hdr/fighters/kirby/src/status/copy/master_special_n.rs:143` kirby status `*FIGHTER_MASTER_GENERATE_ARTICLE_ARROW1` (get_article) — another fighter's id (master's arrow1)
- `vendor/hdr/fighters/kirby/src/status/copy/master_special_n.rs:236` kirby status `*FIGHTER_MASTER_GENERATE_ARTICLE_BOW` (is_exist) — another fighter's id (master's bow)
- `vendor/hdr/fighters/kirby/src/status/copy/master_special_n.rs:237` kirby status `*FIGHTER_MASTER_GENERATE_ARTICLE_BOW` (change_motion) — another fighter's id (master's bow)
- `vendor/hdr/fighters/kirby/src/status/copy/mewtwo_special_n.rs:107` kirby status: not a statement Visionary reads: `ArticleModule::change_status(`
- `vendor/hdr/fighters/kirby/src/status/copy/miifighter_special_n.rs:104` kirby status `*FIGHTER_MIIFIGHTER_GENERATE_ARTICLE_IRONBALL` (remove_exist) — another fighter's id (miifighter's ironball)
- `vendor/hdr/fighters/kirby/src/status/copy/palutena_special_n.rs:85` kirby status `*FIGHTER_PALUTENA_GENERATE_ARTICLE_EXPLOSIVEFLAME` (generate_article) — another fighter's id (palutena's explosiveflame)
- `vendor/hdr/fighters/kirby/src/status/copy/pikmin_special_n.rs:14` kirby status `*FIGHTER_PIKMIN_GENERATE_ARTICLE_PIKMIN` (get_active_num) — another fighter's id (pikmin's pikmin)
- `vendor/hdr/fighters/kirby/src/status/copy/richter_special_n.rs:34` kirby status `*FIGHTER_SIMON_GENERATE_ARTICLE_AXE` (is_generatable) — another fighter's id (simon's axe)
- `vendor/hdr/fighters/kirby/src/status/copy/trail_special_n.rs:186` kirby status `*FIGHTER_TRAIL_GENERATE_ARTICLE_CLOUD` (remove_exist) — another fighter's id (trail's cloud)
- `vendor/hdr/fighters/kirby/src/status/special_hi.rs:185` kirby status `*FIGHTER_KIRBY_GENERATE_ARTICLE_FINALCUTTER` (remove_exist) — environment: kirby is not in the local dump, so its vanilla `finalcutter` is not listed
- `vendor/hdr/fighters/kirby/src/status/special_lw.rs:11` kirby status `*FIGHTER_KIRBY_GENERATE_ARTICLE_STONE` (generate_article) — environment: kirby is not in the local dump, so its vanilla `stone` is not listed
- `vendor/hdr/fighters/kirby/src/status/special_lw.rs:12` kirby status `*FIGHTER_KIRBY_GENERATE_ARTICLE_STONE` (set_visibility_whole) — environment: kirby is not in the local dump, so its vanilla `stone` is not listed
- `vendor/hdr/fighters/lucario/src/status/special_n.rs:116` lucario status: not a statement Visionary reads: `ArticleModule::change_status(`
- `ports/fighters/lucas/src/status/special_hi.rs:10` lucas status `*FIGHTER_LUCAS_GENERATE_ARTICLE_PK_THUNDER` (remove_exist) — environment: lucas is not in the local dump, so its vanilla `pk_thunder` is not listed
- `vendor/hdr/fighters/lucas/src/status/special_hi.rs:10` lucas status `*FIGHTER_LUCAS_GENERATE_ARTICLE_PK_THUNDER` (remove_exist) — environment: lucas is not in the local dump, so its vanilla `pk_thunder` is not listed
- `vendor/hdr/fighters/master/src/status/rebirth.rs:6` master status `*FIGHTER_MASTER_GENERATE_ARTICLE_SWORD` (is_exist) — environment: master is not in the local dump, so its vanilla `sword` is not listed
- `vendor/hdr/fighters/master/src/status/rebirth.rs:7` master status `*FIGHTER_MASTER_GENERATE_ARTICLE_SWORD` (remove_exist) — environment: master is not in the local dump, so its vanilla `sword` is not listed
- `vendor/hdr/fighters/master/src/status/special_lw.rs:26` master status `*FIGHTER_MASTER_GENERATE_ARTICLE_AXE` (generate_article) — environment: master is not in the local dump, so its vanilla `axe` is not listed
- `vendor/hdr/fighters/master/src/status/special_lw.rs:34` master status `*FIGHTER_MASTER_GENERATE_ARTICLE_AXE` (change_motion) — environment: master is not in the local dump, so its vanilla `axe` is not listed
- `vendor/hdr/fighters/master/src/status/special_n.rs:23` master status `*FIGHTER_MASTER_GENERATE_ARTICLE_BOW` (is_exist) — environment: master is not in the local dump, so its vanilla `bow` is not listed
- `vendor/hdr/fighters/master/src/status/special_n.rs:24` master status `*FIGHTER_MASTER_GENERATE_ARTICLE_BOW` (change_motion) — environment: master is not in the local dump, so its vanilla `bow` is not listed
- `vendor/hdr/fighters/master/src/status/special_n.rs:128` master status `*FIGHTER_MASTER_GENERATE_ARTICLE_BOW` (remove_exist) — environment: master is not in the local dump, so its vanilla `bow` is not listed
- `vendor/hdr/fighters/master/src/status/special_n.rs:129` master status `*FIGHTER_MASTER_GENERATE_ARTICLE_ARROW1` (remove_exist) — environment: master is not in the local dump, so its vanilla `arrow1` is not listed
- `vendor/hdr/fighters/master/src/status/special_n.rs:130` master status `*FIGHTER_MASTER_GENERATE_ARTICLE_ARROW2` (remove_exist) — environment: master is not in the local dump, so its vanilla `arrow2` is not listed
- `vendor/hdr/fighters/master/src/status/special_n.rs:138` master status `*FIGHTER_MASTER_GENERATE_ARTICLE_ARROW1` (is_exist) — environment: master is not in the local dump, so its vanilla `arrow1` is not listed
- `vendor/hdr/fighters/master/src/status/special_n.rs:139` master status `*FIGHTER_MASTER_GENERATE_ARTICLE_ARROW1` (get_article) — environment: master is not in the local dump, so its vanilla `arrow1` is not listed
- `vendor/hdr/fighters/master/src/status/special_n.rs:170` master status `*FIGHTER_MASTER_GENERATE_ARTICLE_BOW` (is_exist) — environment: master is not in the local dump, so its vanilla `bow` is not listed
- `vendor/hdr/fighters/master/src/status/special_n.rs:171` master status `*FIGHTER_MASTER_GENERATE_ARTICLE_BOW` (change_motion) — environment: master is not in the local dump, so its vanilla `bow` is not listed
- `vendor/hdr/fighters/master/src/status/special_n.rs:173` master status `*FIGHTER_MASTER_GENERATE_ARTICLE_BOW` (set_frame) — environment: master is not in the local dump, so its vanilla `bow` is not listed
- `vendor/hdr/fighters/master/src/status/special_n.rs:128` master status `*FIGHTER_MASTER_GENERATE_ARTICLE_BOW` (remove_exist) — environment: master is not in the local dump, so its vanilla `bow` is not listed
- `vendor/hdr/fighters/master/src/status/special_n.rs:129` master status `*FIGHTER_MASTER_GENERATE_ARTICLE_ARROW1` (remove_exist) — environment: master is not in the local dump, so its vanilla `arrow1` is not listed
- `vendor/hdr/fighters/master/src/status/special_n.rs:130` master status `*FIGHTER_MASTER_GENERATE_ARTICLE_ARROW2` (remove_exist) — environment: master is not in the local dump, so its vanilla `arrow2` is not listed
- `vendor/hdr/fighters/master/src/status/special_n.rs:138` master status `*FIGHTER_MASTER_GENERATE_ARTICLE_ARROW1` (is_exist) — environment: master is not in the local dump, so its vanilla `arrow1` is not listed
- `vendor/hdr/fighters/master/src/status/special_n.rs:139` master status `*FIGHTER_MASTER_GENERATE_ARTICLE_ARROW1` (get_article) — environment: master is not in the local dump, so its vanilla `arrow1` is not listed
- `vendor/hdr/fighters/master/src/status/special_n.rs:183` master status `*FIGHTER_MASTER_GENERATE_ARTICLE_BOW` (is_exist) — environment: master is not in the local dump, so its vanilla `bow` is not listed
- `vendor/hdr/fighters/master/src/status/special_n.rs:184` master status `*FIGHTER_MASTER_GENERATE_ARTICLE_BOW` (change_motion) — environment: master is not in the local dump, so its vanilla `bow` is not listed
- `vendor/hdr/fighters/master/src/status/special_n.rs:128` master status `*FIGHTER_MASTER_GENERATE_ARTICLE_BOW` (remove_exist) — environment: master is not in the local dump, so its vanilla `bow` is not listed
- `vendor/hdr/fighters/master/src/status/special_n.rs:129` master status `*FIGHTER_MASTER_GENERATE_ARTICLE_ARROW1` (remove_exist) — environment: master is not in the local dump, so its vanilla `arrow1` is not listed
- `vendor/hdr/fighters/master/src/status/special_n.rs:130` master status `*FIGHTER_MASTER_GENERATE_ARTICLE_ARROW2` (remove_exist) — environment: master is not in the local dump, so its vanilla `arrow2` is not listed
- `vendor/hdr/fighters/master/src/status/special_n.rs:138` master status `*FIGHTER_MASTER_GENERATE_ARTICLE_ARROW1` (is_exist) — environment: master is not in the local dump, so its vanilla `arrow1` is not listed
- `vendor/hdr/fighters/master/src/status/special_n.rs:139` master status `*FIGHTER_MASTER_GENERATE_ARTICLE_ARROW1` (get_article) — environment: master is not in the local dump, so its vanilla `arrow1` is not listed
- `vendor/hdr/fighters/master/src/status/special_n.rs:226` master status `*FIGHTER_MASTER_GENERATE_ARTICLE_BOW` (is_exist) — environment: master is not in the local dump, so its vanilla `bow` is not listed
- `vendor/hdr/fighters/master/src/status/special_n.rs:227` master status `*FIGHTER_MASTER_GENERATE_ARTICLE_BOW` (change_motion) — environment: master is not in the local dump, so its vanilla `bow` is not listed
- `vendor/hdr/fighters/master/src/status/special_s.rs:36` master status `*FIGHTER_MASTER_GENERATE_ARTICLE_SPEAR` (generate_article) — environment: master is not in the local dump, so its vanilla `spear` is not listed
- `vendor/hdr/fighters/mewtwo/src/status/special_n.rs:107` mewtwo status: not a statement Visionary reads: `ArticleModule::change_status(`
- `vendor/hdr/fighters/miigunner/src/status/mod.rs:151` miigunner status `*FIGHTER_MIIGUNNER_GENERATE_ARTICLE_STEALTHBOMB` (remove_exist) — environment: miigunner is not in the local dump, so its vanilla `stealthbomb` is not listed
- `vendor/hdr/fighters/miigunner/src/status/mod.rs:156` miigunner status `*FIGHTER_MIIGUNNER_GENERATE_ARTICLE_STEALTHBOMB` (remove_exist) — environment: miigunner is not in the local dump, so its vanilla `stealthbomb` is not listed
- `vendor/hdr/fighters/miigunner/src/status/mod.rs:161` miigunner status `*FIGHTER_MIIGUNNER_GENERATE_ARTICLE_STEALTHBOMB` (remove_exist) — environment: miigunner is not in the local dump, so its vanilla `stealthbomb` is not listed
- `vendor/hdr/fighters/miigunner/src/status/special_s2.rs:90` miigunner status `*FIGHTER_MIIGUNNER_GENERATE_ARTICLE_STEALTHBOMB` (remove_exist) — environment: miigunner is not in the local dump, so its vanilla `stealthbomb` is not listed
- `vendor/hdr/fighters/ness/src/status/special_hi.rs:10` ness status `*FIGHTER_NESS_GENERATE_ARTICLE_PK_THUNDER` (remove_exist) — environment: ness is not in the local dump, so its vanilla `pk_thunder` is not listed
- `ports/fighters/pichu/src/status/special_hi.rs:35` pichu status `*FIGHTER_PIKACHU_GENERATE_ARTICLE_SPECIALUPDUMMY` (generate_article) — another fighter's id (pikachu's specialupdummy)
- `ports/fighters/pichu/src/status/special_hi.rs:36` pichu status `*FIGHTER_PIKACHU_GENERATE_ARTICLE_SPECIALUPDUMMY` (change_motion) — another fighter's id (pikachu's specialupdummy)
- `ports/fighters/pichu/src/status/special_hi.rs:37` pichu status `*FIGHTER_PIKACHU_GENERATE_ARTICLE_SPECIALUPDUMMY` (set_frame) — another fighter's id (pikachu's specialupdummy)
- `vendor/hdr/fighters/pichu/src/status/special_hi.rs:35` pichu status `*FIGHTER_PIKACHU_GENERATE_ARTICLE_SPECIALUPDUMMY` (generate_article) — another fighter's id (pikachu's specialupdummy)
- `vendor/hdr/fighters/pichu/src/status/special_hi.rs:36` pichu status `*FIGHTER_PIKACHU_GENERATE_ARTICLE_SPECIALUPDUMMY` (change_motion) — another fighter's id (pikachu's specialupdummy)
- `vendor/hdr/fighters/pichu/src/status/special_hi.rs:37` pichu status `*FIGHTER_PIKACHU_GENERATE_ARTICLE_SPECIALUPDUMMY` (set_frame) — another fighter's id (pikachu's specialupdummy)
- `ports/fighters/pickel/src/status/air_lasso.rs:74` pickel status `*FIGHTER_PICKEL_GENERATE_ARTICLE_FISHINGROD` (set_flag) — environment: pickel is not in the local dump, so its vanilla `fishingrod` is not listed
- `ports/fighters/pickel/src/status/air_lasso.rs:85` pickel status `*FIGHTER_PICKEL_GENERATE_ARTICLE_FISHINGROD` (remove_exist) — environment: pickel is not in the local dump, so its vanilla `fishingrod` is not listed
- `ports/fighters/pickel/src/status/attack_air_lw.rs:11` pickel status `*FIGHTER_PICKEL_GENERATE_ARTICLE_FORGE` (is_generatable) — environment: pickel is not in the local dump, so its vanilla `forge` is not listed
- `ports/fighters/pickel/src/status/attack_air_lw.rs:13` pickel status `*FIGHTER_PICKEL_GENERATE_ARTICLE_FORGE` (is_exist) — environment: pickel is not in the local dump, so its vanilla `forge` is not listed
- `ports/fighters/pickel/src/status/guard.rs:34` pickel status `*FIGHTER_PICKEL_GENERATE_ARTICLE_STUFF` (is_exist) — environment: pickel is not in the local dump, so its vanilla `stuff` is not listed
- `ports/fighters/pickel/src/status/guard.rs:35` pickel status `*FIGHTER_PICKEL_GENERATE_ARTICLE_STUFF` (generate_article) — environment: pickel is not in the local dump, so its vanilla `stuff` is not listed
- `ports/fighters/pickel/src/status/guard.rs:36` pickel status `*FIGHTER_PICKEL_GENERATE_ARTICLE_STUFF` (set_rate) — environment: pickel is not in the local dump, so its vanilla `stuff` is not listed
- `ports/fighters/pickel/src/status/guard.rs:71` pickel status `*FIGHTER_PICKEL_GENERATE_ARTICLE_STUFF` (is_exist) — environment: pickel is not in the local dump, so its vanilla `stuff` is not listed
- `ports/fighters/pickel/src/status/guard.rs:72` pickel status `*FIGHTER_PICKEL_GENERATE_ARTICLE_STUFF` (remove_exist) — environment: pickel is not in the local dump, so its vanilla `stuff` is not listed
- `ports/fighters/pickel/src/status/special_s.rs:13` pickel status `*FIGHTER_PICKEL_GENERATE_ARTICLE_TROLLEY` (is_exist) — environment: pickel is not in the local dump, so its vanilla `trolley` is not listed
- `vendor/hdr/fighters/pickel/src/status/air_lasso.rs:74` pickel status `*FIGHTER_PICKEL_GENERATE_ARTICLE_FISHINGROD` (set_flag) — environment: pickel is not in the local dump, so its vanilla `fishingrod` is not listed
- `vendor/hdr/fighters/pickel/src/status/air_lasso.rs:85` pickel status `*FIGHTER_PICKEL_GENERATE_ARTICLE_FISHINGROD` (remove_exist) — environment: pickel is not in the local dump, so its vanilla `fishingrod` is not listed
- `vendor/hdr/fighters/pickel/src/status/attack_air_lw.rs:11` pickel status `*FIGHTER_PICKEL_GENERATE_ARTICLE_FORGE` (is_generatable) — environment: pickel is not in the local dump, so its vanilla `forge` is not listed
- `vendor/hdr/fighters/pickel/src/status/attack_air_lw.rs:13` pickel status `*FIGHTER_PICKEL_GENERATE_ARTICLE_FORGE` (is_exist) — environment: pickel is not in the local dump, so its vanilla `forge` is not listed
- `vendor/hdr/fighters/pickel/src/status/guard.rs:34` pickel status `*FIGHTER_PICKEL_GENERATE_ARTICLE_STUFF` (is_exist) — environment: pickel is not in the local dump, so its vanilla `stuff` is not listed
- `vendor/hdr/fighters/pickel/src/status/guard.rs:35` pickel status `*FIGHTER_PICKEL_GENERATE_ARTICLE_STUFF` (generate_article) — environment: pickel is not in the local dump, so its vanilla `stuff` is not listed
- `vendor/hdr/fighters/pickel/src/status/guard.rs:36` pickel status `*FIGHTER_PICKEL_GENERATE_ARTICLE_STUFF` (set_rate) — environment: pickel is not in the local dump, so its vanilla `stuff` is not listed
- `vendor/hdr/fighters/pickel/src/status/guard.rs:71` pickel status `*FIGHTER_PICKEL_GENERATE_ARTICLE_STUFF` (is_exist) — environment: pickel is not in the local dump, so its vanilla `stuff` is not listed
- `vendor/hdr/fighters/pickel/src/status/guard.rs:72` pickel status `*FIGHTER_PICKEL_GENERATE_ARTICLE_STUFF` (remove_exist) — environment: pickel is not in the local dump, so its vanilla `stuff` is not listed
- `vendor/hdr/fighters/pickel/src/status/special_s.rs:13` pickel status `*FIGHTER_PICKEL_GENERATE_ARTICLE_TROLLEY` (is_exist) — environment: pickel is not in the local dump, so its vanilla `trolley` is not listed
- `vendor/hdr/fighters/pikmin/src/status/rebirth.rs:6` pikmin status `*FIGHTER_PIKMIN_GENERATE_ARTICLE_PIKMIN` (generate_article) — environment: pikmin is not in the local dump, so its vanilla `pikmin` is not listed
- `vendor/hdr/fighters/pikmin/src/status/rebirth.rs:7` pikmin status `*FIGHTER_PIKMIN_GENERATE_ARTICLE_PIKMIN` (change_status) — environment: pikmin is not in the local dump, so its vanilla `pikmin` is not listed
- `vendor/hdr/fighters/pikmin/src/status/special_hi.rs:5` pikmin status `*FIGHTER_PIKMIN_GENERATE_ARTICLE_PIKMIN` (set_flag) — environment: pikmin is not in the local dump, so its vanilla `pikmin` is not listed
- `vendor/hdr/fighters/pikmin/src/status/special_hi.rs:6` pikmin status `*FIGHTER_PIKMIN_GENERATE_ARTICLE_PIKMIN` (set_int) — environment: pikmin is not in the local dump, so its vanilla `pikmin` is not listed
- `vendor/hdr/fighters/pikmin/src/status/special_hi.rs:5` pikmin status `*FIGHTER_PIKMIN_GENERATE_ARTICLE_PIKMIN` (set_flag) — environment: pikmin is not in the local dump, so its vanilla `pikmin` is not listed
- `vendor/hdr/fighters/pikmin/src/status/special_hi.rs:6` pikmin status `*FIGHTER_PIKMIN_GENERATE_ARTICLE_PIKMIN` (set_int) — environment: pikmin is not in the local dump, so its vanilla `pikmin` is not listed
- `vendor/hdr/fighters/pikmin/src/status/special_hi.rs:5` pikmin status `*FIGHTER_PIKMIN_GENERATE_ARTICLE_PIKMIN` (set_flag) — environment: pikmin is not in the local dump, so its vanilla `pikmin` is not listed
- `vendor/hdr/fighters/pikmin/src/status/special_hi.rs:6` pikmin status `*FIGHTER_PIKMIN_GENERATE_ARTICLE_PIKMIN` (set_int) — environment: pikmin is not in the local dump, so its vanilla `pikmin` is not listed
- `vendor/hdr/fighters/plizardon/src/status/attack_s4.rs:8` plizardon status `articles::plizardon::ROCK` (remove_exist) — an expression Visionary does not follow
- `vendor/hdr/fighters/plizardon/src/status/attack_s4.rs:18` plizardon status `articles::plizardon::ROCK` (remove_exist) — an expression Visionary does not follow
- `vendor/hdr/fighters/plizardon/src/status/attack_s4.rs:30` plizardon status `articles::plizardon::ROCK` (remove_exist) — an expression Visionary does not follow
- `vendor/hdr/fighters/richter/src/status/special_n.rs:34` richter status `*FIGHTER_SIMON_GENERATE_ARTICLE_AXE` (is_generatable) — another fighter's id (simon's axe)
- `ports/fighters/samusd/src/status/attack_air.rs:22` samusd status `*FIGHTER_SAMUS_GENERATE_ARTICLE_GUN` (remove_exist) — another fighter's id (samus's gun)
- `vendor/hdr/fighters/samusd/src/status/attack_air.rs:22` samusd status `*FIGHTER_SAMUS_GENERATE_ARTICLE_GUN` (remove_exist) — another fighter's id (samus's gun)
- `vendor/hdr/fighters/sheik/src/status/special_hi.rs:110` sheik status `*FIGHTER_SHEIK_GENERATE_ARTICLE_FUSIN` (generate_article) — environment: sheik is not in the local dump, so its vanilla `fusin` is not listed
- `vendor/hdr/fighters/sheik/src/status/special_s.rs:4` sheik status `*FIGHTER_SHEIK_GENERATE_ARTICLE_EXPLOSIONBOMB` (is_generatable) — environment: sheik is not in the local dump, so its vanilla `explosionbomb` is not listed
- `vendor/hdr/fighters/sheik/src/status/special_s.rs:5` sheik status `*FIGHTER_SHEIK_GENERATE_ARTICLE_EXPLOSIONBOMB` (generate_article_have_item) — environment: sheik is not in the local dump, so its vanilla `explosionbomb` is not listed
- `vendor/hdr/fighters/snake/src/status/appeal.rs:65` snake status `*FIGHTER_SNAKE_GENERATE_ARTICLE_CBOX` (shoot) — environment: snake is not in the local dump, so its vanilla `cbox` is not listed
- `vendor/hdr/fighters/snake/src/status/appeal.rs:163` snake status `*FIGHTER_SNAKE_GENERATE_ARTICLE_CBOX` (shoot) — environment: snake is not in the local dump, so its vanilla `cbox` is not listed
- `vendor/hdr/fighters/snake/src/status/appeal.rs:208` snake status `*FIGHTER_SNAKE_GENERATE_ARTICLE_CBOX` (shoot) — environment: snake is not in the local dump, so its vanilla `cbox` is not listed
- `vendor/hdr/fighters/snake/src/status/appeal.rs:210` snake status `*FIGHTER_SNAKE_GENERATE_ARTICLE_C4_SWITCH` (remove_exist) — environment: snake is not in the local dump, so its vanilla `c4_switch` is not listed
- `vendor/hdr/fighters/snake/src/status/rebirth.rs:6` snake status `*FIGHTER_SNAKE_GENERATE_ARTICLE_CBOX` (is_exist) — environment: snake is not in the local dump, so its vanilla `cbox` is not listed
- `vendor/hdr/fighters/snake/src/status/rebirth.rs:7` snake status: not a statement Visionary reads: `ArticleModule::shoot(`
- `vendor/hdr/fighters/szerosuit/src/status/rebirth.rs:6` szerosuit status `*FIGHTER_SZEROSUIT_GENERATE_ARTICLE_WHIP` (is_exist) — environment: szerosuit is not in the local dump, so its vanilla `whip` is not listed
- `vendor/hdr/fighters/szerosuit/src/status/rebirth.rs:7` szerosuit status `*FIGHTER_SZEROSUIT_GENERATE_ARTICLE_WHIP` (remove_exist) — environment: szerosuit is not in the local dump, so its vanilla `whip` is not listed
- `vendor/hdr/fighters/trail/src/status/special_n.rs:187` trail status `*FIGHTER_TRAIL_GENERATE_ARTICLE_FLOWER` (remove_exist) — environment: trail is not in the local dump, so its vanilla `flower` is not listed
- `vendor/hdr/fighters/trail/src/status/special_n.rs:196` trail status `*FIGHTER_TRAIL_GENERATE_ARTICLE_CLOUD` (remove_exist) — environment: trail is not in the local dump, so its vanilla `cloud` is not listed
- `vendor/hdr/fighters/wiifit/src/status/special_n.rs:9` wiifit status `*FIGHTER_WIIFIT_GENERATE_ARTICLE_SUNBULLET` (generate_article) — environment: wiifit is not in the local dump, so its vanilla `sunbullet` is not listed
- `vendor/hdr/fighters/zelda/src/status/special_lw.rs:5` zelda status `*FIGHTER_ZELDA_GENERATE_ARTICLE_PHANTOM` (is_exist) — environment: zelda is not in the local dump, so its vanilla `phantom` is not listed
- `vendor/hdr/fighters/zelda/src/status/special_lw.rs:57` zelda status `*FIGHTER_ZELDA_GENERATE_ARTICLE_PHANTOM` (generate_article) — environment: zelda is not in the local dump, so its vanilla `phantom` is not listed

### Unresolved statuses (10)

- `vendor/hdr/fighters/iceclimber/src/status/mod.rs:41` (no agent) Exit `*FIGHTER_POPO_STATUS_KIND_SPECIAL_HI_JUMP` — the agent is a parameter and no single module installs it from an Agent::new, or several agents claim the module
- `vendor/hdr/fighters/iceclimber/src/status/mod.rs:123` (no agent) Exit `*FIGHTER_POPO_STATUS_KIND_SPECIAL_HI_JUMP` — the agent is a parameter and no single module installs it from an Agent::new, or several agents claim the module
- `vendor/hdr/fighters/iceclimber/src/status/mod.rs:124` (no agent) Pre `*FIGHTER_POPO_STATUS_KIND_SPECIAL_HI_FAIL` — the agent is a parameter and no single module installs it from an Agent::new, or several agents claim the module
- `vendor/hdr/fighters/iceclimber/src/status/mod.rs:125` (no agent) Main `*FIGHTER_STATUS_KIND_THROW` — the agent is a parameter and no single module installs it from an Agent::new, or several agents claim the module
- `vendor/hdr/fighters/iceclimber/src/status/mod.rs:126` (no agent) Pre `*FIGHTER_STATUS_KIND_CATCH_WAIT` — the agent is a parameter and no single module installs it from an Agent::new, or several agents claim the module
- `vendor/hdr/fighters/iceclimber/src/status/mod.rs:127` (no agent) Main `*FIGHTER_STATUS_KIND_CATCH_WAIT` — the agent is a parameter and no single module installs it from an Agent::new, or several agents claim the module
- `vendor/hdr/fighters/iceclimber/src/status/mod.rs:128` (no agent) Pre `*FIGHTER_STATUS_KIND_CATCH_ATTACK` — the agent is a parameter and no single module installs it from an Agent::new, or several agents claim the module
- `vendor/hdr/fighters/iceclimber/src/status/mod.rs:129` (no agent) Main `*FIGHTER_STATUS_KIND_CATCH_ATTACK` — the agent is a parameter and no single module installs it from an Agent::new, or several agents claim the module
- `vendor/hdr/fighters/iceclimber/src/status/mod.rs:130` (no agent) Pre `*FIGHTER_POPO_STATUS_KIND_THROW_NANA` — the agent is a parameter and no single module installs it from an Agent::new, or several agents claim the module
- `vendor/hdr/fighters/iceclimber/src/status/mod.rs:131` (no agent) Main `*FIGHTER_POPO_STATUS_KIND_THROW_NANA` — the agent is a parameter and no single module installs it from an Agent::new, or several agents claim the module

### Agents no catalog lists (125)

- `bayonetta_specialn_bullet` (0 scripts, 0 statuses): environment: bayonetta is not in the local dump, so its vanilla articles are not listed
- `bayonetta_wickedweavearm` (0 scripts, 0 statuses): environment: bayonetta is not in the local dump, so its vanilla articles are not listed
- `bayonetta_wickedweaveleg` (0 scripts, 0 statuses): environment: bayonetta is not in the local dump, so its vanilla articles are not listed
- `brave_crash` (3 scripts, 0 statuses): environment: brave is not in the local dump, so its vanilla articles are not listed
- `brave_deathball` (2 scripts, 0 statuses): environment: brave is not in the local dump, so its vanilla articles are not listed
- `brave_explosion` (1 scripts, 0 statuses): environment: brave is not in the local dump, so its vanilla articles are not listed
- `brave_fireball` (3 scripts, 0 statuses): environment: brave is not in the local dump, so its vanilla articles are not listed
- `brave_flash` (0 scripts, 1 statuses): environment: brave is not in the local dump, so its vanilla articles are not listed
- `brave_lightning` (1 scripts, 0 statuses): environment: brave is not in the local dump, so its vanilla articles are not listed
- `brave_spark` (2 scripts, 0 statuses): environment: brave is not in the local dump, so its vanilla articles are not listed
- `brave_tornado` (6 scripts, 0 statuses): environment: brave is not in the local dump, so its vanilla articles are not listed
- `buddy_bullet` (0 scripts, 0 statuses): environment: buddy is not in the local dump, so its vanilla articles are not listed
- `buddy_pad` (0 scripts, 0 statuses): environment: buddy is not in the local dump, so its vanilla articles are not listed
- `cloud_wave` (6 scripts, 0 statuses): no folder for it under cloud in the dump and no clone adds it
- `daisy_kinopio` (0 scripts, 6 statuses): environment: daisy is not in the local dump, so its vanilla articles are not listed
- `demon_blaster` (1 scripts, 0 statuses): environment: demon is not in the local dump, so its vanilla articles are not listed
- `dolly_burst` (10 scripts, 0 statuses): environment: dolly is not in the local dump, so its vanilla articles are not listed
- `duckhunt_can` (3 scripts, 0 statuses): environment: duckhunt is not in the local dump, so its vanilla articles are not listed
- `duckhunt_clay` (2 scripts, 1 statuses): environment: duckhunt is not in the local dump, so its vanilla articles are not listed
- `duckhunt_gunman` (4 scripts, 0 statuses): environment: duckhunt is not in the local dump, so its vanilla articles are not listed
- `duckhunt_gunmanbullet` (1 scripts, 0 statuses): environment: duckhunt is not in the local dump, so its vanilla articles are not listed
- `duckhunt_reticle` (1 scripts, 0 statuses): environment: duckhunt is not in the local dump, so its vanilla articles are not listed
- `eflame_esword` (8 scripts, 0 statuses): environment: eflame is not in the local dump, so its vanilla articles are not listed
- `eflame_firepillar` (2 scripts, 0 statuses): environment: eflame is not in the local dump, so its vanilla articles are not listed
- `elight_bunshin` (6 scripts, 0 statuses): environment: elight is not in the local dump, so its vanilla articles are not listed
- `elight_exprosiveshot` (2 scripts, 1 statuses): environment: elight is not in the local dump, so its vanilla articles are not listed
- `falco_blaster_bullet` (4 scripts, 0 statuses): environment: falco is not in the local dump, so its vanilla articles are not listed
- `falco_illusion` (1 scripts, 0 statuses): environment: falco is not in the local dump, so its vanilla articles are not listed
- `gamewatch_breath` (1 scripts, 0 statuses): environment: gamewatch is not in the local dump, so its vanilla articles are not listed
- `gamewatch_food` (4 scripts, 0 statuses): environment: gamewatch is not in the local dump, so its vanilla articles are not listed
- `gamewatch_parachute` (1 scripts, 0 statuses): environment: gamewatch is not in the local dump, so its vanilla articles are not listed
- `gamewatch_rescue` (2 scripts, 0 statuses): environment: gamewatch is not in the local dump, so its vanilla articles are not listed
- `gekkouga_shuriken` (1 scripts, 0 statuses): no folder for it under gekkouga in the dump and no clone adds it
- `ike_sword` (6 scripts, 1 statuses): environment: ike is not in the local dump, so its vanilla articles are not listed
- `inkling_inkbullet` (1 scripts, 0 statuses): environment: inkling is not in the local dump, so its vanilla articles are not listed
- `inkling_roller` (9 scripts, 0 statuses): environment: inkling is not in the local dump, so its vanilla articles are not listed
- `inkling_splash` (1 scripts, 0 statuses): environment: inkling is not in the local dump, so its vanilla articles are not listed
- `inkling_splashbomb` (1 scripts, 0 statuses): environment: inkling is not in the local dump, so its vanilla articles are not listed
- `inkling_squid` (2 scripts, 0 statuses): environment: inkling is not in the local dump, so its vanilla articles are not listed
- `jack_doyle` (1 scripts, 1 statuses): no folder for it under jack in the dump and no clone adds it
- `jack_fire` (1 scripts, 0 statuses): no folder for it under jack in the dump and no clone adds it
- `ken_hadoken` (24 scripts, 0 statuses): environment: ken is not in the local dump, so its vanilla articles are not listed
- `ken_shinryuken` (2 scripts, 0 statuses): environment: ken is not in the local dump, so its vanilla articles are not listed
- `kirby_finalcuttershot` (1 scripts, 1 statuses): environment: kirby is not in the local dump, so its vanilla articles are not listed
- `koopa_breath` (4 scripts, 1 statuses): environment: koopa is not in the local dump, so its vanilla articles are not listed
- `lucas_pkfire` (0 scripts, 0 statuses): environment: lucas is not in the local dump, so its vanilla articles are not listed
- `lucas_pkthunder` (0 scripts, 2 statuses): environment: lucas is not in the local dump, so its vanilla articles are not listed
- `master_arrow1` (3 scripts, 0 statuses): environment: master is not in the local dump, so its vanilla articles are not listed
- `master_arrow2` (1 scripts, 0 statuses): environment: master is not in the local dump, so its vanilla articles are not listed
- `master_axe` (8 scripts, 0 statuses): environment: master is not in the local dump, so its vanilla articles are not listed
- `miifighter_ironball` (1 scripts, 0 statuses): no folder for it under miifighter in the dump and no clone adds it
- `miigunner_attackairf_bullet` (2 scripts, 0 statuses): environment: miigunner is not in the local dump, so its vanilla articles are not listed
- `miigunner_bottomshoot` (1 scripts, 1 statuses): environment: miigunner is not in the local dump, so its vanilla articles are not listed
- `miigunner_flamepillar` (3 scripts, 0 statuses): environment: miigunner is not in the local dump, so its vanilla articles are not listed
- `miigunner_fullthrottle` (1 scripts, 0 statuses): environment: miigunner is not in the local dump, so its vanilla articles are not listed
- `miigunner_grenadelauncher` (3 scripts, 0 statuses): environment: miigunner is not in the local dump, so its vanilla articles are not listed
- `miigunner_groundbomb` (3 scripts, 0 statuses): environment: miigunner is not in the local dump, so its vanilla articles are not listed
- `miigunner_gunnercharge` (2 scripts, 0 statuses): environment: miigunner is not in the local dump, so its vanilla articles are not listed
- `miigunner_rapidshot_bullet` (4 scripts, 0 statuses): environment: miigunner is not in the local dump, so its vanilla articles are not listed
- `miigunner_stealthbomb` (4 scripts, 9 statuses): environment: miigunner is not in the local dump, so its vanilla articles are not listed
- `miigunner_stealthbomb_s` (2 scripts, 0 statuses): environment: miigunner is not in the local dump, so its vanilla articles are not listed
- `miigunner_supermissile` (6 scripts, 5 statuses): environment: miigunner is not in the local dump, so its vanilla articles are not listed
- `ness_pkfire` (2 scripts, 0 statuses): environment: ness is not in the local dump, so its vanilla articles are not listed
- `ness_pkflash` (1 scripts, 0 statuses): environment: ness is not in the local dump, so its vanilla articles are not listed
- `ness_pkthunder` (0 scripts, 1 statuses): environment: ness is not in the local dump, so its vanilla articles are not listed
- `ness_yoyohead` (4 scripts, 0 statuses): environment: ness is not in the local dump, so its vanilla articles are not listed
- `packun_poisonbreath` (0 scripts, 0 statuses): environment: packun is not in the local dump, so its vanilla articles are not listed
- `packun_spikeball` (0 scripts, 0 statuses): environment: packun is not in the local dump, so its vanilla articles are not listed
- `pickel_fire` (0 scripts, 0 statuses): environment: pickel is not in the local dump, so its vanilla articles are not listed
- `pickel_fishingrod` (0 scripts, 2 statuses): environment: pickel is not in the local dump, so its vanilla articles are not listed
- `pickel_forge` (0 scripts, 0 statuses): environment: pickel is not in the local dump, so its vanilla articles are not listed
- `pickel_melt` (0 scripts, 0 statuses): environment: pickel is not in the local dump, so its vanilla articles are not listed
- `pickel_pushobject` (0 scripts, 0 statuses): environment: pickel is not in the local dump, so its vanilla articles are not listed
- `pickel_trolley` (0 scripts, 6 statuses): environment: pickel is not in the local dump, so its vanilla articles are not listed
- `pikmin_pikmin` (110 scripts, 2 statuses): environment: pikmin is not in the local dump, so its vanilla articles are not listed
- `pit_arrow` (1 scripts, 0 statuses): environment: pit is not in the local dump, so its vanilla articles are not listed
- `pitb_bowarrow` (1 scripts, 1 statuses): environment: pitb is not in the local dump, so its vanilla articles are not listed
- `plizardon_explosion` (1 scripts, 0 statuses): environment: plizardon is not in the local dump, so its vanilla articles are not listed
- `ptrainer_ptrainer` (0 scripts, 2 statuses): environment: ptrainer is not in the local dump, so its vanilla articles are not listed
- `pzenigame_water` (9 scripts, 5 statuses): environment: pzenigame is not in the local dump, so its vanilla articles are not listed
- `reflet_elwind` (2 scripts, 0 statuses): environment: reflet is not in the local dump, so its vanilla articles are not listed
- `reflet_gigafire` (4 scripts, 0 statuses): environment: reflet is not in the local dump, so its vanilla articles are not listed
- `reflet_thunder` (3 scripts, 0 statuses): environment: reflet is not in the local dump, so its vanilla articles are not listed
- `robot_beam` (1 scripts, 0 statuses): environment: robot is not in the local dump, so its vanilla articles are not listed
- `rosetta_tico` (0 scripts, 16 statuses): environment: rosetta is not in the local dump, so its vanilla articles are not listed
- `ryu_hadoken` (18 scripts, 0 statuses): environment: ryu is not in the local dump, so its vanilla articles are not listed
- `ryu_shinkuhadoken` (4 scripts, 3 statuses): environment: ryu is not in the local dump, so its vanilla articles are not listed
- `samus_cshot` (2 scripts, 0 statuses): environment: samus is not in the local dump, so its vanilla articles are not listed
- `samus_gbeam` (3 scripts, 0 statuses): environment: samus is not in the local dump, so its vanilla articles are not listed
- `samus_missile` (2 scripts, 0 statuses): environment: samus is not in the local dump, so its vanilla articles are not listed
- `samus_supermissile` (3 scripts, 0 statuses): environment: samus is not in the local dump, so its vanilla articles are not listed
- `samusd_bomb` (0 scripts, 0 statuses): environment: samusd is not in the local dump, so its vanilla articles are not listed
- `samusd_cshot` (0 scripts, 0 statuses): environment: samusd is not in the local dump, so its vanilla articles are not listed
- `samusd_gbeam` (0 scripts, 0 statuses): environment: samusd is not in the local dump, so its vanilla articles are not listed
- `samusd_missile` (0 scripts, 0 statuses): environment: samusd is not in the local dump, so its vanilla articles are not listed
- `samusd_supermissile` (0 scripts, 0 statuses): environment: samusd is not in the local dump, so its vanilla articles are not listed
- `sheik_fusin` (1 scripts, 0 statuses): environment: sheik is not in the local dump, so its vanilla articles are not listed
- `sheik_needle` (1 scripts, 0 statuses): environment: sheik is not in the local dump, so its vanilla articles are not listed
- `shizue_bullet` (1 scripts, 0 statuses): environment: shizue is not in the local dump, so its vanilla articles are not listed
- `shizue_clayrocket` (3 scripts, 1 statuses): environment: shizue is not in the local dump, so its vanilla articles are not listed
- `shizue_fishingrod` (1 scripts, 0 statuses): environment: shizue is not in the local dump, so its vanilla articles are not listed
- `shizue_pot` (1 scripts, 0 statuses): environment: shizue is not in the local dump, so its vanilla articles are not listed
- `shizue_trafficsign` (1 scripts, 0 statuses): environment: shizue is not in the local dump, so its vanilla articles are not listed
- `simon_axe` (1 scripts, 0 statuses): environment: simon is not in the local dump, so its vanilla articles are not listed
- `simon_whip` (14 scripts, 0 statuses): environment: simon is not in the local dump, so its vanilla articles are not listed
- `snake_c4` (5 scripts, 0 statuses): environment: snake is not in the local dump, so its vanilla articles are not listed
- `snake_nikitamissile` (12 scripts, 0 statuses): environment: snake is not in the local dump, so its vanilla articles are not listed
- `snake_trenchmortar` (1 scripts, 0 statuses): environment: snake is not in the local dump, so its vanilla articles are not listed
- `szerosuit_paralyzer_bullet` (2 scripts, 0 statuses): environment: szerosuit is not in the local dump, so its vanilla articles are not listed
- `szerosuit_whip` (5 scripts, 0 statuses): environment: szerosuit is not in the local dump, so its vanilla articles are not listed
- `tantan_beam` (0 scripts, 0 statuses): environment: tantan is not in the local dump, so its vanilla articles are not listed
- `tantan_punch1` (0 scripts, 0 statuses): environment: tantan is not in the local dump, so its vanilla articles are not listed
- `tantan_punch2` (0 scripts, 0 statuses): environment: tantan is not in the local dump, so its vanilla articles are not listed
- `tantan_punch3` (0 scripts, 0 statuses): environment: tantan is not in the local dump, so its vanilla articles are not listed
- `tantan_ring` (0 scripts, 0 statuses): environment: tantan is not in the local dump, so its vanilla articles are not listed
- `trail_fire` (2 scripts, 0 statuses): environment: trail is not in the local dump, so its vanilla articles are not listed
- `trail_thunder` (1 scripts, 0 statuses): environment: trail is not in the local dump, so its vanilla articles are not listed
- `wiifit_hulahoop` (1 scripts, 0 statuses): environment: wiifit is not in the local dump, so its vanilla articles are not listed
- `wiifit_sunbullet` (2 scripts, 0 statuses): environment: wiifit is not in the local dump, so its vanilla articles are not listed
- `wolf_blaster_bullet` (1 scripts, 0 statuses): environment: wolf is not in the local dump, so its vanilla articles are not listed
- `wolf_illusion` (2 scripts, 0 statuses): environment: wolf is not in the local dump, so its vanilla articles are not listed
- `yoshi_tamago` (2 scripts, 0 statuses): environment: yoshi is not in the local dump, so its vanilla articles are not listed
- `zelda_dein` (2 scripts, 3 statuses): environment: zelda is not in the local dump, so its vanilla articles are not listed
- `zelda_dein_s` (2 scripts, 0 statuses): environment: zelda is not in the local dump, so its vanilla articles are not listed
- `zelda_phantom` (14 scripts, 1 statuses): environment: zelda is not in the local dump, so its vanilla articles are not listed

### Registered twice (13)

- krool_backpack/effect_fly is registered more than once
- krool_backpack/effect_start is registered more than once
- krool_crown/game_throw is registered more than once
- krool_ironball/game_shoot is registered more than once
- krool_ironball/game_spitshoot is registered more than once
- packun_firebreath/effect_bound is registered more than once
- packun_firebreath/effect_regular is registered more than once
- packun_firebreath/game_regular is registered more than once
- pichu_dengekidama/effect_regular is registered more than once
- pichu_dengekidama/game_regular is registered more than once
- pichu_kaminari/game_regular is registered more than once
- ridley_breath/game_fly is registered more than once
- sonic_gimmickjump/game_fall is registered more than once

## Iron-Man

Root `C:/Users/notja/Documents/Coding/SSBU Modding/Movesets\Iron-Man`. Fighters: mario (dump).

| Article | Origin | Ids | Scripts | Rows (spawns) | Status rows | Param block | Settings |
|---|---|---|---|---|---|---|---|
| mario_cappy | rewritten | FIGHTER_MARIO_GENERATE_ARTICLE_CAPPY | 1 | 5 (1) | 0 | absent |  |
| mario_fireball | rewritten | FIGHTER_MARIO_GENERATE_ARTICLE_FIREBALL | 2 | 6 (6) | 0 | present |  |
| mario_hugeflame | rewritten | FIGHTER_MARIO_GENERATE_ARTICLE_HUGEFLAME | 2 | 0 (0) | 3 | present |  |
| mario_beamcloned | clone of robot/WEAPON_KIND_ROBOT_BEAM | FIGHTER_STARK_GENERATE_ARTICLE_UNIBEAM | 3 | 3 (3) | 0 | absent | Acceleration X: literal 0; Acceleration Y: literal 0; Brake X: literal 0; Brake Y: literal 0; Max speed X: computed `x_speed (reads unibeam.strong_speed)`; Max speed Y: literal 0; Settling speed X: computed `x_speed (reads unibeam.strong_speed)`; Settling speed Y: literal 0; Speed X: computed `x_speed (reads unibeam.strong_speed)`; Speed Y: literal 0 |
| mario_ironballcloned | clone of krool/WEAPON_KIND_KROOL_IRONBALL | FIGHTER_STARK_GENERATE_ARTICLE_SMARTMINE | 6 | 4 (2) | 2 | absent |  |
| mario_cannonballcloned | clone of koopajr/WEAPON_KIND_KOOPAJR_CANNONBALL | FIGHTER_STARK_GENERATE_ARTICLE_ROCKET | 2 | 2 (1) | 0 | absent | Acceleration X: literal 0; Acceleration Y: literal 0; Life: computed `life`, computed `life`; Settling speed X: computed `speed_x`; Settling speed Y: computed `-speed_y`; Speed X: computed `speed_x`; Speed Y: computed `-speed_y` |

### Unread param_config overrides (1)

- `src/lib.rs:69` `param_config::update_int_2(*FIGHTER_KIND_MARIO, marked_slots.clone(), (smash::hash40("param_motion"), (smash::hash40("catch_ik")), *FIGHTER_CATCH_IK_NONE))` — its param name or value is not a literal

## King-Boo-Moveset

Root `C:/Users/notja/Documents/Coding/SSBU Modding/Movesets\King-Boo-Moveset`. Fighters: dedede (dump).

| Article | Origin | Ids | Scripts | Rows (spawns) | Status rows | Param block | Settings |
|---|---|---|---|---|---|---|---|
| dedede_shrine | vanilla | FIGHTER_DEDEDE_GENERATE_ARTICLE_SHRINE | 0 | 2 (0) | 0 | absent |  |
| dedede_fireball | clone of miiswordsman/WEAPON_KIND_MIISWORDSMAN_WAVE | FIGHTER_KINGTERESA_GENERATE_ARTICLE_FIREBALL | 5 | 1 (1) | 0 | absent | Acceleration X: literal 0; Gravity: computed `accel_y`; Life: literal 30, literal 30; Power multiplier: computed `charge_boost + 1.0`; Speed X: computed `speed_x`; Speed Y: computed `speed_y` |
| dedede_portrait | clone of dolly/WEAPON_KIND_DOLLY_CAP | FIGHTER_KINGTERESA_GENERATE_ARTICLE_PORTRAIT | 2 | 4 (2) | 0 | absent | Life: literal 20, literal 20 |
| dedede_spikeball | clone of krool/WEAPON_KIND_KROOL_IRONBALL | FIGHTER_KINGTERESA_GENERATE_ARTICLE_SPIKEBALL | 6 | 1 (1) | 0 | absent | Acceleration X: literal -0.01, literal 0, computed `-0.01 * -lr`; Gravity: literal 0.08, literal 0.08, literal 0.08; Life: literal 50, literal 50, literal 30, literal 30; Power multiplier: computed `charge_boost + 1.0`; Speed X: computed `speed_x`, literal 0, computed `speed_x`, computed `speed_x`, computed `-speed_x`; Speed Y: literal 0.6, computed `speed_y`, literal 0, computed `-speed_y`, computed `speed_y` |
| dedede_teresa | clone of daisy/WEAPON_KIND_DAISY_KINOPIO | FIGHTER_KINGTERESA_GENERATE_ARTICLE_TERESA | 7 | 4 (2) | 0 | absent | Life: literal 30, literal 30; Speed X: computed `TERESA_MOVE_SPEED * uv.x`; Speed Y: computed `TERESA_MOVE_SPEED * uv.y` |
| dedede_thunder | clone of packun/WEAPON_KIND_PACKUN_POISONBREATH | FIGHTER_KINGTERESA_GENERATE_ARTICLE_THUNDER | 2 | 1 (1) | 0 | absent | Life: const THUNDER_INIT_LIFE = 60, const THUNDER_INIT_LIFE = 60; Power multiplier: computed `charge_boost + 1.0` |

### Unread param_config overrides (11)

- `src/lib.rs:272` `param_config::update_attribute_mul_2(*FIGHTER_KIND_DEDEDE, marked_slots.clone(), (smash::hash40("air_accel_x_mul"), 0, 0.07 / 0.04))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/lib.rs:273` `param_config::update_attribute_mul_2(*FIGHTER_KIND_DEDEDE, marked_slots.clone(), (smash::hash40("air_accel_x_add"), 0, 0.04 / 0.01))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/lib.rs:274` `param_config::update_attribute_mul_2(*FIGHTER_KIND_DEDEDE, marked_slots.clone(), (smash::hash40("air_speed_x_stable"), 0, 1.306 / 0.735))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/lib.rs:275` `param_config::update_attribute_mul_2(*FIGHTER_KIND_DEDEDE, marked_slots.clone(), (smash::hash40("air_accel_y"), 0, 0.060 / 0.097))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/lib.rs:276` `param_config::update_attribute_mul_2(*FIGHTER_KIND_DEDEDE, marked_slots.clone(), (smash::hash40("air_speed_y_stable"), 0, 1.04 / 1.95))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/lib.rs:277` `param_config::update_attribute_mul_2(*FIGHTER_KIND_DEDEDE, marked_slots.clone(), (smash::hash40("dive_speed_y"), 0, 1.664 / 3.12))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/lib.rs:279` `param_config::update_attribute_mul_2(*FIGHTER_KIND_DEDEDE, marked_slots.clone(), (smash::hash40("jump_initial_y"), 0, 15.45 / 18.0675))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/lib.rs:280` `param_config::update_attribute_mul_2(*FIGHTER_KIND_DEDEDE, marked_slots.clone(), (smash::hash40("jump_y"), 0, 23.02 / 32.85))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/lib.rs:281` `param_config::update_attribute_mul_2(*FIGHTER_KIND_DEDEDE, marked_slots.clone(), (smash::hash40("mini_jump_y"), 0, 12.48 / 16.02))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/lib.rs:282` `param_config::update_attribute_mul_2(*FIGHTER_KIND_DEDEDE, marked_slots.clone(), (smash::hash40("jump_aerial_y"), 0, 18.4 / 32.85))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/lib.rs:284` `param_config::update_attribute_mul_2(*FIGHTER_KIND_DEDEDE, marked_slots.clone(), (smash::hash40("weight"), 0, 72.0 / 98.0))` — it multiplies the value instead of setting it, which the card doesn't show

## Latios-Moveset

Root `C:/Users/notja/Documents/Coding/SSBU Modding/Movesets\Latios-Moveset`. Fighters: samus (not in the dump).

| Article | Origin | Ids | Scripts | Rows (spawns) | Status rows | Param block | Settings |
|---|---|---|---|---|---|---|---|
| samus_latihado | clone of link/WEAPON_KIND_LINK_SWORD_BEAM | FIGHTER_SAMUS_GENERATE_ARTICLE_LATIHADO | 3 | 3 (3) | 0 | present | Acceleration X: computed `deccel_x_final (reads param_swordbeam.deccel_x)`; Acceleration Y: literal 0; Life: computed `lerp (reads param_swordbeam.min_life, param_swordbeam.max_life)`, computed `lerp (reads param_swordbeam.min_life, param_swordbeam.max_life)`; Size: computed `lerp4 (reads param_swordbeam.min_scale, param_swordbeam.max_scale)`; Speed X: computed `speed_x_final (reads param_swordbeam.min_speed_x, param_swordbeam.max_`; Speed Y: literal 0 |
| samus_latiball | clone of koopajr/WEAPON_KIND_KOOPAJR_CANNONBALL | FIGHTER_SAMUS_GENERATE_ARTICLE_LATIBALL | 3 | 0 (0) | 0 | absent | Other movement: computed `sv_kinetic_energy!( set_speed, weapon, WEAPON_KINETIC_ENERGY_RESERVE_I`, computed `sv_kinetic_energy!( set_speed, weapon, WEAPON_KINETIC_ENERGY_RESERVE_I` |
| samus_latisnow | clone of luigi/WEAPON_KIND_LUIGI_FIREBALL | FIGHTER_SAMUS_GENERATE_ARTICLE_LATISNOW | 6 | 6 (6) | 0 | present | Other movement: computed `sv_kinetic_energy!( set_speed, weapon, WEAPON_KINETIC_ENERGY_RESERVE_I` |
| samus_lativolt | clone of mario/WEAPON_KIND_MARIO_FIREBALL | FIGHTER_SAMUS_GENERATE_ARTICLE_LATIVOLT | 6 | 6 (6) | 0 | present | Other movement: computed `sv_kinetic_energy!( set_speed, weapon, WEAPON_KINETIC_ENERGY_RESERVE_I` |

### Unresolved ids (scripts) (6)

- `src/lati_ball/mod.rs:180` samus/game_specialairnfire `*FIGHTER_SAMUS_GENERATE_ARTICLE_CSHOT` (shoot_exist) — environment: samus is not in the local dump, so its vanilla `cshot` is not listed
- `src/lati_ball/mod.rs:188` samus/game_specialairnfiremax `*FIGHTER_SAMUS_GENERATE_ARTICLE_CSHOT` (shoot_exist) — environment: samus is not in the local dump, so its vanilla `cshot` is not listed
- `src/lati_ball/mod.rs:173` samus/game_specialairnstart `*FIGHTER_LUCARIO_GENERATE_ARTICLE_AURABALL` (generate_article) — another fighter's id (lucario's auraball)
- `src/lati_ball/mod.rs:180` samus/game_specialnfire `*FIGHTER_SAMUS_GENERATE_ARTICLE_CSHOT` (shoot_exist) — environment: samus is not in the local dump, so its vanilla `cshot` is not listed
- `src/lati_ball/mod.rs:188` samus/game_specialnfiremax `*FIGHTER_SAMUS_GENERATE_ARTICLE_CSHOT` (shoot_exist) — environment: samus is not in the local dump, so its vanilla `cshot` is not listed
- `src/lati_ball/mod.rs:173` samus/game_specialnstart `*FIGHTER_LUCARIO_GENERATE_ARTICLE_AURABALL` (generate_article) — another fighter's id (lucario's auraball)

### Agents no catalog lists (1)

- `samus_cshot` (2 scripts, 0 statuses): environment: samus is not in the local dump, so its vanilla articles are not listed

## Master-Chief-Moveset

Root `C:/Users/notja/Documents/Coding/SSBU Modding/Movesets\Master-Chief-Moveset\Master Chief-rs`. Fighters: master (not in the dump).

| Article | Origin | Ids | Scripts | Rows (spawns) | Status rows | Param block | Settings |
|---|---|---|---|---|---|---|---|
| master_plasmashot | clone of koopa/WEAPON_KIND_KOOPA_BREATH | FIGHTER_MASTER_GENERATE_ARTICLE_PLASMASHOT | 2 | 3 (3) | 0 | no param file | Acceleration X: literal 0; Acceleration Y: literal 0; Life: computed `life (reads param_plasmashot.life)`; Settling speed X: computed `speed_x (reads param_plasmashot.min_speed)`; Settling speed Y: computed `speed_y (reads param_plasmashot.min_speed)`; Speed X: computed `speed_x (reads param_plasmashot.min_speed)`; Speed Y: computed `speed_y (reads param_plasmashot.min_speed)` |
| master_gravlift | clone of buddy/WEAPON_KIND_BUDDY_PAD | none | 2 | 0 (0) | 0 | no param file | Life: computed `life`; Other movement: computed `sv_kinetic_energy!(set_stable_speed, agent, *WEAPON_KINETIC_ENERGY_RES`, computed `sv_kinetic_energy!(set_brake, agent, *WEAPON_KINETIC_ENERGY_RESERVE_ID`, computed `sv_kinetic_energy!(set_accel, agent, *WEAPON_KINETIC_ENERGY_RESERVE_ID`, computed `sv_kinetic_energy!(set_speed, agent, *WEAPON_KINETIC_ENERGY_RESERVE_ID`; Speed X: literal 0; Speed Y: literal 0 |

### Unresolved ids (scripts) (14)

- `src/chief/specials/special_hi.rs:107` master/game_mc_specialairhi `FIGHTER_MASTERCHIEF_GENERATE_ARTICLE_GRAVLIFT` (generate_article) — another fighter's id (masterchief's gravlift)
- `src/chief/specials/special_hi.rs:113` master/game_mc_specialairhi `FIGHTER_MASTERCHIEF_GENERATE_ARTICLE_GRAVLIFT` (change_status) — another fighter's id (masterchief's gravlift)
- `src/chief/specials/special_n.rs:230` master/game_mc_specialairnhi `*FIGHTER_MASTER_GENERATE_ARTICLE_ARROW1` (change_status) — environment: master is not in the local dump, so its vanilla `arrow1` is not listed
- `src/chief/specials/special_n.rs:230` master/game_mc_specialairnlw `*FIGHTER_MASTER_GENERATE_ARTICLE_ARROW1` (change_status) — environment: master is not in the local dump, so its vanilla `arrow1` is not listed
- `src/chief/specials/special_n.rs:230` master/game_mc_specialairns `*FIGHTER_MASTER_GENERATE_ARTICLE_ARROW1` (change_status) — environment: master is not in the local dump, so its vanilla `arrow1` is not listed
- `src/chief/specials/special_n.rs:222` master/game_mc_specialairnstart `*FIGHTER_MASTER_GENERATE_ARTICLE_ARROW1` (generate_article) — environment: master is not in the local dump, so its vanilla `arrow1` is not listed
- `src/chief/specials/special_hi.rs:107` master/game_mc_specialhi `FIGHTER_MASTERCHIEF_GENERATE_ARTICLE_GRAVLIFT` (generate_article) — another fighter's id (masterchief's gravlift)
- `src/chief/specials/special_hi.rs:113` master/game_mc_specialhi `FIGHTER_MASTERCHIEF_GENERATE_ARTICLE_GRAVLIFT` (change_status) — another fighter's id (masterchief's gravlift)
- `src/chief/specials/special_n.rs:230` master/game_mc_specialnhi `*FIGHTER_MASTER_GENERATE_ARTICLE_ARROW1` (change_status) — environment: master is not in the local dump, so its vanilla `arrow1` is not listed
- `src/chief/specials/special_n.rs:230` master/game_mc_specialnlw `*FIGHTER_MASTER_GENERATE_ARTICLE_ARROW1` (change_status) — environment: master is not in the local dump, so its vanilla `arrow1` is not listed
- `src/chief/specials/special_n.rs:230` master/game_mc_specialns `*FIGHTER_MASTER_GENERATE_ARTICLE_ARROW1` (change_status) — environment: master is not in the local dump, so its vanilla `arrow1` is not listed
- `src/chief/specials/special_n.rs:222` master/game_mc_specialnstart `*FIGHTER_MASTER_GENERATE_ARTICLE_ARROW1` (generate_article) — environment: master is not in the local dump, so its vanilla `arrow1` is not listed
- `src/chief/other/throws.rs:28` master/game_mc_throwf `*FIGHTER_MASTER_GENERATE_ARTICLE_ARROW1` (generate_article) — environment: master is not in the local dump, so its vanilla `arrow1` is not listed
- `src/chief/other/throws.rs:32` master/game_mc_throwf `*FIGHTER_MASTER_GENERATE_ARTICLE_ARROW1` (change_status) — environment: master is not in the local dump, so its vanilla `arrow1` is not listed

### Unresolved ids (status code) (9)

- `src/chief/other/throws.rs:15` master status `*FIGHTER_MASTER_GENERATE_ARTICLE_ARROW1` (motion_kind) — environment: master is not in the local dump, so its vanilla `arrow1` is not listed
- `src/chief/other/throws.rs:16` master status `*FIGHTER_MASTER_GENERATE_ARTICLE_ARROW1` (remove_exist) — environment: master is not in the local dump, so its vanilla `arrow1` is not listed
- `src/chief/specials/special_hi.rs:96` master status `FIGHTER_MASTERCHIEF_GENERATE_ARTICLE_GRAVLIFT` (motion_kind) — another fighter's id (masterchief's gravlift)
- `src/chief/specials/special_hi.rs:97` master status `FIGHTER_MASTERCHIEF_GENERATE_ARTICLE_GRAVLIFT` (remove_exist) — another fighter's id (masterchief's gravlift)
- `src/chief/specials/special_n.rs:62` master status `*FIGHTER_MASTER_GENERATE_ARTICLE_ARROW1` (get_active_num) — environment: master is not in the local dump, so its vanilla `arrow1` is not listed
- `src/chief/specials/special_n.rs:112` master status `*FIGHTER_MASTER_GENERATE_ARTICLE_ARROW1` (motion_kind) — environment: master is not in the local dump, so its vanilla `arrow1` is not listed
- `src/chief/specials/special_n.rs:114` master status `*FIGHTER_MASTER_GENERATE_ARTICLE_ARROW1` (remove_exist) — environment: master is not in the local dump, so its vanilla `arrow1` is not listed
- `src/chief/specials/special_n.rs:211` master status `*FIGHTER_MASTER_GENERATE_ARTICLE_ARROW1` (motion_kind) — environment: master is not in the local dump, so its vanilla `arrow1` is not listed
- `src/chief/specials/special_n.rs:212` master status `*FIGHTER_MASTER_GENERATE_ARTICLE_ARROW1` (remove_exist) — environment: master is not in the local dump, so its vanilla `arrow1` is not listed

### Agents no catalog lists (2)

- `master_arrow1` (11 scripts, 16 statuses): environment: master is not in the local dump, so its vanilla articles are not listed
- `master_sword` (12 scripts, 0 statuses): environment: master is not in the local dump, so its vanilla articles are not listed

## Parvus-Moveset

Root `C:/Users/notja/Documents/Coding/SSBU Modding/Movesets\Parvus-Moveset`. Fighters: .

| Article | Origin | Ids | Scripts | Rows (spawns) | Status rows | Param block | Settings |
|---|---|---|---|---|---|---|---|

### Unresolved statuses (1)

- `plugin/src/parvus/frame.rs:41` (no agent) Start `` — the agent is a parameter and no single module installs it from an Agent::new, or several agents claim the module

## Potemkin-SL2

Root `C:/Users/notja/Documents/Coding/SSBU Modding/Movesets\Potemkin-SL2`. Fighters: kamui (dump).

| Article | Origin | Ids | Scripts | Rows (spawns) | Status rows | Param block | Settings |
|---|---|---|---|---|---|---|---|
| kamui_fdb | clone of cloud/WEAPON_KIND_CLOUD_WAVE, original code | FIGHTER_KAMUI_GENERATE_ARTICLE_FDB | 5 | 0 (0) | 0 | absent | Acceleration X: literal 2, literal 0; Acceleration Y: literal 0, literal 0; Life: literal 90, literal 90; Max speed X: literal 2, literal 0; Max speed Y: literal 0, literal 0; Size: literal 0.45; Speed X: literal 2, literal 0; Speed Y: literal 0, literal 0 |
| kamui_weapon | clone of kamui/WEAPON_KIND_KAMUI_WATERDRAGON | FIGHTER_KAMUI_GENERATE_ARTICLE_WEAPON | 14 | 13 (4) | 26 | absent |  |
| kamui_effect | clone of kamui/WEAPON_KIND_KAMUI_DRAGONHAND | FIGHTER_KAMUI_GENERATE_ARTICLE_EFFECT | 3 | 15 (3) | 8 | absent |  |
| kamui_heavenly | clone of kamui/WEAPON_KIND_KAMUI_SPEARHAND | FIGHTER_KAMUI_GENERATE_ARTICLE_HEAVENLY | 0 | 0 (0) | 5 | absent |  |

### Unread param_config overrides (7)

- `src/lib.rs:81` `param_config::update_float_2(*FIGHTER_KIND_KAMUI, marked_slots.clone(), (smash::hash40("param_private"), 0x19996b81b6, 1200.0))` — its second hash is not a literal
- `src/lib.rs:149` `param_config::update_int_2  (*FIGHTER_KIND_KAMUI, marked_slots.clone(), (smash::hash40("size"),0, *FIGHTER_SIZE_LL))` — its param name or value is not a literal
- `src/lib.rs:150` `param_config::update_int_2  (*FIGHTER_KIND_KAMUI, marked_slots.clone(), (smash::hash40("bury_motion_height"),0, *HIT_HEIGHT_HIGH))` — its param name or value is not a literal
- `src/lib.rs:151` `param_config::update_int_2  (*FIGHTER_KIND_KAMUI, marked_slots.clone(), (smash::hash40("bury_r_motion_height"),0, *HIT_HEIGHT_HIGH))` — its param name or value is not a literal
- `src/lib.rs:152` `param_config::update_int_2  (*FIGHTER_KIND_KAMUI, marked_slots.clone(), (smash::hash40("attack_combo_type"),0, *FIGHTER_COMBO_TYPE_NONE))` — its param name or value is not a literal
- `src/lib.rs:155` `param_config::update_int_2  (*FIGHTER_KIND_KAMUI, marked_slots.clone(), (smash::hash40("param_motion"),smash::hash40("motion_share"), *FIGHTER_MOTION_SHARE_TYPE_BIG))` — its param name or value is not a literal
- `src/lib.rs:156` `param_config::update_int_2  (*FIGHTER_KIND_KAMUI, marked_slots.clone(), (smash::hash40("param_motion"),smash::hash40("waist_size"), *FIGHTER_WAIST_SIZE_L))` — its param name or value is not a literal

## Sackboy-Moveset

Root `C:/Users/notja/Documents/Coding/SSBU Modding/Movesets\Sackboy-Moveset`. Fighters: shizue (not in the dump).

| Article | Origin | Ids | Scripts | Rows (spawns) | Status rows | Param block | Settings |
|---|---|---|---|---|---|---|---|
| shizue_pad | clone of buddy/WEAPON_KIND_BUDDY_PAD | GENERATE_ARTICLE_BOX | 6 | 2 (1) | 0 | no param file | Acceleration X: literal 0; Gravity: computed `accel_y`; Speed X: computed `speed_x`; Speed Y: computed `speed_y` |

### Unresolved ids (scripts) (43)

- `src/shizue/acmd/airlasso.rs:7` shizue/game_aircatch `*FIGHTER_SHIZUE_GENERATE_ARTICLE_FISHINGROD` (generate_article) — environment: shizue is not in the local dump, so its vanilla `fishingrod` is not listed
- `src/shizue/acmd/airlasso.rs:22` shizue/game_aircatch `*FIGHTER_SHIZUE_GENERATE_ARTICLE_FISHINGROD` (change_status) — environment: shizue is not in the local dump, so its vanilla `fishingrod` is not listed
- `src/shizue/acmd/airlasso.rs:34` shizue/game_aircatch `*FIGHTER_SHIZUE_GENERATE_ARTICLE_FISHINGROD` (change_status) — environment: shizue is not in the local dump, so its vanilla `fishingrod` is not listed
- `src/shizue/acmd/airlasso.rs:46` shizue/game_aircatch `*FIGHTER_SHIZUE_GENERATE_ARTICLE_FISHINGROD` (remove_exist) — environment: shizue is not in the local dump, so its vanilla `fishingrod` is not listed
- `src/shizue/acmd/airlasso.rs:47` shizue/game_aircatch `*FIGHTER_SHIZUE_GENERATE_ARTICLE_FISHINGLINE` (remove_exist) — environment: shizue is not in the local dump, so its vanilla `fishingline` is not listed
- `src/shizue/acmd/airlasso.rs:46` shizue/game_aircatch `*FIGHTER_SHIZUE_GENERATE_ARTICLE_FISHINGROD` (remove_exist) — environment: shizue is not in the local dump, so its vanilla `fishingrod` is not listed
- `src/shizue/acmd/airlasso.rs:106` shizue/game_aircatchlanding `*FIGHTER_SHIZUE_GENERATE_ARTICLE_FISHINGROD` (change_status) — environment: shizue is not in the local dump, so its vanilla `fishingrod` is not listed
- `src/shizue/acmd/ground.rs:158` shizue/game_attackhi4 `*FIGHTER_SHIZUE_GENERATE_ARTICLE_TRAFFICSIGN` (generate_article) — environment: shizue is not in the local dump, so its vanilla `trafficsign` is not listed
- `src/shizue/acmd/ground.rs:160` shizue/game_attackhi4 `*FIGHTER_SHIZUE_GENERATE_ARTICLE_TRAFFICSIGN` (change_motion) — environment: shizue is not in the local dump, so its vanilla `trafficsign` is not listed
- `src/shizue/acmd/ground.rs:158` shizue/game_attackhi4 `*FIGHTER_SHIZUE_GENERATE_ARTICLE_TRAFFICSIGN` (generate_article) — environment: shizue is not in the local dump, so its vanilla `trafficsign` is not listed
- `src/shizue/acmd/ground.rs:171` shizue/game_attackhi4 `*FIGHTER_SHIZUE_GENERATE_ARTICLE_TRAFFICSIGN` (change_motion) — environment: shizue is not in the local dump, so its vanilla `trafficsign` is not listed
- `src/shizue/acmd/ground.rs:183` shizue/game_attackhi4 `*FIGHTER_SHIZUE_GENERATE_ARTICLE_TRAFFICSIGN` (remove_exist) — environment: shizue is not in the local dump, so its vanilla `trafficsign` is not listed
- `src/shizue/acmd/ground.rs:228` shizue/game_attackhi4charge `*FIGHTER_SHIZUE_GENERATE_ARTICLE_TRAFFICSIGN` (change_motion) — environment: shizue is not in the local dump, so its vanilla `trafficsign` is not listed
- `src/shizue/acmd/ground.rs:305` shizue/game_attacklw4 `*FIGHTER_SHIZUE_GENERATE_ARTICLE_BUCKET` (generate_article) — environment: shizue is not in the local dump, so its vanilla `bucket` is not listed
- `src/shizue/acmd/ground.rs:306` shizue/game_attacklw4 `*FIGHTER_SHIZUE_GENERATE_ARTICLE_BUCKET` (change_motion) — environment: shizue is not in the local dump, so its vanilla `bucket` is not listed
- `src/shizue/acmd/ground.rs:305` shizue/game_attacklw4 `*FIGHTER_SHIZUE_GENERATE_ARTICLE_BUCKET` (generate_article) — environment: shizue is not in the local dump, so its vanilla `bucket` is not listed
- `src/shizue/acmd/ground.rs:319` shizue/game_attacklw4 `*FIGHTER_SHIZUE_GENERATE_ARTICLE_BUCKET` (change_motion) — environment: shizue is not in the local dump, so its vanilla `bucket` is not listed
- `src/shizue/acmd/ground.rs:346` shizue/game_attacklw4 `*FIGHTER_SHIZUE_GENERATE_ARTICLE_BUCKET` (remove_exist) — environment: shizue is not in the local dump, so its vanilla `bucket` is not listed
- `src/shizue/acmd/ground.rs:403` shizue/game_attacklw4charge `*FIGHTER_SHIZUE_GENERATE_ARTICLE_BUCKET` (change_motion) — environment: shizue is not in the local dump, so its vanilla `bucket` is not listed
- `src/shizue/acmd/catch.rs:7` shizue/game_catch `*FIGHTER_SHIZUE_GENERATE_ARTICLE_FISHINGROD` (generate_article) — environment: shizue is not in the local dump, so its vanilla `fishingrod` is not listed
- `src/shizue/acmd/catch.rs:8` shizue/game_catch `*FIGHTER_SHIZUE_GENERATE_ARTICLE_FISHINGROD` (change_status) — environment: shizue is not in the local dump, so its vanilla `fishingrod` is not listed
- `src/shizue/acmd/catch.rs:19` shizue/game_catch `*FIGHTER_SHIZUE_GENERATE_ARTICLE_FISHINGROD` (change_status) — environment: shizue is not in the local dump, so its vanilla `fishingrod` is not listed
- `src/shizue/acmd/catch.rs:38` shizue/game_catch `*FIGHTER_SHIZUE_GENERATE_ARTICLE_FISHINGROD` (change_status) — environment: shizue is not in the local dump, so its vanilla `fishingrod` is not listed
- `src/shizue/acmd/catch.rs:42` shizue/game_catch `*FIGHTER_SHIZUE_GENERATE_ARTICLE_FISHINGROD` (remove_exist) — environment: shizue is not in the local dump, so its vanilla `fishingrod` is not listed
- `src/shizue/acmd/catch.rs:49` shizue/game_catchdash `*FIGHTER_SHIZUE_GENERATE_ARTICLE_FISHINGROD` (generate_article) — environment: shizue is not in the local dump, so its vanilla `fishingrod` is not listed
- `src/shizue/acmd/catch.rs:50` shizue/game_catchdash `*FIGHTER_SHIZUE_GENERATE_ARTICLE_FISHINGROD` (change_status) — environment: shizue is not in the local dump, so its vanilla `fishingrod` is not listed
- `src/shizue/acmd/catch.rs:58` shizue/game_catchdash `*FIGHTER_SHIZUE_GENERATE_ARTICLE_FISHINGROD` (change_status) — environment: shizue is not in the local dump, so its vanilla `fishingrod` is not listed
- `src/shizue/acmd/catch.rs:76` shizue/game_catchdash `*FIGHTER_SHIZUE_GENERATE_ARTICLE_FISHINGROD` (change_status) — environment: shizue is not in the local dump, so its vanilla `fishingrod` is not listed
- `src/shizue/acmd/catch.rs:80` shizue/game_catchdash `*FIGHTER_SHIZUE_GENERATE_ARTICLE_FISHINGROD` (remove_exist) — environment: shizue is not in the local dump, so its vanilla `fishingrod` is not listed
- `src/shizue/acmd/catch.rs:87` shizue/game_catchturn `*FIGHTER_SHIZUE_GENERATE_ARTICLE_FISHINGROD` (generate_article) — environment: shizue is not in the local dump, so its vanilla `fishingrod` is not listed
- `src/shizue/acmd/catch.rs:88` shizue/game_catchturn `*FIGHTER_SHIZUE_GENERATE_ARTICLE_FISHINGROD` (change_status) — environment: shizue is not in the local dump, so its vanilla `fishingrod` is not listed
- `src/shizue/acmd/catch.rs:96` shizue/game_catchturn `*FIGHTER_SHIZUE_GENERATE_ARTICLE_FISHINGROD` (change_status) — environment: shizue is not in the local dump, so its vanilla `fishingrod` is not listed
- `src/shizue/acmd/catch.rs:114` shizue/game_catchturn `*FIGHTER_SHIZUE_GENERATE_ARTICLE_FISHINGROD` (change_status) — environment: shizue is not in the local dump, so its vanilla `fishingrod` is not listed
- `src/shizue/acmd/catch.rs:118` shizue/game_catchturn `*FIGHTER_SHIZUE_GENERATE_ARTICLE_FISHINGROD` (remove_exist) — environment: shizue is not in the local dump, so its vanilla `fishingrod` is not listed
- `src/shizue/acmd/other.rs:664` shizue/game_finalr `*FIGHTER_SHIZUE_GENERATE_ARTICLE_BALLOON` (generate_article) — environment: shizue is not in the local dump, so its vanilla `balloon` is not listed
- `src/shizue/acmd/specialhi.rs:21` shizue/game_specialairhi `GENERATE_ARTICLE_BOUNCEPAD` (generate_article) — an expression Visionary does not follow
- `src/shizue/acmd/specialhi.rs:22` shizue/game_specialairhi `GENERATE_ARTICLE_BOUNCEPAD` (change_status_exist) — an expression Visionary does not follow
- `src/shizue/acmd/speciallw.rs:18` shizue/game_specialairlwset `GENERATE_ARTICLE_WARPDOOR` (generate_article) — an expression Visionary does not follow
- `src/shizue/acmd/specialn.rs:15` shizue/game_specialairn `*FIGHTER_SHIZUE_GENERATE_ARTICLE_POT` (generate_article) — environment: shizue is not in the local dump, so its vanilla `pot` is not listed
- `src/shizue/acmd/specialhi.rs:21` shizue/game_specialhi `GENERATE_ARTICLE_BOUNCEPAD` (generate_article) — an expression Visionary does not follow
- `src/shizue/acmd/specialhi.rs:22` shizue/game_specialhi `GENERATE_ARTICLE_BOUNCEPAD` (change_status_exist) — an expression Visionary does not follow
- `src/shizue/acmd/speciallw.rs:18` shizue/game_speciallwset `GENERATE_ARTICLE_WARPDOOR` (generate_article) — an expression Visionary does not follow
- `src/shizue/acmd/specialn.rs:15` shizue/game_specialn `*FIGHTER_SHIZUE_GENERATE_ARTICLE_POT` (generate_article) — environment: shizue is not in the local dump, so its vanilla `pot` is not listed

### Unresolved ids (status code) (31)

- `src/shizue/bucket.rs:30` shizue status `*FIGHTER_SHIZUE_GENERATE_ARTICLE_BUCKET` (remove_exist) — environment: shizue is not in the local dump, so its vanilla `bucket` is not listed
- `src/shizue/bucket.rs:30` shizue status `*FIGHTER_SHIZUE_GENERATE_ARTICLE_BUCKET` (remove_exist) — environment: shizue is not in the local dump, so its vanilla `bucket` is not listed
- `src/shizue/bucket.rs:30` shizue status `*FIGHTER_SHIZUE_GENERATE_ARTICLE_BUCKET` (remove_exist) — environment: shizue is not in the local dump, so its vanilla `bucket` is not listed
- `src/shizue/status/airlasso.rs:49` shizue status `*FIGHTER_SHIZUE_GENERATE_ARTICLE_FISHINGROD` (change_status_exist) — environment: shizue is not in the local dump, so its vanilla `fishingrod` is not listed
- `src/shizue/status/airlasso.rs:78` shizue status `*FIGHTER_SHIZUE_GENERATE_ARTICLE_FISHINGROD` (remove_exist) — environment: shizue is not in the local dump, so its vanilla `fishingrod` is not listed
- `src/shizue/status/airlasso.rs:89` shizue status `*FIGHTER_SHIZUE_GENERATE_ARTICLE_FISHINGROD` (change_status_exist) — environment: shizue is not in the local dump, so its vanilla `fishingrod` is not listed
- `src/shizue/status/airlasso.rs:78` shizue status `*FIGHTER_SHIZUE_GENERATE_ARTICLE_FISHINGROD` (remove_exist) — environment: shizue is not in the local dump, so its vanilla `fishingrod` is not listed
- `src/shizue/status/catch.rs:95` shizue status `*FIGHTER_SHIZUE_GENERATE_ARTICLE_FISHINGROD` (is_exist) — environment: shizue is not in the local dump, so its vanilla `fishingrod` is not listed
- `src/shizue/status/catch.rs:99` shizue status `*FIGHTER_SHIZUE_GENERATE_ARTICLE_FISHINGROD` (remove_exist) — environment: shizue is not in the local dump, so its vanilla `fishingrod` is not listed
- `src/shizue/status/catch.rs:100` shizue status `*FIGHTER_SHIZUE_GENERATE_ARTICLE_FISHINGLINE` (remove_exist) — environment: shizue is not in the local dump, so its vanilla `fishingline` is not listed
- `src/shizue/status/catch.rs:95` shizue status `*FIGHTER_SHIZUE_GENERATE_ARTICLE_FISHINGROD` (is_exist) — environment: shizue is not in the local dump, so its vanilla `fishingrod` is not listed
- `src/shizue/status/catch.rs:99` shizue status `*FIGHTER_SHIZUE_GENERATE_ARTICLE_FISHINGROD` (remove_exist) — environment: shizue is not in the local dump, so its vanilla `fishingrod` is not listed
- `src/shizue/status/catch.rs:100` shizue status `*FIGHTER_SHIZUE_GENERATE_ARTICLE_FISHINGLINE` (remove_exist) — environment: shizue is not in the local dump, so its vanilla `fishingline` is not listed
- `src/shizue/status/specialhi.rs:39` shizue status `GENERATE_ARTICLE_BOUNCEPAD` (is_exist) — an expression Visionary does not follow
- `src/shizue/status/specialhi.rs:40` shizue status `GENERATE_ARTICLE_BOUNCEPAD` (change_status) — an expression Visionary does not follow
- `src/shizue/status/specialhi.rs:101` shizue status `GENERATE_ARTICLE_BOUNCEPAD` (remove_exist) — an expression Visionary does not follow
- `src/shizue/status/speciallw.rs:13` shizue status `GENERATE_ARTICLE_WARPDOOR` (is_exist) — an expression Visionary does not follow
- `src/shizue/status/speciallw.rs:170` shizue status `GENERATE_ARTICLE_WARPDOOR` (is_exist) — an expression Visionary does not follow
- `src/shizue/status/speciallw.rs:171` shizue status `GENERATE_ARTICLE_WARPDOOR` (remove_exist) — an expression Visionary does not follow
- `src/shizue/status/speciallw.rs:217` shizue status `GENERATE_ARTICLE_WARPDOOR` (set_flag) — an expression Visionary does not follow
- `src/shizue/status/speciallw.rs:245` shizue status `GENERATE_ARTICLE_WARPDOOR` (is_exist) — an expression Visionary does not follow
- `src/shizue/status/speciallw.rs:246` shizue status `GENERATE_ARTICLE_WARPDOOR` (set_flag) — an expression Visionary does not follow
- `src/shizue/status/speciallw.rs:323` shizue status `GENERATE_ARTICLE_WARPDOOR` (set_flag) — an expression Visionary does not follow
- `src/shizue/status/speciallw.rs:245` shizue status `GENERATE_ARTICLE_WARPDOOR` (is_exist) — an expression Visionary does not follow
- `src/shizue/status/speciallw.rs:246` shizue status `GENERATE_ARTICLE_WARPDOOR` (set_flag) — an expression Visionary does not follow
- `src/shizue/status/speciallw.rs:170` shizue status `GENERATE_ARTICLE_WARPDOOR` (is_exist) — an expression Visionary does not follow
- `src/shizue/status/speciallw.rs:171` shizue status `GENERATE_ARTICLE_WARPDOOR` (remove_exist) — an expression Visionary does not follow
- `src/shizue/status/specialn.rs:233` shizue status `GENERATE_ARTICLE_WARPDOOR` (set_flag) — an expression Visionary does not follow
- `src/shizue/trafficsign.rs:8` shizue status `*FIGHTER_SHIZUE_GENERATE_ARTICLE_TRAFFICSIGN` (remove_exist) — environment: shizue is not in the local dump, so its vanilla `trafficsign` is not listed
- `src/shizue/trafficsign.rs:8` shizue status `*FIGHTER_SHIZUE_GENERATE_ARTICLE_TRAFFICSIGN` (remove_exist) — environment: shizue is not in the local dump, so its vanilla `trafficsign` is not listed
- `src/shizue/trafficsign.rs:8` shizue status `*FIGHTER_SHIZUE_GENERATE_ARTICLE_TRAFFICSIGN` (remove_exist) — environment: shizue is not in the local dump, so its vanilla `trafficsign` is not listed

### Agents no catalog lists (7)

- `shizue_bucket` (3 scripts, 0 statuses): environment: shizue is not in the local dump, so its vanilla articles are not listed
- `shizue_clayrocket` (15 scripts, 11 statuses): environment: shizue is not in the local dump, so its vanilla articles are not listed
- `shizue_cracker` (2 scripts, 10 statuses): environment: shizue is not in the local dump, so its vanilla articles are not listed
- `shizue_fishingrod` (0 scripts, 8 statuses): environment: shizue is not in the local dump, so its vanilla articles are not listed
- `shizue_pompon` (21 scripts, 1 statuses): environment: shizue is not in the local dump, so its vanilla articles are not listed
- `shizue_pot` (17 scripts, 3 statuses): environment: shizue is not in the local dump, so its vanilla articles are not listed
- `shizue_trafficsign` (3 scripts, 0 statuses): environment: shizue is not in the local dump, so its vanilla articles are not listed

### Unread param_config overrides (12)

- `src/shizue/params.rs:61` `param_config::update_int_2(FIGHTER_KIND, slots.clone(), p)` — its param index is a variable or not a literal tuple, so Visionary can't tell which param it sets
- `src/shizue/params.rs:62` `param_config::update_int_2(-LASSO_KIND, slots.clone(), p)` — its param index is a variable or not a literal tuple, so Visionary can't tell which param it sets
- `src/shizue/params.rs:63` `param_config::update_int_2(-HOOK_KIND, slots.clone(), p)` — its param index is a variable or not a literal tuple, so Visionary can't tell which param it sets
- `src/shizue/params.rs:66` `param_config::update_float_2(FIGHTER_KIND, slots.clone(), p)` — its param index is a variable or not a literal tuple, so Visionary can't tell which param it sets
- `src/shizue/params.rs:67` `param_config::update_float_2(-LASSO_KIND, slots.clone(), p)` — its param index is a variable or not a literal tuple, so Visionary can't tell which param it sets
- `src/shizue/params.rs:68` `param_config::update_float_2(-HOOK_KIND, slots.clone(), p)` — its param index is a variable or not a literal tuple, so Visionary can't tell which param it sets
- `src/shizue/params.rs:71` `param_config::update_attribute_mul_2(FIGHTER_KIND, slots.clone(), p)` — it multiplies the value instead of setting it, which the card doesn't show
- `src/shizue/params.rs:72` `param_config::update_attribute_mul_2(-LASSO_KIND, slots.clone(), p)` — it multiplies the value instead of setting it, which the card doesn't show
- `src/shizue/params.rs:73` `param_config::update_attribute_mul_2(-HOOK_KIND, slots.clone(), p)` — it multiplies the value instead of setting it, which the card doesn't show
- `src/singleslot.rs:197` `param_config::update_int_2(*FIGHTER_KIND_SHIZUE, slots.clone(), p)` — its param index is a variable or not a literal tuple, so Visionary can't tell which param it sets
- `src/singleslot.rs:200` `param_config::update_float_2(*FIGHTER_KIND_SHIZUE, slots.clone(), p)` — its param index is a variable or not a literal tuple, so Visionary can't tell which param it sets
- `src/singleslot.rs:203` `param_config::update_attribute_mul_2(*FIGHTER_KIND_SHIZUE, slots.clone(), p)` — it multiplies the value instead of setting it, which the card doesn't show

## Sceptile-Moveset

Root `C:/Users/notja/Documents/Coding/SSBU Modding/Movesets\Sceptile-Moveset`. Fighters: mario (dump).

| Article | Origin | Ids | Scripts | Rows (spawns) | Status rows | Param block | Settings |
|---|---|---|---|---|---|---|---|
| mario_leafstorm | clone of ridley/WEAPON_KIND_RIDLEY_BREATH | FIGHTER_MARIO_GENERATE_ARTICLE_LEAF_STORM | 5 | 0 (0) | 0 | present | Life: const OWNER_LINK_TIMEOUT = 10, computed `LIFETIME + EXPLODE_FRAMES`, const EXPLODE_FRAMES = 4; Speed X: const FLY_SPEED = 1, const FLY_SPEED = 1; Speed Y: literal 0, literal 0 |
| mario_bulletseed | clone of simon/WEAPON_KIND_SIMON_AXE | FIGHTER_MARIO_GENERATE_ARTICLE_BULLET_SEED | 0 | 0 (0) | 0 | present | Life: const OWNER_LINK_TIMEOUT = 10, const LIFETIME (value unknown); Size: const BULLET_SEED_MODEL_SCALE = 1.5; Speed X: computed `speed_x`, computed `speed_x`; Speed Y: computed `speed_y`, computed `speed_y` |
| mario_leechseed | clone of rockman/WEAPON_KIND_ROCKMAN_HARDKNUCKLE | FIGHTER_MARIO_GENERATE_ARTICLE_LEECH_SEED | 4 | 0 (0) | 0 | present | Life: computed `FLY_LIFETIME + STAGE_TOTAL_LIFE + EXPIRE_FRAMES`; Size: const LEECH_SEED_MODEL_SCALE = 1.8; Speed X: computed `vx`; Speed Y: computed `vy` |
| mario_healingorb | clone of palutena/WEAPON_KIND_PALUTENA_AUTOAIMBULLET | FIGHTER_MARIO_GENERATE_ARTICLE_HEALING_ORB | 4 | 0 (0) | 0 | present | Life: const OWNER_LINK_TIMEOUT = 10, const LIFETIME (value unknown); Speed X: computed `vx`; Speed Y: computed `vy` |
| mario_stick | clone of krool/WEAPON_KIND_KROOL_IRONBALL | FIGHTER_MARIO_GENERATE_ARTICLE_STICK | 3 | 0 (0) | 0 | present | Life: const ACTIVE_FRAMES = 40, const OWNER_LINK_TIMEOUT = 10, const LIFETIME (value unknown); Speed X: const THROW_SPEED_X = 2.2, const THROW_SPEED_X = 2.2; Speed Y: computed `speed_y`, const THROW_SPEED_Y = 1.1 |
| mario_sceptilem | clone of lucario/WEAPON_KIND_LUCARIO_LUCARIOM | FIGHTER_MARIO_GENERATE_ARTICLE_SCEPTILEM | 0 | 0 (0) | 0 | absent |  |

### Unresolved statuses (24)

- `plugin/src/sceptile/articles/bullet_seed/status/mod.rs:37` mario_bulletseed Pre `*WEAPON_SIMON_AXE_STATUS_KIND_HAVED` — registered as a no-op with noop_status!
- `plugin/src/sceptile/articles/bullet_seed/status/mod.rs:37` mario_bulletseed Init `*WEAPON_SIMON_AXE_STATUS_KIND_HAVED` — registered as a no-op with noop_status!
- `plugin/src/sceptile/articles/bullet_seed/status/mod.rs:37` mario_bulletseed Main `*WEAPON_SIMON_AXE_STATUS_KIND_HAVED` — registered as a no-op with noop_status!
- `plugin/src/sceptile/articles/bullet_seed/status/mod.rs:37` mario_bulletseed End `*WEAPON_SIMON_AXE_STATUS_KIND_HAVED` — registered as a no-op with noop_status!
- `plugin/src/sceptile/articles/bullet_seed/status/mod.rs:38` mario_bulletseed Pre `*WEAPON_SIMON_AXE_STATUS_KIND_HOP` — registered as a no-op with noop_status!
- `plugin/src/sceptile/articles/bullet_seed/status/mod.rs:38` mario_bulletseed Init `*WEAPON_SIMON_AXE_STATUS_KIND_HOP` — registered as a no-op with noop_status!
- `plugin/src/sceptile/articles/bullet_seed/status/mod.rs:38` mario_bulletseed Main `*WEAPON_SIMON_AXE_STATUS_KIND_HOP` — registered as a no-op with noop_status!
- `plugin/src/sceptile/articles/bullet_seed/status/mod.rs:38` mario_bulletseed End `*WEAPON_SIMON_AXE_STATUS_KIND_HOP` — registered as a no-op with noop_status!
- `plugin/src/sceptile/articles/leaf_storm/status/mod.rs:36` mario_leafstorm Pre `*WEAPON_RIDLEY_BREATH_STATUS_KIND_VANISH` — registered as a no-op with noop_status!
- `plugin/src/sceptile/articles/leaf_storm/status/mod.rs:36` mario_leafstorm Init `*WEAPON_RIDLEY_BREATH_STATUS_KIND_VANISH` — registered as a no-op with noop_status!
- `plugin/src/sceptile/articles/leaf_storm/status/mod.rs:36` mario_leafstorm Main `*WEAPON_RIDLEY_BREATH_STATUS_KIND_VANISH` — registered as a no-op with noop_status!
- `plugin/src/sceptile/articles/leaf_storm/status/mod.rs:36` mario_leafstorm End `*WEAPON_RIDLEY_BREATH_STATUS_KIND_VANISH` — registered as a no-op with noop_status!
- `plugin/src/sceptile/articles/leech_seed/status/mod.rs:36` mario_leechseed Pre `*WEAPON_ROCKMAN_HARDKNUCKLE_STATUS_KIND_STICK` — registered as a no-op with noop_status!
- `plugin/src/sceptile/articles/leech_seed/status/mod.rs:36` mario_leechseed Init `*WEAPON_ROCKMAN_HARDKNUCKLE_STATUS_KIND_STICK` — registered as a no-op with noop_status!
- `plugin/src/sceptile/articles/leech_seed/status/mod.rs:36` mario_leechseed Main `*WEAPON_ROCKMAN_HARDKNUCKLE_STATUS_KIND_STICK` — registered as a no-op with noop_status!
- `plugin/src/sceptile/articles/leech_seed/status/mod.rs:36` mario_leechseed End `*WEAPON_ROCKMAN_HARDKNUCKLE_STATUS_KIND_STICK` — registered as a no-op with noop_status!
- `plugin/src/sceptile/articles/sceptilem/status.rs:85` mario_sceptilem Pre `kind` — registered as a no-op with noop_status!
- `plugin/src/sceptile/articles/sceptilem/status.rs:85` mario_sceptilem Init `kind` — registered as a no-op with noop_status!
- `plugin/src/sceptile/articles/sceptilem/status.rs:85` mario_sceptilem Main `kind` — registered as a no-op with noop_status!
- `plugin/src/sceptile/articles/sceptilem/status.rs:85` mario_sceptilem End `kind` — registered as a no-op with noop_status!
- `plugin/src/sceptile/articles/stick/status/mod.rs:31` mario_stick Pre `kind` — registered as a no-op with noop_status!
- `plugin/src/sceptile/articles/stick/status/mod.rs:31` mario_stick Init `kind` — registered as a no-op with noop_status!
- `plugin/src/sceptile/articles/stick/status/mod.rs:31` mario_stick Main `kind` — registered as a no-op with noop_status!
- `plugin/src/sceptile/articles/stick/status/mod.rs:31` mario_stick End `kind` — registered as a no-op with noop_status!

## Shadow-The-Hedgehog

Root `C:/Users/notja/Documents/Coding/SSBU Modding/Movesets\Shadow-The-Hedgehog`. Fighters: wolf (not in the dump).

| Article | Origin | Ids | Scripts | Rows (spawns) | Status rows | Param block | Settings |
|---|---|---|---|---|---|---|---|
| wolf_blaster_bullet | rewritten | FIGHTER_WOLF_GENERATE_ARTICLE_BLASTER_BULLET | 2 | 2 (2) | 0 | no param file |  |

### Unread param_config overrides (31)

- `src/params.rs:14` `param_config::update_attribute_mul_2(*FIGHTER_KIND_WOLF, marked_slots.clone(), (smash::hash40("walk_accel_mul"), 0, 0.105/0.105))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/params.rs:15` `param_config::update_attribute_mul_2(*FIGHTER_KIND_WOLF, marked_slots.clone(), (smash::hash40("walk_accel_add"), 0, 0.105/0.105))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/params.rs:16` `param_config::update_attribute_mul_2(*FIGHTER_KIND_WOLF, marked_slots.clone(), (smash::hash40("walk_slow_speed_mul"), 0, 0.2/0.2))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/params.rs:17` `param_config::update_attribute_mul_2(*FIGHTER_KIND_WOLF, marked_slots.clone(), (smash::hash40("walk_middle_ratio"), 0, 0.5/0.5))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/params.rs:18` `param_config::update_attribute_mul_2(*FIGHTER_KIND_WOLF, marked_slots.clone(), (smash::hash40("walk_fast_ratio"), 0, 0.75/0.75))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/params.rs:19` `param_config::update_attribute_mul_2(*FIGHTER_KIND_WOLF, marked_slots.clone(), (smash::hash40("walk_speed_max"), 0, 1.44/1.208))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/params.rs:20` `param_config::update_attribute_mul_2(*FIGHTER_KIND_WOLF, marked_slots.clone(), (smash::hash40("ground_brake"), 0, 0.055/0.11))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/params.rs:21` `param_config::update_attribute_mul_2(*FIGHTER_KIND_WOLF, marked_slots.clone(), (smash::hash40("dash_speed"), 0, 1.8/2.09))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/params.rs:22` `param_config::update_attribute_mul_2(*FIGHTER_KIND_WOLF, marked_slots.clone(), (smash::hash40("run_accel_mul"), 0, 0.115/0.10978))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/params.rs:23` `param_config::update_attribute_mul_2(*FIGHTER_KIND_WOLF, marked_slots.clone(), (smash::hash40("run_accel_add"), 0, 0.02/0.044))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/params.rs:24` `param_config::update_attribute_mul_2(*FIGHTER_KIND_WOLF, marked_slots.clone(), (smash::hash40("run_speed_max"), 0, 3.9/1.54))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/params.rs:25` `param_config::update_attribute_mul_2(*FIGHTER_KIND_WOLF, marked_slots.clone(), (smash::hash40("jump_speed_x"), 0, 0.91/1.0))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/params.rs:26` `param_config::update_attribute_mul_2(*FIGHTER_KIND_WOLF, marked_slots.clone(), (smash::hash40("jump_speed_x_mul"), 0, 1.2/0.8))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/params.rs:27` `param_config::update_attribute_mul_2(*FIGHTER_KIND_WOLF, marked_slots.clone(), (smash::hash40("jump_speed_x_max"), 0, 1.7/1.3))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/params.rs:28` `param_config::update_attribute_mul_2(*FIGHTER_KIND_WOLF, marked_slots.clone(), (smash::hash40("jump_aerial_speed_x_max"), 0, 0.9/0.9))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/params.rs:29` `param_config::update_attribute_mul_2(*FIGHTER_KIND_WOLF, marked_slots.clone(), (smash::hash40("jump_y"), 0, 35.0/32.02))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/params.rs:30` `param_config::update_attribute_mul_2(*FIGHTER_KIND_WOLF, marked_slots.clone(), (smash::hash40("jump_aerial_y"), 0, 35.0/30.71))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/params.rs:31` `param_config::update_attribute_mul_2(*FIGHTER_KIND_WOLF, marked_slots.clone(), (smash::hash40("jump_initial_y"), 0, 19.25/17.611))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/params.rs:32` `param_config::update_attribute_mul_2(*FIGHTER_KIND_WOLF, marked_slots.clone(), (smash::hash40("mini_jump_y"), 0, 16.0/15.38))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/params.rs:33` `param_config::update_attribute_mul_2(*FIGHTER_KIND_WOLF, marked_slots.clone(), (smash::hash40("air_accel_x_mul"), 0, 0.04/0.08))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/params.rs:34` `param_config::update_attribute_mul_2(*FIGHTER_KIND_WOLF, marked_slots.clone(), (smash::hash40("air_accel_x_add"), 0, 0.01/0.01))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/params.rs:35` `param_config::update_attribute_mul_2(*FIGHTER_KIND_WOLF, marked_slots.clone(), (smash::hash40("air_accel_x_stable"), 0, 1.208/1.281))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/params.rs:36` `param_config::update_attribute_mul_2(*FIGHTER_KIND_WOLF, marked_slots.clone(), (smash::hash40("air_brake_x"), 0, 0.01/0.004))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/params.rs:37` `param_config::update_attribute_mul_2(*FIGHTER_KIND_WOLF, marked_slots.clone(), (smash::hash40("dive_speed_y"), 0, 3.0/2.88))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/params.rs:38` `param_config::update_attribute_mul_2(*FIGHTER_KIND_WOLF, marked_slots.clone(), (smash::hash40("damage_fly_top_air_accel_y"), 0, 0.070896/0.072912))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/params.rs:39` `param_config::update_attribute_mul_2(*FIGHTER_KIND_WOLF, marked_slots.clone(), (smash::hash40("damage_fly_top_air_accel_y_stable"), 0, 1.8/1.8))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/params.rs:40` `param_config::update_attribute_mul_2(*FIGHTER_KIND_WOLF, marked_slots.clone(), (smash::hash40("air_break_y"), 0, 0.01/0.01))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/params.rs:41` `param_config::update_attribute_mul_2(*FIGHTER_KIND_WOLF, marked_slots.clone(), (smash::hash40("air_accel_y"), 0, 0.12/0.13))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/params.rs:42` `param_config::update_attribute_mul_2(*FIGHTER_KIND_WOLF, marked_slots.clone(), (smash::hash40("air_accel_y_stable"), 0, 1.8/1.65))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/params.rs:43` `param_config::update_attribute_mul_2(*FIGHTER_KIND_WOLF, marked_slots.clone(), (smash::hash40("weight"), 0, 86.0/92.0))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/params.rs:44` `param_config::update_attribute_mul_2(*FIGHTER_KIND_WOLF, marked_slots.clone(), (smash::hash40("shield_break_y"), 0, 39.0/44.0))` — it multiplies the value instead of setting it, which the card doesn't show

## Shigaraki

Root `C:/Users/notja/Documents/Coding/SSBU Modding/Movesets\Shigaraki`. Fighters: edge (dump).

| Article | Origin | Ids | Scripts | Rows (spawns) | Status rows | Param block | Settings |
|---|---|---|---|---|---|---|---|

## Shy-Guy-Moveset

Root `C:/Users/notja/Documents/Coding/SSBU Modding/Movesets\Shy-Guy-Moveset`. Fighters: pacman (dump).

| Article | Origin | Ids | Scripts | Rows (spawns) | Status rows | Param block | Settings |
|---|---|---|---|---|---|---|---|
| pacman_bigpacman | rewritten | FIGHTER_PACMAN_GENERATE_ARTICLE_BIGPACMAN | 7 | 1 (0) | 0 | present | Life: const LIFETIME_FRAMES (value unknown), const FRYGUY_HOP_LIFETIME = 300; Speed X: const LAUNCH_VX = 0.5, const LAUNCH_VX = 0.5, computed `vx`, computed `vx`; Speed Y: const LAUNCH_VY (value unknown), computed `new_vy`, computed `vy`, computed `vy` |
| pacman_firehydrant | rewritten | FIGHTER_PACMAN_GENERATE_ARTICLE_FIREHYDRANT | 4 | 0 (0) | 1 | present | Life: const CARPET_LIFETIME_FRAMES = 500, const STILTS_LIFETIME_FRAMES = 30; Size: const MODEL_SCALE = 0.8, const MODEL_SCALE = 0.8, const MODEL_SCALE = 0.8; Speed X: const CARPET_TRAVEL_SPEED = 0.4, const CARPET_TRAVEL_SPEED = 0.4; Speed Y: literal 0, const CARPET_SINK_SPEED = -0.4 |
| pacman_trampoline | rewritten | FIGHTER_PACMAN_GENERATE_ARTICLE_TRAMPOLINE | 6 | 0 (0) | 0 | present | Life: const FIREBALL_MAX_LIFETIME = 180, const LIFETIME_FRAMES (value unknown); Size: const JACKBOX_SCALE = 5, const FIREBALL_SCALE = 1.4, const PELLET_SCALE = 0.6; Speed X: const FIREBALL_TRAVEL_SPEED = 2.6, const FIREBALL_TRAVEL_SPEED = 2.6, const PELLET_VX = 3, const PELLET_VX = 3; Speed Y: literal 0, literal 0, const PELLET_VY = 0, const PELLET_VY = 0 |

### Unresolved statuses (21)

- `plugin/src/shyguy/articles/bigpacman/status/mod.rs:52` pacman_bigpacman Pre `*WEAPON_PACMAN_BIGPACMAN_STATUS_KIND_START` — registered as a no-op with noop_status!
- `plugin/src/shyguy/articles/bigpacman/status/mod.rs:52` pacman_bigpacman End `*WEAPON_PACMAN_BIGPACMAN_STATUS_KIND_START` — registered as a no-op with noop_status!
- `plugin/src/shyguy/articles/bigpacman/status/mod.rs:53` pacman_bigpacman Pre `*WEAPON_PACMAN_BIGPACMAN_STATUS_KIND_MOVE` — registered as a no-op with noop_status!
- `plugin/src/shyguy/articles/bigpacman/status/mod.rs:53` pacman_bigpacman Main `*WEAPON_PACMAN_BIGPACMAN_STATUS_KIND_MOVE` — registered as a no-op with noop_status!
- `plugin/src/shyguy/articles/bigpacman/status/mod.rs:53` pacman_bigpacman End `*WEAPON_PACMAN_BIGPACMAN_STATUS_KIND_MOVE` — registered as a no-op with noop_status!
- `plugin/src/shyguy/articles/bigpacman/status/mod.rs:53` pacman_bigpacman Exec `*WEAPON_PACMAN_BIGPACMAN_STATUS_KIND_MOVE` — registered as a no-op with noop_status!
- `plugin/src/shyguy/articles/bigpacman/status/mod.rs:54` pacman_bigpacman Pre `*WEAPON_PACMAN_BIGPACMAN_STATUS_KIND_ENTRY` — registered as a no-op with noop_status!
- `plugin/src/shyguy/articles/bigpacman/status/mod.rs:54` pacman_bigpacman Main `*WEAPON_PACMAN_BIGPACMAN_STATUS_KIND_ENTRY` — registered as a no-op with noop_status!
- `plugin/src/shyguy/articles/bigpacman/status/mod.rs:54` pacman_bigpacman End `*WEAPON_PACMAN_BIGPACMAN_STATUS_KIND_ENTRY` — registered as a no-op with noop_status!
- `plugin/src/shyguy/articles/bigpacman/status/mod.rs:55` pacman_bigpacman Pre `*WEAPON_PACMAN_BIGPACMAN_STATUS_KIND_EAT` — registered as a no-op with noop_status!
- `plugin/src/shyguy/articles/bigpacman/status/mod.rs:55` pacman_bigpacman Main `*WEAPON_PACMAN_BIGPACMAN_STATUS_KIND_EAT` — registered as a no-op with noop_status!
- `plugin/src/shyguy/articles/bigpacman/status/mod.rs:55` pacman_bigpacman End `*WEAPON_PACMAN_BIGPACMAN_STATUS_KIND_EAT` — registered as a no-op with noop_status!
- `plugin/src/shyguy/articles/trampoline/status/mod.rs:13` pacman_trampoline Pre `*WEAPON_PACMAN_TRAMPOLINE_STATUS_KIND_START` — registered as a no-op with noop_status!
- `plugin/src/shyguy/articles/trampoline/status/mod.rs:13` pacman_trampoline Main `*WEAPON_PACMAN_TRAMPOLINE_STATUS_KIND_START` — registered as a no-op with noop_status!
- `plugin/src/shyguy/articles/trampoline/status/mod.rs:13` pacman_trampoline End `*WEAPON_PACMAN_TRAMPOLINE_STATUS_KIND_START` — registered as a no-op with noop_status!
- `plugin/src/shyguy/articles/trampoline/status/mod.rs:14` pacman_trampoline Pre `*WEAPON_PACMAN_TRAMPOLINE_STATUS_KIND_BREAK` — registered as a no-op with noop_status!
- `plugin/src/shyguy/articles/trampoline/status/mod.rs:14` pacman_trampoline Main `*WEAPON_PACMAN_TRAMPOLINE_STATUS_KIND_BREAK` — registered as a no-op with noop_status!
- `plugin/src/shyguy/articles/trampoline/status/mod.rs:14` pacman_trampoline End `*WEAPON_PACMAN_TRAMPOLINE_STATUS_KIND_BREAK` — registered as a no-op with noop_status!
- `plugin/src/shyguy/pacman/status/special_hi.rs:504` pacman Pre `*FIGHTER_PACMAN_STATUS_KIND_SPECIAL_HI_LOOP` — registered as a no-op with noop_status!
- `plugin/src/shyguy/pacman/status/special_hi.rs:504` pacman Main `*FIGHTER_PACMAN_STATUS_KIND_SPECIAL_HI_LOOP` — registered as a no-op with noop_status!
- `plugin/src/shyguy/pacman/status/special_hi.rs:504` pacman End `*FIGHTER_PACMAN_STATUS_KIND_SPECIAL_HI_LOOP` — registered as a no-op with noop_status!

### Unread param_config overrides (3)

- `plugin/src/common/tether.rs:165` `param_config::update_int(kind, slots.clone(), (hash40("air_lasso_type"), 0), 1)` — the kind `kind` isn't a fighter or weapon kind Visionary can name
- `plugin/src/common/tether.rs:172` `param_config::update_int(kind, slots.clone(), index, value)` — its param index is a variable or not a literal tuple, so Visionary can't tell which param it sets
- `plugin/src/common/tether.rs:173` `param_config::update_float(kind, slots.clone(), index, value)` — its param index is a variable or not a literal tuple, so Visionary can't tell which param it sets

## Skull-Kid-Moveset

Root `C:/Users/notja/Documents/Coding/SSBU Modding/Movesets\Skull-Kid-Moveset`. Fighters: .

| Article | Origin | Ids | Scripts | Rows (spawns) | Status rows | Param block | Settings |
|---|---|---|---|---|---|---|---|

## Springtrap-Moveset

Root `C:/Users/notja/Documents/Coding/SSBU Modding/Movesets\Springtrap-Moveset`. Fighters: ganon (not in the dump).

| Article | Origin | Ids | Scripts | Rows (spawns) | Status rows | Param block | Settings |
|---|---|---|---|---|---|---|---|
| ganon_ironballcloned | clone of krool/WEAPON_KIND_KROOL_IRONBALL | FIGHTER_SPRINGTRAP_GENERATE_ARTICLE_AXE | 8 | 4 (4) | 0 | no param file | Acceleration X: computed `brake (reads param_axe.brake_x_max, param_axe.brake_x_min)`, literal 0, literal 0, literal 0, literal 0.03, literal 0; Acceleration Y: literal 0, literal 0.03, literal 0; Gravity: computed `gravity (reads param_axe.gravity_max, param_axe.gravity_min)`, computed `gravity (reads param_axe.gravity_max, param_axe.gravity_min)`, computed `gravity (reads param_axe.gravity_max, param_axe.gravity_min)`; Life: param param_axe/life, param param_axe/life, literal 900; Other movement: computed `sv_kinetic_energy!(set_speed, weapon, *WEAPON_KINETIC_ENERGY_RESERVE_I`, computed `sv_kinetic_energy!(set_speed, weapon, *WEAPON_KINETIC_ENERGY_RESERVE_I`; Size: literal 0.73, literal 0.73; Speed X: computed `speed_x (reads param_axe.speed_x_max, param_axe.speed_x_min)`, literal 0, computed `x_dist/20.0`, computed `x_speed`, literal 0; Speed Y: computed `speed_y (reads param_axe.speed_y_max, param_axe.speed_y_min)`, literal 0, computed `y_dist/7.5`, computed `y_dist/7.5`, literal 0 |
| ganon_phantom | clone of koopajr/WEAPON_KIND_KOOPAJR_CANNONBALL | FIGHTER_SPRINGTRAP_GENERATE_ARTICLE_PHANTOM | 30 | 2 (2) | 0 | no param file | Acceleration X: literal 0, literal 0, literal 0, literal 0, literal 0, literal 0, literal 0, literal 0, literal 0, literal 0, literal 0, literal 0, literal 0, literal 0; Acceleration Y: literal 0, literal 0, literal 0, literal 0, literal 0, literal 0, literal 0, literal 0, literal 0, literal 0, literal 0; Gravity: literal 0.065, literal 0.07, literal 0.07; Life: literal 1800, literal 80, literal 40, literal 100, literal 4, literal 10, literal 10, literal 640, literal 340; Max speed X: literal 0, literal 0; Max speed Y: literal 3, literal 3; Other movement: computed `sv_kinetic_energy!(set_brake, weapon, *WEAPON_KINETIC_ENERGY_RESERVE_I`; Size: literal 0.001, literal 1.75; Speed X: computed `speed_x`, literal 0, literal 0, literal 0, computed `-(current_x_speed*0.1)`, computed `0.5*owner_init_lr`, computed `current_x_speed*10.0`, literal 0, literal 0, literal 0, computed `-(current_x_speed*0.1)`, computed `0.25*owner_init_lr`, computed `current_x_speed*10.0`, literal 0, literal 0, literal 0; Speed Y: computed `speed_y`, literal 0, literal 0, literal 0, literal 0, literal 0, literal 0, literal 0, literal 0, literal 0, literal 0, literal 0, literal 0, literal 0, literal 0, literal 0 |

### Unresolved ids (scripts) (23)

- `src/springtrap/acmd/aerials.rs:215` ganon/game_attackairb `*FIGHTER_GANON_GENERATE_ARTICLE_SWORD` (remove_exist) — environment: ganon is not in the local dump, so its vanilla `sword` is not listed
- `src/springtrap/acmd/aerials.rs:99` ganon/game_attackairf `*FIGHTER_GANON_GENERATE_ARTICLE_SWORD` (generate_article) — environment: ganon is not in the local dump, so its vanilla `sword` is not listed
- `src/springtrap/acmd/aerials.rs:106` ganon/game_attackairf `*FIGHTER_GANON_GENERATE_ARTICLE_SWORD` (remove_exist) — environment: ganon is not in the local dump, so its vanilla `sword` is not listed
- `src/springtrap/acmd/aerials.rs:106` ganon/game_attackairf `*FIGHTER_GANON_GENERATE_ARTICLE_SWORD` (remove_exist) — environment: ganon is not in the local dump, so its vanilla `sword` is not listed
- `src/springtrap/acmd/aerials.rs:281` ganon/game_attackairhi `*FIGHTER_GANON_GENERATE_ARTICLE_SWORD` (remove_exist) — environment: ganon is not in the local dump, so its vanilla `sword` is not listed
- `src/springtrap/acmd/aerials.rs:343` ganon/game_attackairlw `*FIGHTER_GANON_GENERATE_ARTICLE_SWORD` (remove_exist) — environment: ganon is not in the local dump, so its vanilla `sword` is not listed
- `src/springtrap/acmd/aerials.rs:7` ganon/game_attackairn `*FIGHTER_GANON_GENERATE_ARTICLE_SWORD` (remove_exist) — environment: ganon is not in the local dump, so its vanilla `sword` is not listed
- `src/springtrap/acmd/smashes.rs:54` ganon/game_attacks4 `*FIGHTER_GANON_GENERATE_ARTICLE_SWORD` (generate_article) — environment: ganon is not in the local dump, so its vanilla `sword` is not listed
- `src/springtrap/acmd/smashes.rs:59` ganon/game_attacks4 `*FIGHTER_GANON_GENERATE_ARTICLE_SWORD` (remove_exist) — environment: ganon is not in the local dump, so its vanilla `sword` is not listed
- `src/springtrap/acmd/smashes.rs:59` ganon/game_attacks4 `*FIGHTER_GANON_GENERATE_ARTICLE_SWORD` (remove_exist) — environment: ganon is not in the local dump, so its vanilla `sword` is not listed
- `src/springtrap/acmd/specials/special_n_high_fire.rs:9` ganon/game_specialairnhighfire `*FIGHTER_GANON_GENERATE_ARTICLE_SWORD` (remove_exist) — environment: ganon is not in the local dump, so its vanilla `sword` is not listed
- `src/springtrap/acmd/specials/special_n_low_fire.rs:12` ganon/game_specialairnlowfire `*FIGHTER_GANON_GENERATE_ARTICLE_SWORD` (remove_exist) — environment: ganon is not in the local dump, so its vanilla `sword` is not listed
- `src/springtrap/acmd/specials/special_n_recall_end.rs:18` ganon/game_specialairnrecallend `*FIGHTER_GANON_GENERATE_ARTICLE_SWORD` (remove_exist) — environment: ganon is not in the local dump, so its vanilla `sword` is not listed
- `src/springtrap/acmd/specials/special_n_recall_end.rs:18` ganon/game_specialairnrecallend `*FIGHTER_GANON_GENERATE_ARTICLE_SWORD` (remove_exist) — environment: ganon is not in the local dump, so its vanilla `sword` is not listed
- `src/springtrap/acmd/specials/special_n_start.rs:7` ganon/game_specialairnstart `*FIGHTER_GANON_GENERATE_ARTICLE_SWORD` (generate_article) — environment: ganon is not in the local dump, so its vanilla `sword` is not listed
- `src/springtrap/acmd/specials/special_n_high_fire.rs:9` ganon/game_specialnhighfire `*FIGHTER_GANON_GENERATE_ARTICLE_SWORD` (remove_exist) — environment: ganon is not in the local dump, so its vanilla `sword` is not listed
- `src/springtrap/acmd/specials/special_n_low_fire.rs:12` ganon/game_specialnlowfire `*FIGHTER_GANON_GENERATE_ARTICLE_SWORD` (remove_exist) — environment: ganon is not in the local dump, so its vanilla `sword` is not listed
- `src/springtrap/acmd/specials/special_n_recall_end.rs:18` ganon/game_specialnrecallend `*FIGHTER_GANON_GENERATE_ARTICLE_SWORD` (remove_exist) — environment: ganon is not in the local dump, so its vanilla `sword` is not listed
- `src/springtrap/acmd/specials/special_n_recall_end.rs:18` ganon/game_specialnrecallend `*FIGHTER_GANON_GENERATE_ARTICLE_SWORD` (remove_exist) — environment: ganon is not in the local dump, so its vanilla `sword` is not listed
- `src/springtrap/acmd/specials/special_n_start.rs:7` ganon/game_specialnstart `*FIGHTER_GANON_GENERATE_ARTICLE_SWORD` (generate_article) — environment: ganon is not in the local dump, so its vanilla `sword` is not listed
- `src/springtrap/acmd/other/win_1.rs:8` ganon/game_win1 `*FIGHTER_GANON_GENERATE_ARTICLE_SWORD` (generate_article) — environment: ganon is not in the local dump, so its vanilla `sword` is not listed
- `src/springtrap/acmd/other/win_3.rs:7` ganon/game_win3 `*FIGHTER_GANON_GENERATE_ARTICLE_SWORD` (generate_article) — environment: ganon is not in the local dump, so its vanilla `sword` is not listed
- `src/springtrap/acmd/other/win_3.rs:28` ganon/game_win3 `*FIGHTER_GANON_GENERATE_ARTICLE_SWORD` (remove_exist) — environment: ganon is not in the local dump, so its vanilla `sword` is not listed

### Unresolved ids (status code) (9)

- `src/springtrap/status/normal/attack_s4.rs:21` ganon status `*FIGHTER_GANON_GENERATE_ARTICLE_SWORD` (remove_exist) — environment: ganon is not in the local dump, so its vanilla `sword` is not listed
- `src/springtrap/status/special/special_n.rs:75` ganon status `*FIGHTER_GANON_GENERATE_ARTICLE_SWORD` (remove_exist) — environment: ganon is not in the local dump, so its vanilla `sword` is not listed
- `src/springtrap/status/special/special_n.rs:80` ganon status `*FIGHTER_GANON_GENERATE_ARTICLE_SWORD` (remove_exist) — environment: ganon is not in the local dump, so its vanilla `sword` is not listed
- `src/springtrap/status/special/special_n_high_fire.rs:77` ganon status `*FIGHTER_GANON_GENERATE_ARTICLE_SWORD` (remove_exist) — environment: ganon is not in the local dump, so its vanilla `sword` is not listed
- `src/springtrap/status/special/special_n_high_fire.rs:84` ganon status `*FIGHTER_GANON_GENERATE_ARTICLE_SWORD` (remove_exist) — environment: ganon is not in the local dump, so its vanilla `sword` is not listed
- `src/springtrap/status/special/special_n_low_fire.rs:78` ganon status `*FIGHTER_GANON_GENERATE_ARTICLE_SWORD` (remove_exist) — environment: ganon is not in the local dump, so its vanilla `sword` is not listed
- `src/springtrap/status/special/special_n_low_fire.rs:83` ganon status `*FIGHTER_GANON_GENERATE_ARTICLE_SWORD` (remove_exist) — environment: ganon is not in the local dump, so its vanilla `sword` is not listed
- `src/springtrap/status/special/special_n_recall_end.rs:73` ganon status `*FIGHTER_GANON_GENERATE_ARTICLE_SWORD` (remove_exist) — environment: ganon is not in the local dump, so its vanilla `sword` is not listed
- `src/springtrap/status/special/special_n_recall_end.rs:80` ganon status `*FIGHTER_GANON_GENERATE_ARTICLE_SWORD` (remove_exist) — environment: ganon is not in the local dump, so its vanilla `sword` is not listed

## Tails-Moveset

Root `C:/Users/notja/Documents/Coding/SSBU Modding/Movesets\Tails-Moveset`. Fighters: sonic (dump).

| Article | Origin | Ids | Scripts | Rows (spawns) | Status rows | Param block | Settings |
|---|---|---|---|---|---|---|---|
| sonic_gimmickjump | rewritten | FIGHTER_SONIC_GENERATE_ARTICLE_GIMMICKJUMP | 1 | 1 (0) | 0 | present |  |
| sonic_energyball | clone of link/WEAPON_KIND_LINK_SWORD_BEAM | FIGHTER_TAILS_GENERATE_ARTICLE_ENERGYBALL | 3 | 2 (1) | 0 | absent | Life: computed `life`, computed `life`; Other movement: computed `sv_kinetic_energy::get_speed_x(weapon.lua_state_agent)`, computed `sv_kinetic_energy::get_speed_y(weapon.lua_state_agent)`; Speed X: computed `move_speed_x`, computed `speed_x`; Speed Y: computed `-move_speed_y`, computed `speed_y` |
| sonic_tornado | clone of kirby/WEAPON_KIND_KIRBY_FINALCUTTERSHOT | FIGHTER_TAILS_GENERATE_ARTICLE_TORNADO | 3 | 2 (1) | 0 | absent | Size: computed `TORNADO_SIZE[ENTRY_ID]`, computed `TORNADO_SIZE[ENTRY_ID]`, computed `TORNADO_SIZE[ENTRY_ID]`; Speed X: computed `speed_x`; Speed Y: computed `-speed_y` |

### Unread param_config overrides (30)

- `src/lib.rs:171` `param_config::update_attribute_mul_2(*FIGHTER_KIND_SONIC, marked_slots.clone(), (smash::hash40("walk_accel_mul"), 0, 0.14/0.105))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/lib.rs:172` `param_config::update_attribute_mul_2(*FIGHTER_KIND_SONIC, marked_slots.clone(), (smash::hash40("walk_accel_add"), 0, 0.12/0.105))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/lib.rs:173` `param_config::update_attribute_mul_2(*FIGHTER_KIND_SONIC, marked_slots.clone(), (smash::hash40("walk_accel_max"), 0, 1.502/1.444))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/lib.rs:174` `param_config::update_attribute_mul_2(*FIGHTER_KIND_SONIC, marked_slots.clone(), (smash::hash40("walk_slow_speed_mul"), 0, 0.2/0.2))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/lib.rs:175` `param_config::update_attribute_mul_2(*FIGHTER_KIND_SONIC, marked_slots.clone(), (smash::hash40("walk_middle_ratio"), 0, 0.5/0.5))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/lib.rs:176` `param_config::update_attribute_mul_2(*FIGHTER_KIND_SONIC, marked_slots.clone(), (smash::hash40("walk_fast_ratio"), 0, 0.75/0.75))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/lib.rs:177` `param_config::update_attribute_mul_2(*FIGHTER_KIND_SONIC, marked_slots.clone(), (smash::hash40("ground_brake"), 0, 0.087/0.138))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/lib.rs:178` `param_config::update_attribute_mul_2(*FIGHTER_KIND_SONIC, marked_slots.clone(), (smash::hash40("dash_speed"), 0, 2.035/2.31))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/lib.rs:179` `param_config::update_attribute_mul_2(*FIGHTER_KIND_SONIC, marked_slots.clone(), (smash::hash40("run_accel_mul"), 0, 0.132/0.165))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/lib.rs:180` `param_config::update_attribute_mul_2(*FIGHTER_KIND_SONIC, marked_slots.clone(), (smash::hash40("run_accel_add"), 0, 0.044/0.044))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/lib.rs:181` `param_config::update_attribute_mul_2(*FIGHTER_KIND_SONIC, marked_slots.clone(), (smash::hash40("run_speed_max"), 0, 2.077/3.85))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/lib.rs:182` `param_config::update_attribute_mul_2(*FIGHTER_KIND_SONIC, marked_slots.clone(), (smash::hash40("jump_speed_x"), 0, 0.95/0.6))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/lib.rs:183` `param_config::update_attribute_mul_2(*FIGHTER_KIND_SONIC, marked_slots.clone(), (smash::hash40("jump_speed_x_mul"), 0, 0.8/0.5))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/lib.rs:184` `param_config::update_attribute_mul_2(*FIGHTER_KIND_SONIC, marked_slots.clone(), (smash::hash40("jump_speed_x_max"), 0, 1.3/1.0))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/lib.rs:185` `param_config::update_attribute_mul_2(*FIGHTER_KIND_SONIC, marked_slots.clone(), (smash::hash40("jump_aerial_speed_x_mul"), 0, 0.9/0.9))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/lib.rs:186` `param_config::update_attribute_mul_2(*FIGHTER_KIND_SONIC, marked_slots.clone(), (smash::hash40("jump_initial_y"), 0, 20.5/19.25))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/lib.rs:187` `param_config::update_attribute_mul_2(*FIGHTER_KIND_SONIC, marked_slots.clone(), (smash::hash40("jump_y"), 0, 38.0/35.0))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/lib.rs:188` `param_config::update_attribute_mul_2(*FIGHTER_KIND_SONIC, marked_slots.clone(), (smash::hash40("mini_jump_y"), 0, 18.0/16.89))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/lib.rs:189` `param_config::update_attribute_mul_2(*FIGHTER_KIND_SONIC, marked_slots.clone(), (smash::hash40("jump_aerial_y"), 0, 36.0/35.0))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/lib.rs:190` `param_config::update_attribute_mul_2(*FIGHTER_KIND_SONIC, marked_slots.clone(), (smash::hash40("air_accel_x_mul"), 0, 0.07/0.04))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/lib.rs:191` `param_config::update_attribute_mul_2(*FIGHTER_KIND_SONIC, marked_slots.clone(), (smash::hash40("air_accel_x_add"), 0, 0.01/0.01))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/lib.rs:192` `param_config::update_attribute_mul_2(*FIGHTER_KIND_SONIC, marked_slots.clone(), (smash::hash40("air_speed_x_stable"), 0, 1.239/1.208))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/lib.rs:193` `param_config::update_attribute_mul_2(*FIGHTER_KIND_SONIC, marked_slots.clone(), (smash::hash40("air_brake_x"), 0, 0.015/0.01))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/lib.rs:194` `param_config::update_attribute_mul_2(*FIGHTER_KIND_SONIC, marked_slots.clone(), (smash::hash40("air_accel_y"), 0, 0.075/0.09))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/lib.rs:195` `param_config::update_attribute_mul_2(*FIGHTER_KIND_SONIC, marked_slots.clone(), (smash::hash40("air_speed_y_stable"), 0, 1.8/1.65))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/lib.rs:196` `param_config::update_attribute_mul_2(*FIGHTER_KIND_SONIC, marked_slots.clone(), (smash::hash40("air_brake_y"), 0, 0.01/0.01))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/lib.rs:197` `param_config::update_attribute_mul_2(*FIGHTER_KIND_SONIC, marked_slots.clone(), (smash::hash40("dive_speed_y"), 0, 2.60/2.64))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/lib.rs:198` `param_config::update_attribute_mul_2(*FIGHTER_KIND_SONIC, marked_slots.clone(), (smash::hash40("weight"), 0, 80.0/86.0))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/lib.rs:199` `param_config::update_attribute_mul_2(*FIGHTER_KIND_SONIC, marked_slots.clone(), (smash::hash40("air_ground_speed_brake"), 0, 0.045/0.03))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/lib.rs:200` `param_config::update_attribute_mul_2(*FIGHTER_KIND_SONIC, marked_slots.clone(), (smash::hash40("shield_radius"), 0, 11.2/10.9))` — it multiplies the value instead of setting it, which the card doesn't show

## Toad-Moveset

Root `C:/Users/notja/Documents/Coding/SSBU Modding/Movesets\Toad-Moveset`. Fighters: murabito (dump).

| Article | Origin | Ids | Scripts | Rows (spawns) | Status rows | Param block | Settings |
|---|---|---|---|---|---|---|---|
| murabito_beetle | vanilla | FIGHTER_MURABITO_GENERATE_ARTICLE_BEETLE | 0 | 2 (0) | 0 | absent |  |
| murabito_bowlingball | rewritten | FIGHTER_MURABITO_GENERATE_ARTICLE_BOWLINGBALL | 1 | 2 (0) | 0 | present |  |
| murabito_bullet | rewritten | FIGHTER_MURABITO_GENERATE_ARTICLE_BULLET | 6 | 4 (2) | 0 | present |  |
| murabito_butterflynet | vanilla | FIGHTER_MURABITO_GENERATE_ARTICLE_BUTTERFLYNET | 0 | 6 (0) | 0 | absent |  |
| murabito_clayrocket | rewritten | FIGHTER_MURABITO_GENERATE_ARTICLE_CLAYROCKET | 5 | 3 (0) | 0 | present |  |
| murabito_firework | vanilla | FIGHTER_MURABITO_GENERATE_ARTICLE_FIREWORK | 0 | 2 (1) | 0 | absent |  |
| murabito_flowerpot | rewritten | FIGHTER_MURABITO_GENERATE_ARTICLE_FLOWERPOT | 8 | 4 (2) | 0 | present | Acceleration X: literal 0; Gravity: computed `gravity`; Life: literal 90; Settling speed X: computed `speed_x`, computed `speed_x`; Settling speed Y: computed `speed_y`, computed `bounce`; Size: literal 1.5; Speed X: computed `speed_x`, computed `speed_x`; Speed Y: computed `speed_y`, computed `bounce` |
| murabito_house | vanilla | FIGHTER_MURABITO_GENERATE_ARTICLE_HOUSE | 0 | 2 (0) | 0 | absent |  |
| murabito_slingshot | rewritten | FIGHTER_MURABITO_GENERATE_ARTICLE_SLINGSHOT | 6 | 2 (1) | 0 | absent |  |
| murabito_umbrella | vanilla | FIGHTER_MURABITO_GENERATE_ARTICLE_UMBRELLA | 0 | 1 (0) | 0 | absent |  |
| murabito_weeds | vanilla | FIGHTER_MURABITO_GENERATE_ARTICLE_WEEDS | 0 | 2 (1) | 0 | absent |  |

## Waluigi-Moveset

Root `C:/Users/notja/Documents/Coding/SSBU Modding/Movesets\Waluigi-Moveset\plugin`. Fighters: dolly (not in the dump).

| Article | Origin | Ids | Scripts | Rows (spawns) | Status rows | Param block | Settings |
|---|---|---|---|---|---|---|---|
| dolly_diceblock | clone of mario/WEAPON_KIND_MARIO_FIREBALL | FIGHTER_DOLLY_GENERATE_ARTICLE_DICEBLOCK | 8 | 2 (2) | 0 | no param file | Speed X: literal 0; Speed Y: literal 0 |

### Unread param_config overrides (21)

- `src/lib.rs:154` `param_config::update_attribute_mul_2(*FIGHTER_KIND_DOLLY, marked_slots.clone(), (smash::hash40("jump_y"), 0, 47.848/27.0))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/lib.rs:155` `param_config::update_attribute_mul_2(*FIGHTER_KIND_DOLLY, marked_slots.clone(), (smash::hash40("dash_speed"), 0, 1.58/1.65))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/lib.rs:156` `param_config::update_attribute_mul_2(*FIGHTER_KIND_DOLLY, marked_slots.clone(), (smash::hash40("walk_accel_mul"), 0, 0.097/0.1))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/lib.rs:157` `param_config::update_attribute_mul_2(*FIGHTER_KIND_DOLLY, marked_slots.clone(), (smash::hash40("walk_accel_add"), 0, 0.243/0.03))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/lib.rs:158` `param_config::update_attribute_mul_2(*FIGHTER_KIND_DOLLY, marked_slots.clone(), (smash::hash40("walk_speed_max"), 0, 0.84/0.85))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/lib.rs:159` `param_config::update_attribute_mul_2(*FIGHTER_KIND_DOLLY, marked_slots.clone(), (smash::hash40("ground_brake"), 0, 0.062/0.111))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/lib.rs:160` `param_config::update_attribute_mul_2(*FIGHTER_KIND_DOLLY, marked_slots.clone(), (smash::hash40("run_accel_mul"), 0, 0.12/0.12))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/lib.rs:161` `param_config::update_attribute_mul_2(*FIGHTER_KIND_DOLLY, marked_slots.clone(), (smash::hash40("run_accel_add"), 0, 0.026/0.04))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/lib.rs:162` `param_config::update_attribute_mul_2(*FIGHTER_KIND_DOLLY, marked_slots.clone(), (smash::hash40("run_speed_max"), 0, 1.67/1.72))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/lib.rs:163` `param_config::update_attribute_mul_2(*FIGHTER_KIND_DOLLY, marked_slots.clone(), (smash::hash40("jump_speed_x"), 0, 0.87/0.8))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/lib.rs:164` `param_config::update_attribute_mul_2(*FIGHTER_KIND_DOLLY, marked_slots.clone(), (smash::hash40("jump_speed_x_mul"), 0, 0.768/0.75))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/lib.rs:165` `param_config::update_attribute_mul_2(*FIGHTER_KIND_DOLLY, marked_slots.clone(), (smash::hash40("jump_speed_x_max"), 0, 2.672/1.1))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/lib.rs:166` `param_config::update_attribute_mul_2(*FIGHTER_KIND_DOLLY, marked_slots.clone(), (smash::hash40("jump_aerial_speed_x_mul"), 0, 1.089/0.8))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/lib.rs:167` `param_config::update_attribute_mul_2(*FIGHTER_KIND_DOLLY, marked_slots.clone(), (smash::hash40("air_accel_y"), 0, 0.09/0.09))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/lib.rs:168` `param_config::update_attribute_mul_2(*FIGHTER_KIND_DOLLY, marked_slots.clone(), (smash::hash40("jump_aerial_y"), 0, 28.248/29.0))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/lib.rs:169` `param_config::update_attribute_mul_2(*FIGHTER_KIND_DOLLY, marked_slots.clone(), (smash::hash40("jump_initial_y"), 0, 28.548/14.85))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/lib.rs:170` `param_config::update_attribute_mul_2(*FIGHTER_KIND_DOLLY, marked_slots.clone(), (smash::hash40("mini_jump_y"), 0, 15.155/15.2))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/lib.rs:171` `param_config::update_attribute_mul_2(*FIGHTER_KIND_DOLLY, marked_slots.clone(), (smash::hash40("air_accel_x_mul"), 0, 0.085/0.05))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/lib.rs:172` `param_config::update_attribute_mul_2(*FIGHTER_KIND_DOLLY, marked_slots.clone(), (smash::hash40("air_accel_x_add"), 0, 0.02/0.01))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/lib.rs:173` `param_config::update_attribute_mul_2(*FIGHTER_KIND_DOLLY, marked_slots.clone(), (smash::hash40("dive_speed_y"), 0, 2.2/2.368))` — it multiplies the value instead of setting it, which the card doesn't show
- `src/lib.rs:174` `param_config::update_attribute_mul_2(*FIGHTER_KIND_DOLLY, marked_slots.clone(), (smash::hash40("weight"), 0, 93.0/108.0))` — it multiplies the value instead of setting it, which the card doesn't show

## joob-ssbu-moveset-template

Root `C:/Users/notja/Documents/Coding/SSBU Modding/Movesets\joob-ssbu-moveset-template`. Fighters: .

| Article | Origin | Ids | Scripts | Rows (spawns) | Status rows | Param block | Settings |
|---|---|---|---|---|---|---|---|

### Unresolved statuses (1)

- `plugin/src/template/frame.rs:40` (no agent) Start `` — the agent is a parameter and no single module installs it from an Agent::new, or several agents claim the module

## lily-srs-template

Root `C:/Users/notja/Documents/Coding/SSBU Modding/Movesets\lily-srs-template`. Fighters: mario (dump).

| Article | Origin | Ids | Scripts | Rows (spawns) | Status rows | Param block | Settings |
|---|---|---|---|---|---|---|---|

## ssbuPhosphoraMoveset

Root `C:/Users/notja/Documents/Coding/SSBU Modding/Movesets\ssbuPhosphoraMoveset`. Fighters: zelda (not in the dump).

| Article | Origin | Ids | Scripts | Rows (spawns) | Status rows | Param block | Settings |
|---|---|---|---|---|---|---|---|

### Unresolved ids (scripts) (6)

- `src/specials/speciallw/mod.rs:95` zelda/game_specialairlw_phosphora `*FIGHTER_ZELDA_GENERATE_ARTICLE_DEIN` (generate_article) — environment: zelda is not in the local dump, so its vanilla `dein` is not listed
- `src/specials/specialn/mod.rs:96` zelda/game_specialairn_phosphora `*FIGHTER_ZELDA_GENERATE_ARTICLE_DEIN` (generate_article) — environment: zelda is not in the local dump, so its vanilla `dein` is not listed
- `src/specials/specials/mod.rs:9` zelda/game_specialairsstart_phosphora `*FIGHTER_ZELDA_GENERATE_ARTICLE_DEIN` (generate_article) — environment: zelda is not in the local dump, so its vanilla `dein` is not listed
- `src/specials/speciallw/mod.rs:95` zelda/game_speciallw_phosphora `*FIGHTER_ZELDA_GENERATE_ARTICLE_DEIN` (generate_article) — environment: zelda is not in the local dump, so its vanilla `dein` is not listed
- `src/specials/specialn/mod.rs:96` zelda/game_specialn_phosphora `*FIGHTER_ZELDA_GENERATE_ARTICLE_DEIN` (generate_article) — environment: zelda is not in the local dump, so its vanilla `dein` is not listed
- `src/specials/specials/mod.rs:9` zelda/game_specialsstart_phosphora `*FIGHTER_ZELDA_GENERATE_ARTICLE_DEIN` (generate_article) — environment: zelda is not in the local dump, so its vanilla `dein` is not listed

### Agents no catalog lists (2)

- `zelda_dein` (2 scripts, 3 statuses): environment: zelda is not in the local dump, so its vanilla articles are not listed
- `zelda_dein_s` (3 scripts, 0 statuses): environment: zelda is not in the local dump, so its vanilla articles are not listed
