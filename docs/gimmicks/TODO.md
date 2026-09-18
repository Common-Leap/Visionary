# Fighter Gimmicks — Task Board (deduplicated)

Source: gimmick research (Cloud Limit, Banjo feathers, Olimar Pikmin, etc.).
Each task below is **unsupported today**: outside ACMD `game_/effect_/sound_/expression_`,
outside `fighter/common/param/fighter_param.prc`, outside `ui_chara_db.prc`/`.eff` visuals.

Design input: personal tunables live in `fighter/<name>/param/vl.prc`
(sub-structs like `param_special_lw[0].limit_gauge_add`, labels via
`ultimate-research/param-labels`); behavior lives in compiled status scripts
(`WorkModule` `FIGHTER_*_INSTANCE_WORK_ID_*`, `ArticleModule`, `WEAPON_*`
statuses, smashline/HDR `fighters/<name>/src/status/*.rs`).

## Duplicates removed (do not re-add)

- `Peach` + `Daisy (Peach echo)` merged → **G-48 Peach/Daisy family** (was listed twice).
- `Link` + `Toon Link` + `Young Link` merged → **G-35 Link family** (bomb/shield shared, per-variant notes kept).
- `Samus` + `Dark Samus (variant-only)` merged → **G-55 Samus family**.
- `Ryu` + `Ken` merged → **G-31 Ryu/Ken family** (input parser shared, per-fighter tables kept).
- `Simon` + `Richter` merged → **G-73 Simon/Richter** (none, was listed twice).
- `Pit` + `Dark Pit` merged → **G-72 Pit family** (none).
- `Marth` + `Lucina` + `Roy` + `Chrom` merged → **G-71 FE swords** (none, tipper is ACMD).
- `Fox` + `Falco` + `Wolf` merged → **G-70 Star Fox** (none, was triplicated).
- `Pikachu` + `Pichu` merged → **G-49 Pikachu family** (shared Skull Bash, Pichu recoil noted).
- `Ness` / `Lucas` kept separate (G-45, G-38): distinct Freeze vs Flash + Thunder pierce. Shared Magnet text cross-referenced, not duplicated.
- `Villager` / `Isabelle` kept separate (G-64, G-27): share Pocket concept, distinct Tree vs Trap/Rod. Cross-referenced.
- If a fighter appears in two phases below, that is a bug — fix by merging, not copying.

## How to use this board

Same markers as `docs/roster/TODO.md`. Change in place, same commit as the work:

- `[ ]` **Not started** — free if `Blocks on` are all `[x]`.
- `[~]` **In progress** — add `Owner:` name/agent + date. Abandon → back to `[ ]` + reason.
- `[x]` **Done** — add `Done:` date + commit or verification note.
- `[!]` **Blocked** — add `Blocked:` naming exactly what is missing.

Rules:

- Do not start a fighter task whose `Blocks on` framework task is unmet.
- `Done` requires real-dump verification (personal `vl.prc` path + labels resolve, or explicit `[!]`).
- If a task is wrong/unneeded, do not delete — mark `[x]` + `Done:` why dropped.
- Keep scope. New findings → new `G-##` at end, do not grow the task you are in.

---

## Phase 0 — Framework (blocks all fighter work)

- [ ] **G-00 — Sparse personal-prc editor (`fighter/<name>/param/vl.prc`)**
  Tunables card per fighter: path + `param_*` sub-struct selector + index, type-coerced like `src/roster/traits.rs`, labels from ParamLabels.csv. Sparse like `ParamMod`, never whole-file copy.
  Blocks on: —
  Owner: —
- [ ] **G-01 — Work/Article slot registry**
  Per-fighter allowlist of `FIGHTER_*_INSTANCE_WORK_ID_FLOAT/INT/FLAG_*` (from `skyline-smash` `lua_const.rs`) + `ArticleModule` generate/remove ops + `WEAPON_*` statuses. No free-form slot writes.
  Blocks on: G-00
  Owner: —
- [ ] **G-02 — Status-hook codegen (Cloud-like logic)**
  Export `status_script` stubs for hooks: `onDealDamage/onTakeDamage/onChargeTick/onStore/onSpend/onTimeout/onWhiff/onStockStart/onKO/onArticleSpawn/Catch/Timeout/Transfer/onRNG/onCounterHit/onPin/Carry/Mash/onSwitch/onAbsorb`. Cloud Limit is the template (G-15). Behavior lives in code, not numbers.
  Blocks on: G-01
  Owner: —
- [ ] **G-03 — HUD meter framework**
  Meter above % vs icon vs pips (Limit, Arsène, MP, KO arrows, Ink tank, Waft flash, Robin pips, Sora cycle icon, Steve mats). Toggle + bind to Work float/int.
  Blocks on: G-01
  Owner: —
- [ ] **G-04 — Verification harness**
  Real-dump check per fighter: `vl.prc` path exists, labels resolve, article folder present. Wrong path/label is silent — test it.
  Blocks on: G-00
  Owner: —

---

## Phase 1 — Fighters A–M (deduplicated)

- [ ] **G-10 — Banjo & Kazooie (`buddy`): Wonderwing feathers / eggs / Blaster decay**
  5 feathers/stock, spend on dash start only (startup interrupt = no spend), replenish KO only. Wonderwing 22%/16%, intang 18–end. Blaster decay + cooldown. Rear Egg 1-active throwable/pocketable.
  Files: `fighter/buddy/param/vl.prc`, `fighter/buddy/article/egg|spring_pad`, `special_s/n/lw` status.
  Editable: `max_feathers, refill_on_stock, spend_on_dash, invinc, clean/late dmg/KB, decay_per_shot/recover, max_eggs=1`. Hooks: `onStockStart(reset 5)/onSpend/onInterrupt`.
  Blocks on: G-00, G-01
  Owner: —
- [ ] **G-11 — Bayonetta: Witch Time + Bat Within + Bullet Arts**
  Counter 8–27f intang 8–23f, slow 1/8, `frames=(90-n)*b+0.3p` max 240 min 22, n +60/use +40 success, regen 0.04/f, projectile −30f. Bat 24–35f or early dodge: 0.5× dmg 0 KB + forced-down air. Bullet Arts disjoint hold-gun.
  Files: `fighter/bayonetta/param/vl.prc`, `special_lw` + dodge statuses.
  Editable: `base, per_use/success penalty, regen, max/min, percent_scale, bat_mul, windows, slow_mag`.
  Blocks on: G-00, G-01
  Owner: —
