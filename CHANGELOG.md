# Visionary Changelog

## 0.3.2

Frame data for every move, a Motion List editor, game version 13.0.5 and the Ozone loader,
costumes and fighter sizes as the game shows them, a choice of who you grab, and a Settings
window. Visionary also reads far more of the ways people write movesets.

The node system is still unfinished, so keep backups of your projects.

### Changed from 0.3.1

- **The camera drags the other way round.** Left drag now turns around the fighter, and right
  or middle drag pans. Scroll still zooms, double-click still goes back to the side view, and
  the new ⟲ button resets the view. Prefer Blender's buttons? Pick them in Settings.
- **Project files are now version 8.** 0.3.2 opens projects from every earlier version, but
  0.3.1 and older can't open a project once 0.3.2 has saved it. They say so and ask you to
  update rather than opening it.
- **You choose who is grabbed and thrown.** The opponent in grabs and throws is the fighter and
  costume picked under **Thrown fighter** in the viewport toolbar (Mario by default). Turn it
  off to hide them.

### Highlights

- **Frame data at a glance.** Pick a move and see its startup, active frames, when you can act
  again, landing lag, and how safe it is on shield. Hover any number to see where it came from.
- **New Motion List editor.** Edit a fighter's motion list (animations, cancel frames,
  intangibility and more) without leaving Visionary. Find it under **Tools → Motion list**.
- **Your mod's code, read properly.** Visionary understands far more of the ways people write
  movesets, so your moves show what your mod actually does instead of falling back to vanilla.
- **Game version 13.0.5 works too.** The in-game plugin checks whether you're on 13.0.4 or
  13.0.5 and uses the right addresses for it. On any other version it stays off and says so,
  instead of misbehaving.

### Editing your moveset

- **Move a hitbox in time in your own code.** Moving a hitbox to another frame now moves it in
  your linked moveset's code as well. Scripts timed with loops or branches are left alone, and
  you're told why.
- **Grabs end where you set them.** Making a grab last longer now works in game, and the status
  line warns you when one grab box is cut short by another.
- **Effects on any frame.** An effect you move, add or stop on a frame where the script doesn't
  stop now plays in game.
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
- **Build & Deploy** builds your linked moveset and replaces every copy of its plugin on your
  emulator's SD card, so you can test it in one click.

### Viewport

- **Better-looking models.** Elbows, knees and other helper bones now move like they do in game,
  and glow, colour and eye-swap animations play.
- **Recolours and shared costumes work.** Costumes that borrow files from other costumes or from
  the base game now load completely, just like they do in game.
- **Thrown animations.** On throws, the fighter being thrown plays its costume's own thrown
  animation. **Auto** picks the one that matches the throw.
- **True-to-game size.** Characters that a mod makes bigger or smaller are drawn at that size,
  hitboxes included.
- **Camera controls from SSBH Editor.** The camera turns around whatever is in the middle of
  the view, so something you pan to stays put while you orbit. Numpad 1, 3 and 7 look from the
  front, right and top (with Ctrl, from the other side; 9 flips the view), Home frames the
  fighter, Period frames the selected hitbox, and the arrow keys pan.
- **Blender navigation.** In Settings, switch to middle drag to orbit, Shift + middle drag to
  pan and Ctrl + middle drag to zoom. **Emulate 3 button mouse** lets Alt + left drag stand in
  for the middle button on a trackpad.
- **Orientation widget.** Axis balls in the viewport's top right corner show which way you're
  looking. Click one to look along it, or drag them to orbit. Settings can show a cube with turn
  arrows instead, or hide it.
- **World axes.** A red X line and a blue Z line through the origin on the ground show where
  the fighter starts.
- **Camera animations.** Pick a camera animation (like a Final Smash camera) in Settings, and
  the viewport camera plays it along the timeline.
- **Settings window.** Click the ⚙ gear to change the interface size, background colour, floor
  grid, world axes, bloom, helper bones and camera, including how close and how far it draws,
  and save the current view as the one it starts at. Your choices are remembered.

### Projects and setup

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

### Characters and mods

- Characters added on extra costume slots now get their own entry in the fighter list, under
  their own name.
- Before your model preview reloads in game, Visionary checks it for problems that would break it
  and warns you about smaller issues.

### Game, emulators and live preview

- **Boots with the Ozone plugin loader.** The in-game plugin no longer stops the game from
  starting under Ozone.
- Works with more emulators: sudachi, suyu, citron and torzu, as well as Eden, yuzu and Ryujinx.
  If you have several installed, Visionary uses the one you played on most recently.
- Live effect previews work when you open a mod folder instead of a full game dump. Point
  **Vanilla game files** on the Effects page at your full dump. If a preview can't start,
  you're told why right away instead of waiting for it to time out.

### Fixes

- **Character select no longer gets scrambled** when Visionary's in-game plugin is installed
  alongside roster mods.
- Exporting no longer overwrites the original file of an effect you opened from outside your
  game dump (with **Open effect file** or **Browse for ef_….eff**).
- On Windows, the black console window is now hidden unless you turn it on in Settings, and
  builds no longer flash console windows.

## 0.3.1

- **Character and Move pickers** at the top of the Effects page. The move list marks the moves
  that play the selected effect, and picking a move lists only that move's effects.
- **Effect edits are kept** when you switch to another effect file and back, or borrow an
  effect. Saving no longer writes the unedited effects over your project.
