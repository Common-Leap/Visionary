# Visionary Changelog (v0.3.2)

This fork is now built on Visionary v0.3.1. **New in 0.3.2** is what this fork adds,
**New from Visionary 0.2.0 to 0.3.1** is everything the official releases added since the
fork's last version, and the sections after that are the fork's earlier features.

## New in 0.3.2

- **Game version 13.0.5 works too.** The in-game plugin checks whether you're on 13.0.4 or
  13.0.5 and uses the right addresses for it. On any other version it stays off and says so,
  instead of misbehaving.
- **Boots with the Ozone plugin loader.** The plugin no longer stops the game from starting
  under Ozone.
- **Move a hitbox in time in your own code.** Moving a hitbox to another frame now moves it in
  your linked moveset's code as well. Scripts timed with loops or branches are left alone, and
  you're told why.
- **Grabs end where you set them.** Making a grab last longer now works in game, and the status
  line warns you when one grab box is cut short by another.
- **Effects on any frame.** An effect you move, add or stop on a frame where the script doesn't
  stop now plays in game.
- **Pick who you're grabbing.** Grabs and throws hold the fighter and costume you pick in the
  viewport toolbar. Turn off **Thrown fighter** to hide them.
- **Project files are now version 8.** Visionary opens projects from the official release and
  from older versions of this fork. Older versions of Visionary can't open a version 8 project.

## New from Visionary 0.2.0 to 0.3.1

The node system is still unfinished upstream, so keep backups of your projects.

### Move nodes

- **A new Move nodes tab.** Pick a move slot and build the move out of blocks, starting empty
  or from a starter like Quick strike, Heavy hit, Lunge forward and hit, or Shoot a projectile.
  A 3D preview sits next to the graph, and a panel reads the whole move back in plain words.
- **If blocks** let a move check things while it runs, like being on the ground, a button
  being held, or a gimmick value.
- **Edits update live in the match.** No apply button, restart or export, and new hitboxes
  don't need you to do the move first. A few rare blocks (Final Smash cameras, some attack
  settings) aren't live yet; the top bar names them.
- **Make a move that grabs** with a Go to another action block set to catch. Cancel, slow down
  and remove blocks do what they say, and there's a Code block. Blocks can show their own code.
- Every fighter's values from the game are in the block list, so more can be checked and
  changed from a move.
- Live move edits stay on the fighter you made them for. Editing Cloud's neutral special no
  longer changes Joker's too.

### Gimmicks

- **Character gimmicks.** Build rules that run all the time (when you deal or take damage,
  timers, meters), or start from a ready-made one like extra jumps. Moves can talk to them, and
  adding a rule makes a gimmick for it if there isn't one.
- Rules can change fighter resources on a hit: Cloud's Limit, Hero's MP, Joker's Rebel Gauge,
  Little Mac's KO meter (a full one gives the real KO Uppercut) and Inkling's ink.
- **Partner control.** Rules can make Nana use her neutral special and change Nana's and Luma's
  damage. Setting Luma's state to dead removes her.
- **Steve rules.** Hits can earn materials and upgrade a tool's tier mid-match.
- Gimmicks can run on chosen costumes only.
- Fixed Nana's hits being ignored, which made Ice Climbers gimmicks only work some of the time.

### Effects page

- **Edit effects.** Pick an effect, change how it looks and see it in the preview as you edit.
  Play it alone or in any move that spawns it, and send it to the game when it's connected.
- **Character and Move pickers** at the top. The move list marks the moves that play the
  selected effect, and picking a move lists only that move's effects.
- **Colours** take a picker, a colour code like #FF8800, or exact numbers. The old colour stays
  as a small swatch you can click to put it back. Each spawn's colour, opacity and rate can be
  changed from the move too.
- Each effect file opens again on the effect and part you last had open, and each part keeps
  its own scroll position.
- **Borrowing effects** from other files: the borrow window uses plainer words, has a bigger
  list of effects to replace, can move to another monitor, and no longer slows the app down. A
  borrowed effect you put in moves is no longer greyed out, the Effects page shows where it
  came from with a Remove button, and the original stops listing moves you switched away.
- New effects can be added to moves that have no effect script; the missing function is made
  in your linked source.

### Preview

- **Effects drawn the way the game draws them**, with its own shaders, textures, glow and
  colours. They follow the fighter's bones, fade when killed and hide behind the fighter.