- [ ] **G-12 — Bowser (`koopa`): Tough Guy + Fire Breath**
  Armor 19 KB always-on, crouch 0.85× stacks, decays with %/rage, no grab protection. Breath 1.8% loop /7f, weaken 300f use, refresh 700f idle, size 1.7→0.2×.
  Files: `fighter/koopa/param/vl.prc`, damage callback (not ACMD).
  Editable: `armor_threshold, crouch_mul, tick_dmg, weaken/refresh, min/max range/size`.
  Blocks on: G-00
  Owner: —
- [ ] **G-13 — Bowser Jr. (`koopajr`): Car split + Cannon/Mecha**
  Car 0.88×, body 1.15×, car priority; Abandon Ship ledge = always 1.15×. Cannon 10%/7%→20%/14% full max 2, falling double-hits shields 8. Mecha 1-active, wall-turn, 2% bite +7% boom ~5s, shield → pickup item.
  Files: `fighter/koopa_jr/param/vl.prc`, `article/cannonball|mechakoopa|clown_car`.
  Editable: `muls, priority, charge_rate/max_active/shield_dmg, walk_time/HP/pickup`.
  Blocks on: G-00, G-01
  Owner: —
- [ ] **G-14 — Byleth (`master`): Failnaught lock + Areadbhar tipper/lunge + Aymr armor**
  Failnaught early cancelable, late locked; 12% tap 29% full beam. Tip 1.5× shaft. Smash-input lunge extends. Aymr 30% + shockwave, armor 34–63f + Zoom.
  Files: `fighter/master/param/vl.prc`, `special_n` phase machine.
  Editable: `cancel_window, full_time, tipper_mul, lunge_dist, armor frames/threshold`.
  Blocks on: G-00, G-01
  Owner: —
- [ ] **G-15 — Cloud: Limit gauge (TEMPLATE for all meters)**
  100 units, 15s/900f no countdown, lost KO. Deal 0.667×, take 1.0× (shields/items excluded). Charge +0.3/f 334f solo. Hit out of charge subtracts gain. Full: mobility buff (not attack speed) + one powered special, spent even miss/interrupt.
  Files: `fighter/cloud/param/vl.prc` (`limit_max, gain_deal/take, charge_per_frame, break_duration`), `limit_gauge` float + `limit_break` flag + % meter UI.
  Editable: `max, deal_mul, take_mul, charge_per_frame, duration, mobility_muls, powered_ids`. Hooks: `onDeal/onTake/sub-if-charging/onChargeTick/onSpend/onTimeout/onKO`.
  Blocks on: G-00, G-01, G-02, G-03
  Owner: —
- [ ] **G-16 — Corrin (`kamui`): Fang Shot dual charge + Lunge pin**
  Shot water 4→11% paralyze/range, bite 10→20% separately chargeable. Lunge 7%/8%/15% tip sticks ~2–3s, mash escape, F/back kick 12%/7%/5%, jump, down-cancel.
  Files: `fighter/kamui/param/vl.prc`, `article/spear_pin`, `special_n/s`.
  Editable: `shot/bite frames, pin_max, mash_mul, kick dmgs`.
  Blocks on: G-00, G-01
  Owner: —
- [ ] **G-17 — Donkey Kong (`donkey`): Giant Punch + Cargo**
  Punch 110f full, `10%+18*charge/110` → 28%/25% air, full armor 9–20f, store-cancel, lose on knockback. Cargo `((90+1.7p)*11/6)-g`, mash −8f/−14.4f, 1.1× carry, 4 throws.
  Files: `fighter/donkey/param/vl.prc`, `special_n` int + `cargo` timer.
  Editable: `full_frames, bonus, armor, store_lag, cargo_consts, mash_reduction, carry_mul`.
  Blocks on: G-00, G-01
  Owner: —
- [ ] **G-18 — Diddy Kong (`diddy`): Banana + Peanut overcharge + Rocketbarrel detach**
  Banana pull 4f/throw 20f, 61f cooldown, 1-active, slips, transcendent, gone after 2 throws/foe hit. Peanut 94f full 4.8→13.8%, overcharge 23% self-vuln. Rocketbarrel air-charge, hit detaches 1%→18% boom.
  Files: `fighter/diddy/param/vl.prc`, `article/banana|peanut|rocketbarrel`.
  Editable: `max 1, cooldown, slip, damage_curve, overcharge_delay/explosion, charge_gain, detach_flag`.
  Blocks on: G-00, G-01
  Owner: —
- [ ] **G-19 — Dr. Mario (`mariod`): verify minor/none**
  Pills gravity/bounce, Sheet mul, Tornado mash-rise (no FLUDD). Confirm no persistent resource, then close as none + tunables only.
  Files: `fighter/mariod/param/vl.prc`.
  Blocks on: G-04
  Owner: —
- [ ] **G-20 — Duck Hunt (`duckhunt`): Can + Clay + Gunmen**
  Can F1 spawn, 8 shots → auto-boom; 1.8% shots + 1.8–4.4% contact + 10% boom. Movable any attack, hurts owner, usable while grabbing/grabbed. Clay slow-arc vs fast-straight, B 1 shot/press, B prioritizes clay. Gunmen 5-cycle shuffle no-repeat, delay/dmg 8–11%/angle, 5 HP, body blocks.
  Files: `fighter/duckhunt/param/vl.prc`, `article/can|clay|gunman0-4`.
  Editable: `max_shots, dmg_curve, clay_hp/speed, per-gunman delay/dmg/angle, cycle_mode, block_hp`.
  Blocks on: G-00, G-01
  Owner: —
- [ ] **G-21 — Greninja (`greninja`): Shuriken inverse charge + Sneak + Substitute**
  Tap small/fast/weak 3–10.8%, hold bigger/slower/stronger, full multi 1%+9% auto-release unstoreable, air Fall Break once. Sneak hold extends shadow, limited actions, 10% fwd/12% reverse. Substitute counter → 8-way kick 13/12/11% (down meteor), log blocks single-hit projectiles.
  Files: `fighter/greninja/param/vl.prc`, `article/shadow|substitute_log`.
  Editable: `charge_time, size/speed/dmg curves, shadow_speed/max, kick dmg/angles, counter_window, log_hp/life`.
  Blocks on: G-00, G-01
  Owner: —
