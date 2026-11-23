# Articles (fighter objects), vanilla and moveset, end to end

Date: 2026-11-20. Branch base: `merge-upstream-0.3` (upstream Common-Leap/Visionary `352dcdd`, v0.3.2, merged in).

## Intent

joobert wants every article a real moveset makes (projectiles, held weapons, summoned helpers,
cloned weapons) to work in Visionary as fully as a fighter's own move does: found, shown,
previewed, editable and saved back into the moveset's source, and sent live to the game. The
acceptance bar is the movesets under `SSBU Modding/Movesets`: open any of them and every
article it makes is there, attached to the right owner and source, with the settings that
shape it (size, speed, life, gravity, the effects that follow it, its hitboxes) visible and
editable, whether the moveset writes them as a param, a constant, or a function call.

"It needs to be perfect": the bar is that nothing a moveset writes about an article is lost,
misattributed or silently wrong. Where Visionary cannot model something exactly, it says so
in plain words and points at the source line instead of guessing.

## Where things stand (on the merged base)

Upstream already brought a **vanilla-oriented object preview** (`src/article_preview.rs`,
`app.rs::preview_objects`, `data.rs` walk):

- Spawn events come from the move's game/expression/sound scripts: `generate_article`,
  `remove`/`remove_exist`, `shoot`, `change_motion`, `set_visibility_whole`
  (`data.rs` ~6898 and ~7151, `record_native_article_call`).
- An object's model/skeleton/motions are loaded from `fighter/<owner>/model|motion/<name>`;
  its flight (`Flight`: speed, angle, gravity, life, bounce, accel, max speed, return) from
  `param_<name>` in the fighter's param file; its hitboxes and effects from its own game
  script **fetched from the vanilla dump** (`acmd::fetch_object_script`, GitHub).
- Held/carried weapons, Inkling, items, per-size/stage speeds (559c5f7), effects on bones.

Gaps against the moveset corpus (31 movesets with articles; ~680 `generate_article`,
77 `clone_weapon`, ~470 article agents):

1. **Spawn symbols must start with `*`** (a dereferenced `lua_const`). Moveset clone IDs are
   plain statics (`FIGHTER_SAMUS_GENERATE_ARTICLE_LATIVOLT`, `FIGHTER_GOOSE_GENERATE_ARTICLE_BREAD`),
   locals (`generate_id`), or expressions; all are dropped today.
2. **Object scripts come only from the vanilla dump.** The moveset's own article agents
   (`Agent::new("falco_cannon")`, already indexed by `acmd_src::SourceIndex` under that key)
   are never used, so a clone or a rewritten article shows vanilla hitboxes or none.
3. **No notion of clones.** `smashline::clone_weapon` is not read, in either signature:
   - old (smashline `df194a0`/`9f36c5f`): `(src_owner, src_kind, new_owner, new_name, use_original_code) -> i32`,
     id captured by `+=` or `6 + …`;
   - new (`c510441`/`137aeb7`): `(new_owner, new_name, src_kind, use_original_code) -> CloneWeapon { generate_id, weapon_id }`,
     captured into `OnceLock`/statics;
   - indirections: consts (`REDSHELL_ORIGIN`, `*ACCESSORIES_KIND`), loops over tables
     (Sceptile: `clone_weapon("mario", name, source, false)`), `update_weapon_count`.
4. **Status-code settings are ignored.** In movesets, size/speed/life live in the article's
   status scripts, not params: `ModelModule::set_scale` (209 uses), `PostureModule::set_scale` (69),
   `sv_kinetic_energy!(set_speed|set_accel|…)` (~1,300), `INT_LIFE`/`INT_INIT_LIFE`/`set_life` (~1,800),
   `AttackModule::set_power_mul` (31), as literals, consts, `get_param_*` reads, or arithmetic
   (charge lerps).
5. **Call coverage.** 30 distinct `ArticleModule` functions are used; the walk models 5.
   Missing ones that change what is seen: `change_status`/`change_status_exist`, `set_rate`,
   `set_frame`, `add_motion_partial`, `shoot_exist`, `set_visibility`, `set_pos`, target
   selectors (`ARTICLE_OPE_TARGET_ALL/LAST/FIRST/TERM`).
6. **Spawns outside ACMD** (~30%: status `main`/`exec`, opff, helpers like
   `generate_stacked_article`) are invisible.
7. **No editing.** Article calls are read and drawn but are not typed, editable rows in the
   Classic editor or move nodes; nothing writes them back.
8. **Live.** The plugin deliberately skips article/weapon scripts for capture and injection
   (`hitbox_viewer/mod.rs` ~842, ~3454, ~3732; `acmd_hooks.rs` ~3512).

## Goals

- G1. Every article a linked moveset makes is listed for its owner, named plainly, with its
  origin: vanilla, rewritten vanilla, or clone of `<owner>/<article>` (with `use_original_code`).
- G2. Every spawn/control call in a move is a typed timeline row (create, remove, shoot,
  motion, status, rate/frame, visibility, position), editable and round-tripped, with its target
  selector, wherever it is written: ACMD scripts first; status/opff sites listed and previewed
  read-only with a jump to source.
