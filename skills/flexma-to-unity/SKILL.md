---
name: flexma-to-unity
description: Build Flexma designs into Unity uGUI exactly as designed, as prefabs made from Flexma's own renders, instead of rebuilding them by hand in code. Use whenever someone wants a Flexma screen, HUD, menu, popup, panel or component in their Unity game, says "implement / build / port / copy this design in Unity", asks why Unity doesn't match Flexma, or is about to recreate a Flexma layout with UI code (Image, TextMeshPro, layout groups, a UI kit), even if they never name this skill. Covers importing, re-importing after design changes, wiring prefabs into game code, checking the result against Flexma, and how to design in Flexma for Unity.
---

# Flexma → Unity

Hand-rebuilding a design in Unity code drifts from the design. Values get retyped, and uGUI can't do some of what Flexma does: clip to rounded corners, blur shadows, draw real font weights. The agent then approximates each one. This skill doesn't rebuild anything. It imports the design:

- **Every layer that paints something** (fills, strokes, corners, shadows, gradients, icons, images) arrives as an image drawn by Flexma's own renderer, exactly as it shows in the design.
  - It is already cut by the frames that clip it. A header inside a rounded window comes with its top corners rounded and its bottom square.
  - Each image's stretchable middle is cut down to a few pixels and set as the sprite's 9-slice border. A 1680×952 window is a ~100 px texture that stretches to any size. Identical images are shared.
- **Single-color layers** (icons, pips, badges, solid pills) come white with their design color as `Image.color`, so code can recolor them (an icon that turns green when maxed). The color is the design's, so nothing changes until code changes it.
- **Text stays live TextMeshPro text.** It uses the design's own font files, one per weight, with the same size, letter spacing, line height and baseline. Code can still change it and translations still work.
- **Constraints become anchors**, so screens resize like Unity UI.
- **Auto layout becomes `FlexmaLayout`**, which runs Flexma's layout rules at runtime. A pill whose number grows still hugs it, and a list reflows when items are added.
- **In a Linear-color-space project**, see-through pixels (a 10% white border, a selected tab) are corrected for Unity's linear blending, so they come out as in the design, not twice as bright.
- **Every import is checked.** Each prefab is rendered in Unity and compared pixel by pixel with Flexma's render of the frame. Expect 99%+; the rest is text anti-aliasing.

Load the `flexma` skill too, for reading files (`get_file_tree`, `get_nodes`, `get_screenshot`).

## What you need

- The Unity project open, and a way to run editor code: a Unity MCP server with an execute-code tool works best. Otherwise the user clicks **Tools > Flexma > Import from Flexma**.
- **Unity 2021.3 or newer** with TextMeshPro (part of uGUI in Unity 6) and Newtonsoft Json (`com.unity.nuget.newtonsoft-json`). Ask before adding Newtonsoft to `Packages/manifest.json`.
- **A Flexma API token**, made in Flexma under AI Agents > Connected agents & API tokens.
  - The user pastes it into the import window. It is kept in EditorPrefs on that machine and shared with the Unity UI exporter.
  - Never ask for it in chat, and never print it or write it into a file.
- **The file link** (`https://host/file/ID`) and the **ids of the frames to import**.

## Workflow

### 1. Install the scripts

| Copy | To | Role |
|---|---|---|
| `scripts/Runtime/*.cs` | `Assets/Flexma/Runtime/` | Runtime, ships with the game: layer ids and sizing (`FlexmaNode`), Flexma's auto layout (`FlexmaLayout`), helpers for game code (`FlexmaScreens`, `FlexmaScrollHug`) |
| `scripts/Editor/*.cs` | `Assets/Editor/FlexmaImport/` | Editor only: importer, pixel check, window |

The runtime scripts must not be in an `Editor` folder. If the project uses assembly definitions, put the runtime ones where game code can reference them. Let Unity compile, then read the console for errors.

### 2. Pick the frames

- Run `get_file_tree` on the file and pick the frames.
- **Screens** are frames at the game's reference resolution (usually 1920×1080). A screen's prefab fills the canvas it's put on.
- **Components** (repeated rows, cards, buttons) import as fixed-size prefabs to instantiate from code.
- **Import the design the user names.** Redesign pages often hold several versions (v2, v3), so ask which one if it isn't clear.

### 3. Import

With execute-code:

```csharp
return Flexma.Import.FlexmaImportWindow.Run("https://host/file/FILE_ID", "frameId1,frameId2");
```

- It uses the saved token and returns a summary: the prefabs, warnings, and each frame's match with Flexma.
- Big imports can outlast the tool call. The import keeps running; read `Library/FlexmaImport/FILE_ID/report.txt` when it's done.
- Output goes to `Assets/Flexma/FILE_ID/`:
  - `Screens/<frame>.prefab`
  - `Sprites/<hash>.png`, named by content, so each image is imported once
  - `Fonts/<Family>-<weight>.ttf` and their `SDF` font assets
- The download is unpacked in `Library/FlexmaImport/FILE_ID/` (`manifest.json`, `refs/`, `check/`).

### 4. Check the result

- Every frame in the summary reads like `Camp - Shop: 99.6% of pixels match`. It lists where the differences are, by layer.
- Open `Library/FlexmaImport/FILE_ID/check/<frame>.png` (Flexma | Unity | differences in red) and look at it.
- Below about 99%, something is wrong:
  - a missing font (see warnings);
  - a layer whose image came out empty (warned; report it);
  - a feature that doesn't import yet (see Known limits).
- Show the user the side-by-side for the screens that matter.

### 5. Use the prefab in the game

- **Canvas.** Put the screen under the game's Canvas, set up like this:
  - **Canvas Scaler**: Scale With Screen Size, reference resolution = the frame's size, match 0.5.
  - In a Linear project, turn on **Vertex Color Always In Gamma Color Space** (`canvas.vertexColorAlwaysGammaSpace = true`), or dark text colors round toward green.
- **Code against the layers; never restyle them.** That means no colors, sizes, sprites, fonts or positions set in code. Change the design in Flexma and import again; otherwise Unity and Flexma drift apart again, which is the problem this skill exists to fix. Layer names are the Flexma layer names, so ask the designer (or rename in Flexma) for clear, unique names on what code touches.
- **`FlexmaScreens`** (namespace `Flexma`) does the wiring. Set `FlexmaScreens.Folder` once to the Resources path of the imports (`"UI/Flexma/FILE_ID/Screens/"` when `Settings.Folder` is `Assets/Game/Resources/UI/Flexma`).

  ```csharp
  using Flexma;
  var screen = FlexmaScreens.Load("Camp - Upgrades", canvas.transform);   // the frame's name, "/" written "-"
  var coins = FlexmaScreens.Text(screen, "Header/Coins/Value");            // names below each other, nearest first
  coins.text = gm.Coins.ToString("N0");                                    // FlexmaLayout re-hugs the pill
  FlexmaScreens.Button(FlexmaScreens.Find(screen, "Buy button"), Buy);
  ```

  | Helper | Does |
  |---|---|
  | `Load(frame, parent)`, `Prefab(frame)` | Instantiate a screen; read one (its samples) without instantiating |
  | `Take(frame, path, parent)` | One layer of a frame, the rest destroyed: each tab's body from its own frame inside one shared window |
  | `Find`, `TryFind`, `Get<T>`, `Text` | Layers by name path. A layer whose own name has a "/" (`Toggle/On`) is found by its whole name |
  | `Kid(parent, i)` | The i-th layer, skipping the `<paint>` image (for layers named after their text) |
  | `Paint(layer)` | The image that draws a layer: on it, or on its `<paint>` child when it reaches past its box (shadows) |
  | `Button(layer, onClick, navigation)` | Click target, hover and press tints on what the layer draws. Pass `Navigation.Mode.None` when the game moves gamepad focus itself |
  | `Template`, `ClearSamples`, `Clone` | Lists from the design's own sample rows (below) |
  | `SetIcon(iconFrame, sprite)` | An icon the data picks, in the design icon's frame and color |
  | `Look.Of(sample)`, `look.Apply(layer)` | A state copied from the layer the design shows in it (below) |
  | `Dim(color)` | A flat dim made in code that darkens like the design's in a Linear project |