- [ ] **G-22 — Hero (`brave`): MP + Menu RNG + crits**
  100 MP start/full respawn. Costs Frizz 6 → Kazap 42 etc. (see catalog). Regen 1/s idle +0.8× base dmg on hit/shield, paused menu/charging. Menu 4 distinct of 21, no repeat slot twice row, 480f auto-close, reroll on shield/jump cancel. Smashes 1/8 crit ~1.5×. Burst `0.2+0.008*MP`.
  Files: `fighter/brave/param/vl.prc`, `article/command_window`, `mp/psyche/oomph/accel` Work ints.
  Editable: `max, regen, hit_gain, costs, pool_weights, no_repeat, timeout, buff durations/muls, burst_formula, KO curves, crit_rate/mul`.
  Blocks on: G-00, G-01, G-02, G-03
  Owner: —
- [ ] **G-23 — Ice Climbers (`popo/nana`): Nana partner**
  Mirror 6f delay, higher mobility, deals less, takes 1.02×, separate hidden %. Separated = AI beeline, buffers ignored till reunite; reunion mid-move = desync. Leader grabbed = cheer (no chain-grabs). Sopo nerfs.
  Files: `fighter/popo/param/vl.prc`, `article/nana_ai`.
  Editable: `delay, dealt/taken_muls, offsets, reunion_dist, solo_penalties`.
  Blocks on: G-00, G-01
  Owner: —
- [ ] **G-24 — Ike: Eruption pillars + Quick Draw hold**
  Eruption base 10%+4%/0.5s, L2 120f L3 180f, full 35/28/26% + armor + 10% self F11 + Zoom, auto-fires ~2s full, breaks shields. Quick Draw hold ~2s auto-fire, tap ½ FD → full ¾–FD, 9→16%, ledge-stop grounded, air helpless.
  Files: `fighter/ike/param/vl.prc`, `special_n/s`.
  Editable: `thresholds, curve, self_dmg, armor, distance/dmg curves, max_hold, ledge_stop`.
  Blocks on: G-00, G-01
  Owner: —
- [ ] **G-25 — Incineroar (`gaogaen`): Revenge stacks**
  Counter F3–27, tanks 0.4× + set-KB burst. Next attack up to 3× dmg (KB scales to 3×, per-move KB muls), shield safety up (lag/stun NOT reduced). Stacks re-counter, resets duration. 60s max, decays whiffs (per-move cost), lost on 36% taken or any throw. Pummel/floor/edge/items exempt.
  Files: `fighter/gaogaen/param/vl.prc`, `special_lw` counter + buff flag.
  Editable: `window, taken_mul 0.4, max_mul 3.0, duration 3600f, loss_threshold 36, miss_costs, per-move KB muls, shield_mul`.
  Blocks on: G-00, G-01, G-02
  Owner: —
- [ ] **G-26 — Inkling: Ink Tank + inked (Kirby copy note)**
  Tank 150, Bomb line 30. Costs inf 2/hit, Shot 1.5/hit, Bomb 30. Below line Shot falls; empty: inf/finisher no hitbox, Roller harmless, Bomb look-back, smashes/F-throw gutted. Refill shield+B grounded or auto when empty, full KO, HUD + red X. Inked ~60% =1.5× cap, extends 5s→20s decay. Kirby copy tank no refill → auto-lose empty.
  Files: `fighter/inkling/param/vl.prc` (+ kirby ink copy), `article/ink_splat`, `ink_remaining/inked_amount/timer`.
  Editable: `max, costs, threshold, refill_rate, fallback muls/disable, cap/coating/decay`.
  Blocks on: G-00, G-01, G-03
  Owner: —
- [ ] **G-27 — Isabelle (`shizue`): Lloid Trap + Rod + Pocket**
  Pair with G-64 Villager (shared Pocket concept, distinct Tree vs Trap/Rod).
  Trap plant 51f, 10s life, 8 HP (destroyed boom hurts owner), proximity/manual detonate. Rod tilt 38 vs smash 58 (2.0 vs 3.6 speed), 5s linger, hook hit-grab shieldable, throw 9.5–18.7% by bobber speed. Pocket 1.9× (0.475× teammate), indefinite + HUD, lost KO.
  Files: `fighter/shizue/param/vl.prc`, `article/lloid_trap|fishing_rod`.
  Editable: `plant_lag, life, hp, radius, rod_lengths/speeds/linger, throw_range, pocket_mul`.
  Blocks on: G-00, G-01
  Owner: —
- [ ] **G-28 — Jigglypuff (`purin`): Rollout + Rest + shield-break death**
  Rollout 35f full +100f hold auto-fire, 20–90f roll, speed 0.8–4.0 (6.5 downhill), hitbox ≥2.0, `dmg=speed*5.1` 10–20% (33% downhill), turn 29f 0.25×, wall 0.75×, hit = rebound helpless +20f lag. Rest 2–4f hit 1–27f intang 20% + flower, 187f hit/210f miss. Shield break = rocket KO (unique).
  Files: `fighter/purin/param/vl.prc`, `special_n/lw`, shield-break override.
  Editable: `charge/hold, thresholds, formula, rebound, rest dmg/KB/interrupt, pound_shield`.
  Blocks on: G-00, G-01
  Owner: —
- [ ] **G-29 — Joker (`jack`): Rebellion / Arsène**
  Passive 0.00925/f (~180s solo, paused revival). Take 1.3× (77% solo full); −1 stock 1.625×, −2 1.95× (× players); teammate 0.2×, teammate KO +4. Guard armor 3–19f holdable 120f 0.4× taken: gain `m*0.4*6.7368*dmg` → 28.6% full 1v1. KO reset 20 (25/30 if −2/−3, × players). Full = Arsène 1800f/30s +31f intang. Drain over time +16× dmg frames 1v1 (player-count table). Tetra 1.6× min 12% (4f), Maka 1.6× +1.9× reflector.
  Files: `fighter/jack/param/vl.prc`, `article/arsene`, gauge UI.
  Editable: `max, passive, take/behind/player tables, guard_taken/gain, arsene_time, drain_per_dmg, reset, tetra/maka muls`.
  Blocks on: G-00, G-01, G-02, G-03
  Owner: —
- [ ] **G-30 — Kazuya: Rage + Tough Body + Crouch Dash/EWGF**
  Rage 100% (25% HP stamina): 1.1× dmg, aura + rumble, no timer. Ends Drive land or `(misses*110+taken*17.6/11.22≥650)` ≈36.9% no whiffs; once/stock. Drive armor 5–12f + Zoom. Tough Body 14 KB (+crouch 0.85× + move armor). Crouch →↓↘ slide + upper intang, chained wavedash; tap-A WGF, just-frame 3f EWGF (pre-hit intang, paralysis, shield push), hold-A Dragon (disabled Rage → Drive).
  Files: `fighter/kazuya/param/vl.prc`, command parser + rage flag/timers.
  Editable: `threshold, dmg_mul, end_consts, once_per_stock, armor_threshold, input_windows (12f/3f EWGF), drive dmgs`.
  Blocks on: G-00, G-01, G-02
  Owner: —