- G3. Each article's own scripts (game/effect/sound/expression) from the moveset source when
  it has them, else the dump; editable in the same editors as a fighter's move.
- G4. An article's settings panel: size, speed (x/y), accel, max speed, life, gravity, power
  multiplier, plus its effects and hitboxes. Each value shows **where it comes from**
  (param `param_<name>/<field>`, a constant `FOO_SPEED`, a literal in `status/fly.rs:41`, or a
  computed expression). Literal, const and param values are editable and saved back to that
  exact place; computed ones are shown with their formula and are read-only.
- G5. The 3D preview flies the article from those resolved values (G4), not only params, at
  its scale, playing its motions and following-effects.
- G6. Live: the plugin captures and injects the selected fighter's article ACMD the same way it
  does the fighter's, so edits to an article's scripts show in game.
- G7. A corpus sweep test (below) that proves G1–G4 on every moveset under `Movesets/`.

## Non-goals

- Simulating arbitrary status logic (homing, steering, sticking, state machines). The preview
  stays a guide there and says so.
- Authoring new status code from scratch, or new article model/motion files.
- Editing `clone_weapon` registrations through a UI beyond what G1 shows (a later step; see "Left to the builder").
- Smashline 1 era movesets beyond reading them (they need porting anyway).

## Anti-goals (failure even if it "works")

- A save that changes any byte of a moveset's source the user did not edit (the project's
  existing round-trip guarantee extends to every new row and setting).
- Attributing an article to the wrong owner or source, or showing vanilla scripts for an
  article the moveset rewrote.
- A value shown without its origin, or a computed value written over with a literal.
- Article hooks in the plugin affecting other fighters' articles or the fighter's own capture
  (the existing guards stay; kinds are category-local and collide numerically).

## Approaches considered

1. **Source first:** model all layers from source, then preview. Solid, slow to show.
2. **Live first:** plugin article capture/injection. Fast to show, still blind to clones and params.
3. **Both, in phases (chosen by joobert):** phase A source model + editing, phase B live,
   phase C preview driven by the resolved settings. Phase C largely extends upstream's
   existing `article_preview`, so it builds on that rather than replacing it.

## Design

### Phase A — the article model (source)

New module `src/articles.rs` (pure, testable, no UI):

- `ArticleCatalog::build(fighter, &SourceIndex, dump roots, param file)` →
  `Vec<Article>`, `Article { owner, name, agent ("owner_name"), origin, generate_id_symbols, weapon_kind_symbol, scripts, settings, model_dir, motion_dir }`.
  - `origin`: `Vanilla`, `Rewritten` (moveset registers scripts on a vanilla agent),
    `Clone { src_owner, src_name, src_kind, use_original_code, site }`.
  - Vanilla articles: from the dump's `fighter/<owner>/model|motion` folders and the
    `FIGHTER_<OWNER>_GENERATE_ARTICLE_*` constants (`lookup` data).
- `clone_weapon` reader in `acmd_src` (syn-based, like the existing registration reader):
  both signatures, distinguished by argument count/types; the id binding (`+=`, `= N + …`,
  `let x = …; X.get_or_init(|| x.generate_id)`); const indirection resolved through
  `project_consts`; loops over literal tables unrolled; unresolvable sites reported, not dropped.
- `generate_id` resolution: a spawn's first argument resolves to an `Article` through, in order,
  a lua_const, a clone binding static/OnceLock, a project const, or a local bound earlier in the
  function; otherwise the row is kept as "unresolved article id `<expr>`".
- **Settings reader**: scans the article agent's status scripts in the source for the calls in
  gap 4 and records each as `Setting { field, value: Literal|Const|Param{block,field}|Computed(expr), site }`.
  Vanilla articles fall back to `param_<name>` (existing `flight_from_params`).
- Writers: each `Setting` site and each new timeline row writes back through the existing
  byte-exact rewrite machinery in `acmd_src` (the same path hitboxes/effects use).

Timeline rows (`data.rs`): a typed `ExcuteStmt::Article(ArticleCall)` with
`ArticleOp::{Generate{ignore_has, model}, Remove, RemoveExist, Shoot, ShootExist, ChangeMotion,
ChangeStatus, SetRate, SetFrame, AddMotionPartial, SetVisibilityWhole, SetVisibility, SetPos, Other(raw)}`,
`target: ArticleTarget::{Last, First, All, Term, None}`, the article symbol as written, and the
resolved `Article`. Unknown `ArticleModule::` functions stay `Other` (kept verbatim). The
existing native-call path in `move_nodes/native_calls.rs` and the walk in `data.rs` both read
from this one typed call so they cannot drift.

UI: the move's timeline shows article rows in plain words ("Creates the fireball", "Shoots the
last fireball"). Each owner gets an **Objects** list (left panel, under the fighter's moves):
each article with origin badge, its scripts as moves, and a Settings card (G4) with origin chips
and jump-to-source.

### Phase B — live