- Each effect file opens again on the effect and part you last had open, and each part keeps its
  own scroll position.
- The timeline keeps its height across pages and moves. The colour panel no longer widens when
  you change a colour, and the old colour is a small swatch you can click to put it back.
- A borrowed effect you put in moves is no longer greyed out and can play in those moves. The
  Effects page shows where it was borrowed from, with a Remove button, and the original effect
  stops listing moves you switched away from it.
- Borrowed effects now reach forward smash and other moves whose script name differs from the
  move name.
- The borrow window uses plainer words, has a bigger list of effects to replace, and no longer
  slows the app down while it is open.
- The Tools menu closes when you open one of its windows.

## 0.3.0

Mostly about effects and the 3D preview.

- **An Effects page.** Pick an effect, change how it looks and see it in the preview as you
  edit. Play it alone or in any move that spawns it, and send it to the game when it's
  connected.
- **Colours** take a picker, a colour code like #FF8800, or exact numbers. Each spawn's colour,
  opacity and rate can be changed from the move too.
- **Effects drawn the way the game draws them**, with its own shaders, textures, glow and
  colours. They follow the fighter's bones, fade when killed and hide behind the fighter.
- **The objects a move uses** are shown: projectiles and their flight, boomerangs coming back,
  held and crafted weapons, items, Link's bow, Inkling's roller and squid, and the opponent in
  grabs and throws.
- **Weapon swing trails** and a **stage picker**. Moves play on the stage, aerials above the
  ground and ledge moves on the ledge.
- Effect edits show in the preview right away, and the preview no longer stutters when picking
  fighters or moves.
- The camera turns around the fighter with a right or middle drag (#5), and double-clicking
  goes back to the side view.
- The borrow effects window can be moved to another monitor (#32).
- New effects can be added to moves that have no effect script; the missing function is made in
  your linked source (#35).
- A failed live read no longer makes ARCropolis panic (#38).
- **Exported mods build their own plugin** named after the mod and don't need Visionary. Live
  editing, Export mod and Export source code no longer mix files.
- Moves with no script can be edited, and the script a move really runs is the one loaded.
- Searchable, grouped pickers for fighters, moves, bones and actions. One top bar switches
  between the Classic editor, Move nodes, Gimmicks, Effects and Characters.
- Plainer words across the Classic editor, the node screen and block properties. Blocks can
  show their own code, and there's a Code block.
- Gimmicks can run on chosen costumes only. Cancel, slow down and remove blocks now do what they
  say, and every block updates live.
- New characters start from the donor's model and animations.

### Known limits

- Only a few fighters have been played much: Mario, Cloud, Hero, Joker, Little Mac, Inkling,
  Steve, Villager, Rosalina, Ice Climbers and Donkey Kong. Most other fighters' mechanics aren't
  recreated yet.
- Some effects may not match the game exactly, and throws were only checked roughly.
- Villager's Lloid Rocket and Steve's minecart aren't drawn in the preview.
- The borrow window can't yet switch effects played inside a facing check or from an object a
  move makes (like Mario's fireball). It lists those moves so you can see them.
- A few rare node blocks (Final Smash cameras, some attack settings) don't update live yet. The
  top bar names them.

## 0.2.1

- **Live move edits stay on their fighter.** Editing Cloud's neutral special no longer changes
  Joker's too.
- **Gimmick rules for fighter resources** during attacks: a hit can change Cloud's Limit,
  Hero's MP, Joker's Rebel Gauge, Little Mac's KO meter (a full one gives the real KO Uppercut)
  and Inkling's ink.
- **Partner control.** Rules can make Nana use her neutral special and change Nana's and Luma's
  damage. Setting Luma's state to dead removes her.
- Nana's hits are no longer ignored, so Ice Climbers gimmicks work every time.
- **Steve rules.** Hits can earn materials and upgrade a tool's tier mid-match.
- **Moves that grab.** Put a Go to another action block on a move and pick catch.
- Every fighter's values from the game are in the block list, and about 45 more game calls
  update live.
- Adding a rule, or Tell a gimmick, makes the gimmick if there isn't one.
- Plainer words in gimmick rules, actions and object pickers. Specials read as neutral, side, up
  and down special.
- Fit shows a whole move, new cards don't overlap, and the action search box works.
- A line in the game's own script no longer blocks every edit to Inkling's jab.

## 0.2.0

A prerelease of the node system.

- **The Move nodes tab.** Pick a move slot and build the move out of blocks, starting empty or
  from a starter like Quick strike, Heavy hit, Lunge forward and hit, or Shoot a projectile. A
  3D preview sits next to the graph, and a panel reads the whole move back in plain words.
- **If blocks** let a move check things while it runs, like being on the ground, a button being
  held, or a gimmick value.
- **Character gimmicks.** Build rules that run all the time (when you deal or take damage,
  timers, meters), or start from a ready-made one like extra jumps. Moves can talk to them.
- **Node edits update live in the match.** No apply button, restart or export, and new hitboxes
  don't need you to do the move first.
- **Export Mod Folder doesn't need Rust.** The mod carries the rules you tested live, plus the
  plugin. With Rust and the plugin source, it still compiles the mod like before. Keep only one
  copy of the plugin installed: an exported mod has its own plugin.nro, so add it to the mod
  folder.