## Phase 2 — Fighters N–Z + specials (deduplicated)

- [ ] **G-31 — Ryu/Ken family: strength states + command inputs + Focus (merged)**
  Tap=light hold=heavy, cancel on hit. Hadoken speeds 0.9/1.2/1.5, input 1.25× (Ken SSB4 spread, no Shakunetsu; Ryu Shakunetsu multi-fire). Shoryu tap 1-hit/mid 2/flame 3, input 1.2× + arm intang 1–14f + ⅓ lag 8–12f. Ken Roundhouse half-circle Oosoto 12% / Inazuma axe. Focus 3 levels: 1-hit armor 7–14% by charge, Lv1 knockdown Lv2–3 crumple, dash-cancel + shuffle. Auto-face 1v1. Windows 12f (22f Terry-like N/A).
  Files: `fighter/ryu|ken/param/vl.prc`, command buffer + `FOCUS_LEVEL`.
  Editable: `light/heavy dmg, input_mul/window, intang/lag tables, focus armor/charge/crumple, cancel_list, autoface`.
  Blocks on: G-00, G-01, G-02
  Owner: —
- [ ] **G-32 — King Dedede (`dedede`): Gordo + Jet Hammer + Inhale**
  Gordo hammer 10% + 14/12.5/11/9.5% timing, U/F/D, ~3s, ≥2% reverses owner; Inhale own → relaunch, attack-catch ×3. Inhale 14f vacuum, spit 12% (14.4% 1v1), 0.25× taken holding, catches projectiles 1.5× uncapped. Jet Hammer walk 0.7 u/f + jump charging, 120f full +0.15%/f 12–40%/11–32% air (pre-full best KB, full breaks shields), hold-full self 1%/30f cap 100%, swing armor 14% 1–14f grounded.
  Files: `fighter/dedede/param/vl.prc`, `article/gordo`, `special_s/lw`, `inhale_hold`.
  Editable: `reflect_threshold, life, charge_rate/max, self_rate/cap, armor, resist/spit/reflect mul`.
  Blocks on: G-00, G-01
  Owner: —
- [ ] **G-33 — King K. Rool (`krool`): Belly HP + Crown + Blunderbuss**
  Belly 18.01 HP (50/50 split → ~36.02 practical), regen 0.08/16f, cracks 11.5/5.02, clang+16f hitlag, halves dmg; break = shieldbreak stun + Zoom. Per-move armor windows (F-tilt 5–11f, dash 7–28f, etc.). Crown armor 12% 6–63f cumulative, 27f toss, boomerang return, 1 crown; miss-catch = item (catch 18f vs pickup 28f), 12s respawn or touch; foe item 9%. Blunderbuss 25f stance, 1 kannonball ~2.5s re-arm 120f + ricochet ~3–4s, 13%/12%/17% reshot; vacuum 6–36f +90f hold, fighters > ball, 3 angles.
  Files: `fighter/krool/param/vl.prc`, `article/crown|kannonball`.
  Editable: `hp/split/regen/cracks, per-move windows, crown armor/respawn, kannon dmg/speed/life, vacuum duration/priority/throw dmgs`.
  Blocks on: G-00, G-01
  Owner: —
- [ ] **G-34 — Kirby (`kirby`): Copy Ability**
  Inhale 10f 8.5u vortex holdable. Swallow 10% or spit star `25+0.5t` / `2.8+0.008(t-k)`. Copy = foe neutral-B 1.2×, 20s grace then loss chance = % taken/hit, lose KO/taunt. Per-fighter adapts (Olimar max 3, Dedede spit-only, Inkling no refill → auto-lose, Steve 4/2/1 cap 20 no craft, Robin break=lose). Projectile ≥5.0 hold 180f → spit 0.8× min 10% or heal 1%.
  Files: `fighter/kirby/param/vl.prc`, `article/hatXX` + star, `COPY_ABILITY_ID`.
  Editable: `copy_table, damage_mul, grace/loss_curve, star_consts, threshold/hold/spit/heal`.
  Blocks on: G-00, G-01, G-02
  Owner: —
- [ ] **G-35 — Link family (`link|toonlink|younglink`): Bomb + Shield + Fire Bow (merged)**
  1 bomb max, no spawn holding item, no detonate if foe holds or Link helpless. Throw 1% + detonate 7%, 12f delay, 50 HP, 1800f auto + blink, bounce 0.9/0.7/0.2×. Shield blocks standing/walk/crouch except head/feet. Young: fire arrows + enemy-hit blast does NOT hurt self; Toon: can trade; Hero Bow auto-fires >3s.
  Files: `fighter/link|toonlink|younglink/param/vl.prc`, `article/remote_bomb|bowarrow|boomerang`.
  Editable: `max_active, fuse/blink/delay, HP, bounce, detonate dmg/KB/radius, shield_poses, fire_ticks, self_hit_flag`.
  Blocks on: G-00, G-01
  Owner: —
- [ ] **G-36 — Little Mac (`littlemac`): Power Meter / KO Punch**
  Deal 0.3× + take 1.0× (shields count) → 333% dealt or 100% taken solo. 10 arrows + bell + flash. Full swaps Lunge → KO Uppercut: ground unblockable (except Witch Time), armor 8–9f, reaches BF low plats, KO 10%→40%; air weaker/blockable. Spend on press even hitstun, lost KO or tumble after 240f grace (footstool exempt), hidden Giga Mac.
  Files: `fighter/littlemac/param/vl.prc`, `power` float + `ko_ready` flag + UI.
  Editable: `max, muls, count_shields, grace, uppercut dmg/armor/unblockable, air_mul`.
  Blocks on: G-00, G-01, G-02, G-03
  Owner: —
- [ ] **G-37 — Lucario: Aura**
  `0.66×@0% →1.0×@65% →1.67×@190%`: below `1+(p-65)*0.34/65`, above `1+(p-65)*0.67/125`. Stock Aura (humans, 6.25s blend, clamp 0.6–1.8×): 1v1 +2 0.83×/+1 0.915×/0 1.0×/−1 1.2×/−2 1.4× (3p/4p tables in files). Scales dmg/KB + Sphere scale (2.576× 0→max), Palm range, ES distance. Stacks Rage.
  Files: `fighter/lucario/param/vl.prc`, recompute on %/stock.
  Editable: `floor/base/ceiling, slopes, stock_formula, clamp, exempt_flags, sphere_link, palm_range, es_dist`.
  Blocks on: G-00, G-02
  Owner: —