- **The objects a move uses** are shown: projectiles and their flight, boomerangs coming back,
  held and crafted weapons, items, Link's bow, Inkling's roller and squid, and the opponent in
  grabs and throws (picked with this fork's **Thrown fighter**).
- **Weapon swing trails.**
- **A stage picker.** Moves play on the stage, aerials above the ground and ledge moves on the
  ledge.
- The camera follows the fighter, and double-clicking goes back to the side view. Dragging works
  as in this fork: left drag turns, right or middle drag pans.
- The preview no longer stutters when picking fighters or moves, and the timeline keeps its
  height across pages and moves.

### Exporting and the app

- **Exported mods don't need Rust or Visionary.** An exported mod carries the rules you tested
  live and builds its own plugin named after the mod. Keep only one copy of the plugin
  installed: add the mod's own plugin.nro to the mod folder. With Rust and the plugin source,
  it still compiles like before.
- Live editing, Export mod and Export source code no longer mix files.
- Moves with no script can be edited, and the script a move really runs is the one loaded.
- One top bar switches between the Classic editor, Move nodes, Gimmicks, Effects and
  Characters, and fighters, moves, bones and actions have searchable, grouped pickers.
- New characters start from the donor's model and animations.
- Plainer words across the Classic editor, the node screen and block properties.
- A failed live read no longer makes ARCropolis panic, and the Tools menu closes when you open
  one of its windows.

### Still rough upstream

- Only a few fighters have been played much: Mario, Cloud, Hero, Joker, Little Mac, Inkling,
  Steve, Villager, Rosalina, Ice Climbers and Donkey Kong. Most other fighters' mechanics aren't
  recreated yet.
- Some effects may not match the game exactly, and throws were only checked roughly.
- Villager's Lloid Rocket and Steve's minecart aren't drawn in the preview.
- The borrow window can't yet switch effects played inside a facing check or from an object a
  move makes (like Mario's fireball). It lists those moves so you can see them.

## Highlights

- **Frame data at a glance.** Pick a move and see its startup, active frames, when you can act
  again, landing lag, and how safe it is on shield. Hover any number to see where it came from.
- **New Motion List editor.** Edit a fighter's motion list (animations, cancel frames,
  intangibility and more) without leaving Visionary. Find it under **Tools → Motion list**.
- **Your mod's code, read properly.** Visionary understands far more of the ways people write
  movesets, so your moves show what your mod actually does instead of falling back to vanilla.

## Editing your moveset

- Shortcuts in your code (sound macros, named numbers, small helper functions) are shown as
  their real values, and edits keep your code written the way you wrote it.
- Moves that several characters or moves share are shown correctly, and Visionary won't
  overwrite shared code by accident.
- Custom moves you added to the motion list now show up under their real names.
- The status bar tells you where the move you're looking at came from: your code, vanilla, your
  saved project, or the live game. If it's showing vanilla when you expected your mod, it tells
  you why.
- Fewer moves go missing from projects that set up a fighter and its projectiles in the same
  place.

## Viewport

- **Better-looking models.** Elbows, knees and other helper bones now move like they do in game,
  and glow, colour and eye-swap animations play.
- **Hitboxes in the right place.** Hitboxes, hurtboxes and effects follow helper bones too. Bones
  that used to snap to the floor (like Mario's jaw) are fixed.
- **Recolours and shared costumes work.** Costumes that borrow files from other costumes or from
  the base game now load completely, just like they do in game.
- **See who's being grabbed and thrown.** In grabs and throws, a second fighter is drawn being
  held and thrown. It's Mario by default, but you can pick anyone and any costume.
- **True-to-game size.** Characters that a mod makes bigger or smaller are drawn at that size,
  hitboxes included.
- **New camera controls.** Left drag to orbit, right or middle drag to pan, scroll to zoom. The
  ⟲ button resets the view.
- **Settings window.** Click the ⚙ gear to change the interface size, background colour, floor
  grid, bloom, helper bones and camera. Your choices are remembered.

## Projects and setup

- **The fighter list follows your project.** With a project open, the fighters it is about sit
  at the top under "In this project", and everything else is tucked into a collapsed
  "Everything else" group. A fighter you have edited, or that has files in your project, is
  always listed. Right-click any fighter to add it to or remove it from the project, and new
  projects ask which fighters they are for.
- **One Sources window.** File → Sources… shows the game dump, your reference mods, your
  project's files and your moveset code in one place, with what each is for and whether it is
  working. Your project's own models, motions and params win over the game dump when
  previewing; effects still come from the dump.
- **Your moveset link is saved with the project.** Opening a project links its moveset again.
- **First-run setup.** Visionary asks for your ArcExplorer dump first, warns you if the folder
  looks like a mod, then shows the Project Hub.

## Characters and mods

- Characters added on extra costume slots now get their own entry in the fighter list, under
  their own name.
- Before your model preview reloads in game, Visionary checks it for problems that would break it
  and warns you about smaller issues.

## Emulators and live preview

- Works with more emulators: sudachi, suyu, citron and torzu, as well as Eden, yuzu and Ryujinx.
  If you have several installed, Visionary uses the one you played on most recently.
- Live effect previews work when you open a mod folder instead of a full game dump. Point
  **Vanilla dump** in the Eff Editor at your full dump. If a preview can't start, you're told why
  right away instead of waiting for it to time out.

## Fixes

- **Character select no longer gets scrambled** when Visionary's in-game plugin is installed
  alongside roster mods.
- On Windows, the black console window is now hidden unless you turn it on in Settings, and
  builds no longer flash console windows.
- Exporting no longer overwrites the original file of an effect you opened from outside your
  game dump.
- The stage picker no longer covers the viewport toolbar.
- Some tooltips had big gaps in the middle of their text. They read normally now.
