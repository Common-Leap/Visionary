# Visionary Changelog (v0.3.1)

This fork is now built on Visionary v0.3.0, so everything new there (Move nodes, Gimmicks,
the Effects page, effects drawn in the preview, stages, objects and the grab opponent) is
included. The notes below are what the fork adds on top.

## New in 0.3.1

- **Built on Visionary 0.3.0.** Everything in the official 0.3.0 release is here, together
  with everything this fork adds.
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