- [ ] **G-38 — Lucas: Freeze + Thunder pierce + Magnet 2.0×**
  Pair with G-45 Ness (shared Magnet/Thunder family, distinct Freeze vs Flash).
  Freeze steerable 10–23% freeze-scaled, passes soft platforms. Fire straight burst. Thunder head pierces multi-tail, tighter turn, Thunder2 multi 32.5% + wall-bounce window. Magnet 2.0× cap 30% + faster + frontal windbox, release 8% semi-spike, ground absorb-cancelable. Rope Snake tether.
  Files: `fighter/lucas/param/vl.prc`, `article/pk_freeze|pk_thunder_head`.
  Editable: `freeze curve/steer, thunder turn/tail, thunder2 hits/distance, heal_mul/cap/windbox/release`.
  Blocks on: G-00, G-01
  Owner: —
- [ ] **G-39 — Luigi: Missile misfire + Cyclone mash**
  Missile 90f full 6.16→21%, misfire 10% any charge = 25% flat dist 3.0, intang 18–22f, pierces walls. Cyclone invinc 4–8f ground/1–7f air, 2%×4+4%, windbox pull, mash-height, no helpless.
  Files: `fighter/luigi/param/vl.prc`, `special_s` RNG + charge float.
  Editable: `charge_time, curves, misfire_rate/dmg/dist/intang, mash_gain, invinc, windbox`.
  Blocks on: G-00, G-01
  Owner: —
- [ ] **G-40 — Mario: F.L.U.D.D. tank + Cape**
  Auto-charge, store shield/roll/jump/spot (air = airdodge), lose on hit. 7 globs aimable U/D, 0% +70 set-KB push, clangs/blocks, recoil self-push. Cape reverse + 1.5× reflector.
  Files: `fighter/mario/param/vl.prc`, `special_lw` float + `cape_reverse`.
  Editable: `charge_time, shots, push KB/range/angle, recoil, store_cancels, lose_on_hit, cape mul`.
  Blocks on: G-00, G-01
  Owner: —
- [ ] **G-41 — Mega Man (`rockman`): Blade + Bomber + Leaf**
  1 Blade alive, 8-way throw, sticks, anyone picks up. 1 Bomber alive, latches fighter/surface, transfers contact, timer + primed contact explode. Leaf 4 leaves ~3s, each eats 1 hit/projectile, then throw.
  Files: `fighter/rockman/param/vl.prc`, `article/metalblade|crashbomber|leafshield`.
  Editable: `max 1, stick_time, fuse/transfer, leaf_count/duration/thrown_dmg`.
  Blocks on: G-00, G-01
  Owner: —
- [ ] **G-42 — Mewtwo: Ball store + Disable + Confusion**
  Ball ~123f full, store shield/roll/jump indefinite, air recoil full. Disable ground-only + eye-contact + %-scaled stun, whiff lag. Confusion command-grab spin + energy reflector.
  Files: `fighter/mewtwo/param/vl.prc`, `SHADOWBALL_HAD`.
  Editable: `charge_max, store_flag, recoil, disable_range/stun/whiff, reflect_window`.
  Blocks on: G-00, G-01
  Owner: —
- [ ] **G-43 — Mii Brawler/Sword/Gunner: custom-select + class stats**
  3×4 options per class (Brawler Shot Put/Mach Punch/Side Kick; Sword Gale/Chakram/Spin; Gunner Charge Blast/Blaze/Rocket + Grenade). Fixed Ultimate stats: Brawler lightest/fastest, Gunner heaviest/slowest, Sword mid (weight 100). No height/weight sliders.
  Files: `fighter/mii_fighter|mii_brawler|swordfighter|gunner/param/vl.prc`, `ui/mii` DB.
  Editable: `special_id_table[4][3], class weight/walk/run/air/fall, per-option article params`.
  Blocks on: G-00, G-01
  Owner: —
- [ ] **G-44 — Min Min (`tantan`): ARM cycle + Dragon buff**
  Right cycles Ramram (fast/curvy/fire smash) → Megawatt (slow/short/electric KO ~40%) → Dragon (mid + held-smash laser homing). Left locked Dragon. Throw foe = Power Dragon ~20s flames + left dmg/KB incl. laser up. Lost grab/freeze/stun/sleep/crumple/paralyze/bury/missed tech.
  Files: `fighter/tantan/param/vl.prc`, `ARM_CHANGE + POWER_DRAGON_TIMER`.
  Editable: `order, per-arm dmg/range/speed/charge_effect, laser dmg/homing, duration, mult, clear_bitmask`.
  Blocks on: G-00, G-01, G-02
  Owner: —
- [ ] **G-45 — Ness: Magnet + Flash + Thunder2 + Yo-yo**
  Pair with G-38 Lucas (shared Magnet/Thunder family, distinct Flash vs Freeze).
  Magnet holdable F7, 1.6× cap 30% + release 8% + windbox. Flash hold-charge 1.05–1.8×. Thunder steerable head, self-hit = PKT2 stick angle. Yo-yo active while charging, hangs ledge.
  Files: `fighter/ness/param/vl.prc`, `article/pkflash|pkthunder`.
  Editable: `heal_mul/cap/absorb_start, flash min/max dmg/size, thunder_speed/control, pkt2 dmg/angle`.
  Blocks on: G-00, G-01
  Owner: —
- [ ] **G-46 — Olimar (`pikmin`): queue / maturity / elements / HP**
  Pluck max 3, loop Red→Yellow→Blue→White→Purple. Front used for smashes/aerials/grab then back. Red dmg+fire immune, Yellow range+electric, Blue throw+water, White fast/poison latch+long grab, Purple heavy slam no latch short throw. Each HP. Side-B latch DoT except Purple single. Whistle recall+reorder. Winged Up-B distance ∝ 1/count, no HP.
  Files: `fighter/pikmin/param/vl.prc`, `article/pikmin` per-color, Pikmin slot WorkModule.
  Editable: `max, order[5], per-color dmg/KB/element/latch_dps/HP/throw, winged_vs_count`.
  Blocks on: G-00, G-01, G-03
  Owner: —