The plugin (`slight_replica`): extend capture/injection to the **selected fighter's** articles
only (owner resolved through `article_notifier`'s owner-slot lookup, including nested owners),
keyed by (owner kind, article kind, script). Fighter capture stays isolated exactly as now.
Clone articles: resolved through smashline's kind mapping at runtime; if that cannot be read
safely, injection for clones is disabled and the UI says so.

### Phase C — preview

`article_preview` consumes `Article` + resolved `Setting`s: scale applied to the object's
placement, flight speed/accel/life/gravity from settings (params as fallback, as now), scripts
from the moveset source before the dump, spawn rows from G2 (including `ALL`/`LAST` targets,
status changes and rate). Spawns from status/opff sites appear on the frame the status
reaches, marked approximate.

### Verification

- **Corpus sweep** (`tests/article_corpus.rs`, ignored by default, run with an env var naming
  the `Movesets` folder): for each moveset, build the `SourceIndex` and `ArticleCatalog` and
  write a report (`target/article-corpus.md`): per article, owner, origin, resolved/unresolved
  ids, number of spawn rows, settings found and their origins, and a byte-exact round trip of
  every file that has article rows. Pass = zero misattributions, zero unexplained unresolved
  ids, round trip identical. The report's residue is the work list until it is empty.
- Unit tests per reader with fixtures copied from the corpus shapes listed in gap 3–6.
- `cargo test` stays green; plugin builds with and without `mod_plugin`.
- Manual live check: Shy Guy (trampoline/fireball nested owner), King Boo (new clone API),
  Latios (old clone API), Sceptile (looped clones), Mario fireball (vanilla).

## Calls I made (not discussed; correct me)

- Base branch `merge-upstream-0.3`; nothing pushed.
- Computed settings (charge lerps etc.) are read-only with their formula, not "simplified".
- Status/opff spawn sites are previewed and listed but not editable in phase A.
- Clone registrations are shown, not edited, in phase A.

## Clone params and the clone compatibility sheet

**Where a clone's params live (settled 2026-11-20).** A clone reads `param_<new_name>` from
the **new owner's** param file. The source article's original code, when it runs, reads that
block too. Evidence: every cloning moveset that ships a `vl.prc` carries a block named after the
clone, and none carry the source's name:
- Latios: `param_lativolt`, `param_latisnow`, `param_latihado` are in `fighter/samus/param/vl.prc`. `param_fireball` and `param_swordbeam` are absent.
- Sceptile: `param_stick`, `param_healingorb`, `param_leafstorm`, `param_leechseed`, `param_bulletseed` are in `fighter/mario/param/vl.prc`.
- Amy: `param_present` is in `fighter/kamui/param/vl.prc`. `param_ironball` is absent.
- Iron-Man and Latios's `latiball` ship no block for the clone. Their settings come from their own status code, or are missing.

joobert's sheet gives the in-game side, as the "Depends on vl.prc" rows. Mario's fireball clone:
horizontal speed "will always be controlled via vl parameters". Yoshi's egg: whether it
self-destructs depends on vl.prc.

So in Phase A, the settings reader resolves a clone's `Param` settings against the new
owner's file, merged as the game loads it (dump, then reference mods, then the moveset's
`romfs`, then `param_config` overrides). Two cases need care:
- When the clone uses its source's original code but the new owner's file has no
  `param_<new_name>` block, the Settings card says the article reads params that aren't there,
  and that the source's defaults are not inherited. The values are shown as missing, never
  borrowed from the source.
- When the block exists, its fields are editable and saved to the moveset's own param file,
  or its prcxml when that is what the moveset ships.

**Clone compatibility sheet** (<https://docs.google.com/spreadsheets/d/16SEU3MibrzTJHTjxJb7c5e7JzGgrfWY_c_hqNJtGvNY>):
67 community-verified weapon kinds. Each records whether it can be cloned, whether it
self-destructs on hitting an opponent and on hitting a shield, who verified it, and notes. It
ships as a data file with a link to the source and the verifiers' credit. Visionary uses it in
three places:
- The Objects list warns when a moveset clones a kind marked "Can Clone: No": `BRAVE_CRASH`,
  `EFLAME_ESWORD`, `LUCARIO_AURABALL`, `MURABITO_BULLET`, `PACMAN_FIREHYDRANT`, `PICKEL_MELT`,
  `SZEROSUIT_LASER`.
- The clone's card shows the sheet's self-destruct behaviour and notes for its source kind.
  Examples: `SAMUS_BOMB` "ArticleModule::get_active_num doesn't work"; `LINK_BOWARROW` crashes
  when picked up while using its original code; `PALUTENA_AUTOAIMBULLET` always spawns with its
  effect.
- The sheet's top-of-sheet rule of thumb is shown as advice for kinds the sheet doesn't list.
  Articles that talk to their owner in a special way, use stage geometry, or come from a DLC or
  echo fighter will probably not clone well.

The preview does not model self-destruction from the sheet. It only shows it as text.

## Left to the builder

- Check the param rule above in game with the plugin for one moveset that ships the block
  (Latios `lativolt`) and one that doesn't (Iron-Man), before Phase C relies on it.
- The precise smashline revision → signature table (two shapes known; check the moveset's
  `Cargo.lock` pin first, argument types second).
- How far the syn-based settings reader follows helpers (one call deep is the suggested start).