- **Lists.** Take the design's sample row as the template, so it looks exactly like the design: move it into a hidden holder with `Template`, remove the other samples with `ClearSamples`, then `Clone` one per item under the list frame. `FlexmaLayout` places them as Flexma would. A row component can also be imported as its own prefab.
- **States** (selected tab, disabled button, toggle off, unaffordable price) come from the design. Keep a `Look.Of(...)` of the layer the design shows in each state and `Apply` it to the layer changing. It copies sprites, tints, opacity and text colors, matched child by child; shapes (icons) keep their own image and take only the state's color. Pass `withPositions: true` for a toggle's knob. When a state appears nowhere in the design, ask for it (see Designing below) rather than inventing it in code.
- **Scrolling lists that hug their rows.** A list that is as tall as its rows until it runs out of room: put `FlexmaScrollHug` on the scroll view (a ScrollRect with a RectMask2D and a fixed height), with `Content` its rows, `Frame` the auto-layout frame holding it and `Column` the fill-height frame above that.
- **Buttons** need a click target; `FlexmaScreens.Button` adds one where the layer draws nothing. Re-imports keep added components.
- **Check the real screen too.** The import check compares the prefab; after wiring, render the game's own screen filled with the design's sample data (an editor script that builds the screen, fills it and renders it with a camera to a PNG) and put it next to `refs/<frame>.png`. That catches wiring mistakes: a template missing a state, a hidden sample, a row in the wrong list.

### 6. Re-import after design changes

- Run the same call again. Layers are matched by their Flexma id, so the prefab updates in place.
  - Components and child objects added in Unity stay.
  - Layers deleted in Flexma are removed, with anything added under them.
- Run the check again and compare with the last numbers.

## Moving a code-built UI to the prefabs

When the game already builds its UI in code (a UI kit, `new GameObject` + `Image`), replace it screen by screen:

1. **Import every frame first**, including component sheets and frames that only show states ("Components", "Missing states"); their layers are where states and templates come from.
2. **Keep the game's side.** Keep the class names, public methods, events and fields other code and editor checks use (they may find fields by reflection). Only the building code changes: build from `Load`/`Take` instead of `new GameObject`.
3. **Keep the game's strings.** Use its localized strings and number formats over the design's sample words; they have translations. New strings go in untranslated in the project's source language; never edit translation tables unless asked.
4. **Gaps in the design** (a state or widget the game needs that no frame shows) reuse the closest piece of the design's kit, cloned and filled, and get listed for the user to design. Don't draw new looks in code.
5. **One screen at a time.** Wire it, render it with sample data, compare with the ref, then move on. Several agents in one Unity editor at once step on each other's compiles.
6. **Delete the old code-built look** once a screen is moved, so the two don't coexist.

### Pitfalls in game code

- **Unity's fake null:** `GetComponent<T>() ?? AddComponent<T>()` never adds, since a missing component isn't C# null. Use `if (!c) c = AddComponent<T>()`.
- **One Graphic per GameObject:** a text layer can't also get an `Image` as a click target; it is its own target.
- **`Destroy` waits until the end of the frame** and never runs in edit mode, so copies made meanwhile copy the doomed objects too. Remove samples with `DestroyImmediate` (as `ClearSamples` does) before cloning.
- **`transform.Find` splits on "/"**, so it can't find a layer named `Toggle/On`; `FlexmaScreens.Find` can.
- **A flat dim or overlay made in code** with the design's alpha looks lighter in a Linear project; pass the color through `FlexmaScreens.Dim`. Imported dims are already corrected.
- **A dialog whose frame includes the screen behind it** loads that whole backdrop with it. Take the dialog layer with `Take`, or design dialogs in lean frames.
- **Long imports outlast tool calls** (around 30 s for most MCP servers). The import goes on; wait for `report.txt` to list every frame before reading prefabs.

## Project hooks

Set these from an `[InitializeOnLoad]` editor class in the project (they run on every import, so keep them idempotent):

- `FlexmaImporter.TextComponent`: the text class imported text gets, e.g. the project's localizing subclass of `TextMeshProUGUI`.
- `FlexmaImporter.FontAssetCreated`: runs for each new font asset, e.g. to give it the project's fallback fonts (CJK, Thai).
- `FlexmaImporter.LayerBuilt`: runs for each layer with its manifest entry, for project-specific components.
- `Settings.Folder`: where imports go. Use a `Resources` folder when game code loads the prefabs by name.

## Designing in Flexma for Unity

These make imports exact and keep them working as the game changes. The `flexma` skill points here.