- [ ] **G-47 — Pac-Man (`pacman`): Fruit cycle + Hydrant + Trampoline**
  Cycle Cherry→…→Key holds Key. Charge lengthens after Orange (Apple ~52f … Key ~132f). Store shield/jump/dodge, tap throw, hold resume. 1 fruit alive, catchable/pocketable, catch = resume (Recycle). Galaxian double, Bell stun/freeze, Key 16% KO. Hydrant falls 9%, waters push, launched ≥13% → 13% KO. Trampoline 3 bounces red=high, damageable.
  Files: `fighter/pacman/param/vl.prc`, `article/fruit|hydrant|trampoline`.
  Editable: `order[8], per-fruit charge/dmg/effect, hydrant_hp/launch_threshold, max_uses`.
  Blocks on: G-00, G-01
  Owner: —
- [ ] **G-48 — Peach/Daisy family: Turnip RNG + Float + Toad (merged)**
  Grin 5%/60.35%, Stare 5%/10.35%, Closed 5%/8.62%, Surprised/Happy 5%/5.17% each, Wink 10%/6.9%, Dot 16%/1.72%, Stitch 24%/1.72%, Saturn 7–9%/0.6%, Bob-omb 25–36%/0.4%. Throw speed scales. Float hold-jump hover + act. Toad counter scales.
  Files: `fighter/peach|daisy/param/vl.prc`, `article/turnip`, `float` flag + `special_lw` RNG.
  Editable: `weights[8]+saturn+bomb, base_damages, float_duration/speed/cancel, throw_scaling`.
  Blocks on: G-00, G-01
  Owner: —
- [ ] **G-49 — Pikachu family (`pikachu|pichu`): Skull store + recoil + Thunder (merged)**
  Skull Pika 6.2→21.4% ~80f, Pichu 4→33% ~150f, smash-input pre-charge, recovery-usable. Pichu 1–3% self on electrics. Thunder cloud+bolt+discharge.
  Files: `fighter/pikachu|pichu/param/vl.prc`, `article/thunder`.
  Editable: `charge_max, damage_curve 7+0.2*frames, distance_curve, recoil_per_move`.
  Blocks on: G-00, G-01
  Owner: —
- [ ] **G-50 — Pokémon Trainer: Switch cooldown + tri-fighter (1 task)**
  Down-B cycles Squirtle (small/combo) → Ivysaur (Razor/Seed zoner) → Charizard (heavy, Flare Blitz 5%+5% recoil, multi-jump). No stamina/type-effectiveness. Aerial switch allowed, intang during swap, ~120f cooldown, stale-queue NOT reset, HUD order.
  Files: `fighter/ptrainer/param/vl.prc` + per-mon `fighter/pzenigame|pfushigisou|plizardon/param/vl.prc`, `POKEMON_CHANGE_TIMER`.
  Editable: `order, cooldown, intang, per-mon weight/speed + article tunables`.
  Blocks on: G-00, G-01, G-02
  Owner: —
- [ ] **G-51 — Piranha Plant (`packun`): Ptooie hold + Poison charge + Long-Stem armor**
  Ptooie spike ball hold-blow (rises/falls), stick L/R fires (lowest = farthest), 14% held/18% early/13% late, transcendent, ≥10% to ball stops, drop on Plant hit = pseudo-counter, stall descent air, loses hitbox after bounce + per-hit decay. Poison chargeable store? 0.6–2.7%/loop center, no flinch/hitstun, heavy shield dmg, reflectable anytime. Long-Stem aimable bite 8.4/12% → 18.2/26% full, extends hurtbox, Pot Armor 15% charging/12% release 18–86f 0.8× taken, proximity F19 + head intang 1–2f.
  Files: `fighter/packun/param/vl.prc`, `article/spikeball|poison_cloud`.
  Editable: `hold_max, throw_dmg/range vs height, stop_hp 10, poison charge/range/loop/shield, stem charge/range/armor`.
  Blocks on: G-00, G-01
  Owner: —
- [ ] **G-52 — Pyra/Mythra (`eflame|elight`): Swap + Foresight + sword-loss**
  Down-B Swap intang F6, no cooldown. Pyra slow/power (Nova chargeable spin, Blazing End throwable sword — no attack/grab/swap till returns ~20f lockout). Mythra fast/combo (Buster levels, Photon Edge 5 tele-slashes, Ray/Dust projectile). Foresight spotdodge/roll vs hit = slow-mo + reduction. Start Pyra, A on select = Mythra.
  Files: `fighter/eflame|elight/param/vl.prc`, `article/aegissword`.
  Editable: `swap_intang/cooldown 0, foresight_window/slow/taken, nova/buster stages, return_lockout`.
  Blocks on: G-00, G-01, G-02
  Owner: —
- [ ] **G-53 — R.O.B. (`robot`): Beam + Gyro + Fuel**
  Beam charges by NOT using → burst → standard (~1s) → Super ~20s idle (LED flash), angleable + ricochet, Super 22% point-blank wider. Gyro hold-charge spinner, 1 alive, 6.1–10.8% by charge, re-grab recharge, foe usable. Burner fuel ~160f, no helpless, refill grounded ~1.5s, gauge blue→yellow→red + blink.
  Files: `fighter/robot/param/vl.prc`, `article/gyro|beam`.
  Editable: `recharge/super_time, gyro_max 1/charge_mult, fuel_max/burn/refill`.
  Blocks on: G-00, G-01, G-03
  Owner: —
- [ ] **G-54 — Rosalina (`rosetta`): Luma HP/respawn/detether**
  Luma mimics even detached, 40 HP (ignores 1v1 mult), tired low HP, extra KB taken. KO = respawn 10s 1v1/7s 3P/5s 4P+. Detached 1.5×. Without Luma Shot/Bits disabled.
  Files: `fighter/rosetta/param/vl.prc`, `article/luma`, `LUMA_STATE/RESPAWN_TIMER`.
  Editable: `hp, respawn_by_players[3], detether_mult, leash`.
  Blocks on: G-00, G-01
  Owner: —
- [ ] **G-55 — Samus family (`samus|samusd`): Charge store + Missile select (merged)**
  Hold-charge ~7 stages to ~25%, store shield/jump/airdodge, air-charge + instant air-fire, cancel without 8f lag. Tap = Homing 5% tracks, smash-B = Super 10% straight fast. Bombs/Morph Ball articles, no other meter.
  Files: `fighter/samus|samusd/param/vl.prc`, `article/chargeshot|missile`.
  Editable: `charge_max/levels, per-level dmg/size/speed, homing_turn/super_speed`.
  Blocks on: G-00, G-01
  Owner: —
