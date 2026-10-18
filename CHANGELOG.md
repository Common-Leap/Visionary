# Visionary Changelog (v1.3)

## Highlights

- **Frame data at a glance.** Pick a move and see its startup, active frames, when you can act
  again, landing lag, and how safe it is on shield. Hover any number to see where it came from.
- **New Motion List editor.** Edit a fighter's motion list (animations, cancel frames,
  intangibility and more) without leaving Visionary. Find it under **Windows → Motion List**.
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
- **See who's being thrown.** On throws, a second fighter is drawn being held and thrown.
  It's Mario by default, but you can pick anyone.
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
  working. Your project's own files win over the game dump when previewing.
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