- **One frame per screen state, at the reference resolution.** The frame's fill is a mockup of the game behind the UI and isn't imported (pass `background: true` to `Run` if it is real UI, like a menu backdrop).
- **Anchor with constraints.** `CENTER`, `MAX` and `STRETCH` become Unity anchors. A HUD corner panel uses `MAX`/`MIN`, and a bottom bar `STRETCH`/`MAX`.
- **Use auto layout for anything whose content changes.** That covers number pills, labels next to icons, lists and button rows. Hug the content (`width: "hug"`); fill what should stretch. Fixed boxes don't grow when text does.
- **Real text for anything live.** Numbers, names, prices and translated words must be text layers, never drawn into shapes or images.
- **Components for repeated things.** Rows, cards and list items are components, so the game can instantiate the component's prefab.
- **Name what code touches**, uniquely within its screen: buttons, labels, lists, icons that change. Keep those names stable; code finds layers by name.
- **Keep moving content away from rounded clipping corners.** Clipping is baked into each layer's image at its design position, so a list row that scrolls into a rounded corner keeps square corners there. Pad lists inside rounded frames by at least the radius, or make the scrolling area a plain rectangle. The importer adds a `RectMask2D` to clipping frames whose children overflow them.
- **See-through layers are corrected against what is behind them in the design.** If something different shows behind at runtime (a translucent overlay above live content), the color can be slightly off there. Translucency over the 3D game world blends as Unity blends.
- **Text: one font family per text layer.** Weights within a text are fine. Every family must be in the Flexma workspace (`list_fonts`); otherwise Flexma draws, and exports, a fallback, and the import warns.
- **Center windows and dialogs with constraints** (`CENTER`/`CENTER`), not left/top, or they hug the top-left corner on wider screens.
- **Let rows grow with the game's strings.** Translations and real values run longer than sample text: rows whose text can grow use auto layout with wrapping text (`width: "fill"`) or room to spare.
- **Design every state the game shows**, each as its own layer or frame: selected and unselected, disabled, toggle on and off, empty lists, full or maxed, locked, warnings, every settings tab, open dropdowns, text inputs, focus rings for gamepad. A state missing from the design gets invented in code, and drifts.
- **Use the game's real strings and number formats in sample text**, so the design shows the real lengths.
- **Dims are black see-through layers** (or `#000` at some opacity) over the whole screen; they import as tints Unity can stretch.
- **Don't overlap see-through layers on other UI** unless that is what shows behind them at runtime; each is corrected against what the design shows behind it.

## Known limits

- Background blur and blend modes other than Normal aren't imported. uGUI has no backdrop blur without a custom shader. The import warns.
- Text outlines and text shadows aren't imported yet (warned). Gradient or image text fills come in as their first solid color.
- Text anti-aliasing: TextMeshPro's edges differ slightly from Flexma's. In Linear projects, light text on dark comes out a little bolder (Unity blends text edges in linear light).
- Rotated layers are imported rotated, but see-through pixels on them aren't corrected for linear blending.
- Flexma's text box sizes are used while a text still says what the design says. Once code changes a text, TextMeshPro measures it (rounded up to whole pixels like Flexma), which can differ by a pixel or two for letter-spaced text.
- The check renders with a plain camera. In HDRP, exposure and tonemapping on that camera can shift colors; compare in the Game view instead.

## Troubleshooting

| Symptom | Likely cause |
|---|---|
| `Flexma answered 401` | Token missing, wrong or revoked; the user makes a new one and pastes it in the window |
| `Flexma answered 400: ... isn't a frame` | The id is a layer inside a frame; import the frame |
| Compile error about `Newtonsoft` | Add `com.unity.nuget.newtonsoft-json`, after asking |
| A text uses TextMeshPro's default font | Its font isn't in the Flexma workspace; add it in Flexma and import again |
| Everything is a little too big or small in the game | The Canvas Scaler's reference resolution isn't the frame's size |
| Dark text looks greenish in the game | The canvas lacks Vertex Color Always In Gamma Color Space |
| A screen matches below ~99% | Read the "differs at" layers and the side-by-side; check warnings |
| A button's hover tints the wrong layer, or gamepad focus jumps twice | Wire it with `FlexmaScreens.Button`; pass `Navigation.Mode.None` when the game moves focus itself |
| List shows the design's sample rows next to the real ones | Samples weren't removed, or were removed with `Destroy` after cloning; use `ClearSamples` first |
| A dim made in code looks lighter than the design | Linear blending; use `FlexmaScreens.Dim` |
| Re-import made new objects instead of updating | The layers were recreated in Flexma (new ids), e.g. by deleting and redrawing; references to the old ones need re-linking |