- [ ] **G-56 — Sephiroth (`edge`): Wing + Flare stages**
  Wing when behind: higher % + stock/points deficit lowers threshold (~30% −2 → ~110% +2 ahead). 1.1× dmg, faster walk/run/air, 3rd jump, smash armor to 20%, Gigaflare shield-break. Points decay dealing/KOs, lost KO, no regain till next KO. Neutral-B tap Flare (fast far) → mid Megaflare multi → full Gigaflare slow starter → huge delayed boom (bypasses absorbers boom).
  Files: `fighter/edge/param/vl.prc`, `WING_POINTS/ACTIVE`.
  Editable: `threshold_table, mults/speed/armor/jump, decay_on_deal/KO, stage_thresholds/dmg/radius`.
  Blocks on: G-00, G-01, G-02, G-03
  Owner: —
- [ ] **G-57 — Sheik: Needles store (6)**
  Charge to 6, shield-store, B throws all. Ground straight / air 45° down, transcendent, damage falls distance, lost KO.
  Files: `fighter/sheik/param/vl.prc`, `NEEDLE_COUNT`.
  Editable: `max 6, per_frame, near/far dmg`.
  Blocks on: G-00, G-01
  Owner: —
- [ ] **G-58 — Shulk: Arts dial + cooldowns**
  Neutral-B dial: Jump (high jump/fall 6s/18s), Speed (2× run/dash 8s/16s), Shield (0.6× taken + move penalty, damage shortens 6s/18s), Buster (dmg up/KB down +1.3× taken 10s/14s), Smash (0.3× dmg, 1.25× KB dealt, 1.2× taken 8s/16s). Overwrite = old to cooldown, reset KO. Storage/MALLC buffer tech.
  Files: `fighter/shulk/param/vl.prc`, `ART_ID/TIMER/COOLDOWN[5]`.
  Editable: `duration[5], cooldown[5], mults, shield_drain`.
  Blocks on: G-00, G-01, G-02, G-03
  Owner: —
- [ ] **G-59 — Snake: Grenade/C4/Nikita inventory**
  Grenades max 2, cook holding B, fuse ~150f from pull. C4 max 1, stick ground/wall/player, re-press detonate, auto 1600f, falls off player ~12.5s, survives KO. Nikita max 1 steerable, shield-cancel.
  Files: `fighter/snake/param/vl.prc`, `article/grenade|c4|nikita|mortar|claymore`.
  Editable: `max_counts, fuse/cook, stick/auto/falloff, speed/turn/cancel`.
  Blocks on: G-00, G-01
  Owner: —
- [ ] **G-60 — Sonic: Spin levels + Homing + Spring**
  Side-B hold-charge + hop release, 6f start intang, blue→yellow full, ~2s hold, jump-cancel Spin Shot. Down-B mash-charge, no hop, faster multi-hit, ~3s hold, air dive +20% holding forward, shield-cancel landing. Homing lock reticle + bounce. Spring leaves usable article.
  Files: `fighter/sonic/param/vl.prc`, `article/spring`.
  Editable: `charge_max/hold, dmg_vs_charge, intang, spinshot_vel, lock_range`.
  Blocks on: G-00, G-01
  Owner: —
- [ ] **G-61 — Sora (`trail`): Magic cycle HUD**
  Fixed Firaga (mashable straight longest) → Thundaga (3 vertical bolts front, last KOs, shorter air) → Blizzaga (short cone ice, high shield, freeze close/mid-high %). No advance unless cast; startup interrupt keeps queued. All reflect/absorb/pocket. Side-B multi-dash, Down-B front-only 1.4× reflector cap 30% (no B-reverse).
  Files: `fighter/trail/param/vl.prc`, `MAGIC_INDEX` + HUD.
  Editable: `order, per-spell dmg/range/freeze/shield`.
  Blocks on: G-00, G-01, G-03
  Owner: —
- [ ] **G-62 — Steve (`pickel`): Mining + durability/tiers + crafting + blocks/Redstone**
  Mine ground: tool by terrain axe=wood pick=stone/iron shovel=dirt/sand, bare if broken slower; longer = rarer Dirt/Wool/Stone/Wood/Iron/Gold/Diamond/Redstone, stage table. Start stock minimum + ≥3 Iron. Tiers Wood→Stone→Iron→Gold→Diamond; Sword 25/40/50, Axe/Pick/Shovel 70/85/95; broken = punch stub. Craft at table 30 HP auto-spawns, re-summon shield+B costs 2 Wood/4 Stone/1 Iron cheapest-first, respawn 240f, falls offstage waste; hold B repair/upgrade highest affordable (Diamond priority). Dirt 2pts / Wood-Stone 5 / Iron 10 → TNT 50pts, blocks dirt<wood<stone<iron limited count, Minecart Iron, Anvil/Elytra. KO resets tools Wood, keeps mats.
  Files: `fighter/pickel/param/vl.prc` (+ craft-table concept), `article/crafttable|block|tnt|minecart|anvil|elytra`.
  Editable: `mining_weights per stage, durability per tier/tool, craft_costs, tnt_cost/point_values, block_hp/cap, table_hp/respawn`.
  Blocks on: G-00, G-01, G-02, G-03
  Owner: —
- [ ] **G-63 — Terry (`dolly`): GO + Supers + autoface**
  ≥100% (or ≤30% HP stamina) GO flashes icon. ↓↙←↙→/↓←↓→+A/B = Geyser 26/23/20% near/mid/far + late weaker, armor 5–14f, anti-air; ↓↘→↓↘→/↓→↓→+A/B = Buster Wolf dash-grab 5% +20% blast +15% collateral. Ground-only, cancel from normals, blue overlay. Tap/hold weak/strong + command normals. Auto-face 1v1.
  Files: `fighter/dolly/param/vl.prc`, `GO_ACTIVE`, input 22f + hold-extend.
  Editable: `threshold, window, super dmg/armor, autoface`.
  Blocks on: G-00, G-01, G-02, G-03
  Owner: —
- [ ] **G-64 — Villager (`murabito`): Pocket + Timber 4-stage + Lloid**
  Pair with G-27 Isabelle (shared Pocket concept, distinct Tree vs Trap/Rod).
  Pocket intang F1+, stores indefinitely, HUD icon, re-press 1.9× (teammate 0.5×), denies single-instance items. Lloid tap projectile / hold ride (more dmg, helpless unless explodes), 1 alive. Timber 4 presses same spot: plant→water (push+growth 13–18%)→chop→fell 25% + axe 14% if standing, destructible, falls off edges. Balloons poppable.
  Files: `fighter/murabito/param/vl.prc`, `articles lloid|tree|balloon`.
  Editable: `pocket_mul/store/invuln, lloid_max/ride_mult, tree_hp/growth/fell/axe, balloon_hp`.
  Blocks on: G-00, G-01
  Owner: —
- [ ] **G-65 — Wario: Waft + Bike + Chomp**
  Waft persists KO: L0 0% trip F16, L1 12–15.5% F10, L2 half 20–29.9% F8 (best KO), L3 27%+20% headbutt F12 + rise + armor 4–13f + Zoom, bulge/flash 110s (half 55s). Chomp food/items/bike = charge. Bike summon if grounded +6s since loss, 18 HP dismounted, ride armor 60/30 KB, breaks to 2 tires (eat to heal/charge).
  Files: `fighter/wario/param/vl.prc`, `article/bike|tire`.
  Editable: `stage_times/damages/armor/rise, chomp_bonus, bike_hp/cooldown`.
  Blocks on: G-00, G-01
  Owner: —
- [ ] **G-66 — Wii Fit Trainer (`wiifit`): Sun store + Breathing buff**
  Sun hold-charge store via shield/jump, windbox charging, full 21% (tap 5%) + heal 2% on fire. Header headable spike, re-press early head. Breathing ring F~38 32f window fresh: +2% heal + 1.2–1.25× dealt, 0.9× taken, 1.17–1.2× walk/air/fall/grav, 1.1–1.13× dash/run/traction 12.2s fresh / ~8.8s stale; 40s stale lockout; refresh KO.
  Files: `fighter/wiifit/param/vl.prc`, `BREATHING_FRESH_TIMER`.
  Editable: `sun_max/heal/store, ball_physics, window/stale/duration_fresh/stale/mults`.
  Blocks on: G-00, G-01
  Owner: —
- [ ] **G-67 — Yoshi: Egg Lay + Throw + DJ armor**
  Lay tongue grab, egg time `1.8*d+100f` cap, 7%, victim 0.66× in egg, mash −2f/%. Roll speed=dmg. Throw hold=distance stick=angle, bounce, diminishing lift per air use. DJ damage-armor.
  Files: `fighter/yoshi/param/vl.prc`, `article/egg`.
  Editable: `egg_base/per_dmg/cap, mash_reduction, throw_angles/distances, dj_armor_limit`.
  Blocks on: G-00, G-01
  Owner: —
- [ ] **G-68 — Zelda: Phantom stages**
  L1 kick F15 → L2 punch F20 → L3 slash F28 → L4 overhead F38 → L5 rising F50 hold to F120. `base+1.3*(level+1)` 4.6–17.7%. Early launch on attack/special; completed = Zelda free F67 + auto-fire ~1s later. HP + shield L4–5, transcendent + windbox + push. Collapse if Zelda hit pre-launch.
  Files: `fighter/zelda/param/vl.prc`, `article/phantom`.
  Editable: `thresholds[7], formula, hp, auto_delay, free_start`.
  Blocks on: G-00, G-01
  Owner: —
- [ ] **G-69 — Zero Suit Samus: Paralyzer + Flip Jump**
  Charge any level 4–6% + stun/range/duration scale, 24–48f life, d-smash ground stun scales. Whip tipper + pull-in hold. Flip button=spike / stick=kick.
  Files: `fighter/samusd?/param/vl.prc` (verify ZSS internal name on dump; do not guess in code), `article/paralyzer_shot`.
  Editable: `charge_max, stun_vs_charge/victim%, whip_tip/pull, spike_flag`.
  Blocks on: G-00, G-01
  Owner: —
- [ ] **G-75 — Ridley: Plasma store**
  Hold-charge plasma, store via shield (like Samus), release count/size scales; side-B skewer command grab, up-B multi-angle rush.
  Files: `fighter/ridley/param/vl.prc`, `article/plasma`.
  Editable: `charge_max/store_enabled, fireball dmg/size/speed`.
  Blocks on: G-00, G-01
  Owner: —

## Phase 3 — None verification (prove completeness, then close)

These fighters have no persistent meter/resource/partner/RNG/store — ACMD + shared `fighter_param` already cover them. Verify against dump, then mark `[x]` + `Done: verified none`.

- [ ] **G-70 — Star Fox (`fox|falco|wolf`): verify none**
  Blaster/Reflector/Fire Bird/Phantasm/Flash are single-use charge/ACMD. No store/meter.
  Blocks on: G-04
  Owner: —
- [ ] **G-71 — FE swords (`marth|lucina|roy|chrom`): verify none**
  Tipper/uniform/reverse-tipper + Flare Blade hold-charge (not storable) are ACMD. No meter/partner/RNG.
  Blocks on: G-04
  Owner: —
- [ ] **G-72 — Pit family (`pit|pitb`): verify none**
  Upperdash/Orbitars/Flight are fixed articles, no meter/store. Orbitars break visually but no HP meter like Luma.
  Blocks on: G-04
  Owner: —
- [ ] **G-73 — Simon/Richter: verify none**
  Axe/Cross/Holy Water/Uppercut fixed articles, no meter/store/RNG.
  Blocks on: G-04
  Owner: —
- [ ] **G-74 — Solo-none: Falcon, Ganondorf, Meta Knight, Palutena (verify each, then close)**
  Punch turnaround/Knee/Choke/Tornado/Cape/Autoreticle/Flame/Warp are ACMD/status single-use. No store/meter/partner/RNG.
  Blocks on: G-04
  Owner: —

---

## Conventions (do not guess in code)

- Personal path is `fighter/<name>/param/vl.prc` (HDR `vl.prcxml`); resolve exact `param_*` keys via ParamLabels.csv. ZSS/internal-name mismatches (`samusd`, `gaogaen`, `packun`, `tantan`, `pickel`, `trail`, `edge`, `jack`, `brave`, `buddy`, `dolly`, `kamui`, `reflet`, `rosetta`, `shizue`, `murabito`, `wiifit`, `purin`, `koopajr`) must be verified against the dump, never hard-coded from memory.
- Article folders live under `fighter/<name>/article/...` or `article/weapon/...` per fighter; list exact folder during G-04 verification.
- Status logic exports as smashline `status_script` stubs per G-02; never as ACMD `game_` edits.
