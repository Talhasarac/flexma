---
name: flexma-to-unity
description: Build Flexma designs into Unity uGUI exactly as designed, as prefabs made from Flexma's own renders, instead of rebuilding them by hand in code. Use whenever someone wants a Flexma screen, HUD, menu, popup, panel or component in their Unity game, says "implement / build / port / copy this design in Unity", asks why Unity doesn't match Flexma, or is about to recreate a Flexma layout with UI code (Image, TextMeshPro, layout groups, a UI kit), even if they never name this skill. Covers importing, re-importing after design changes, wiring prefabs into game code, checking the result against Flexma, and how to design in Flexma for Unity.
---

# Flexma → Unity

Hand-rebuilding a design in Unity code drifts from the design. Values get retyped, and uGUI can't do some of what Flexma does: clip to rounded corners, blur shadows, draw real font weights. The agent then approximates each one. This skill doesn't rebuild anything. It imports the design:

- **Every layer that paints something** (fills, strokes, corners, shadows, gradients, icons, images) arrives as an image drawn by Flexma's own renderer, exactly as it shows in the design.
  - It is already cut by the frames that clip it. A header inside a rounded window comes with its top corners rounded and its bottom square.
  - Each image's stretchable middle is cut down to a few pixels and set as the sprite's 9-slice border. A 1680×952 window is a ~100 px texture that stretches to any size. Identical images are shared.
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
| `scripts/Runtime/FlexmaNode.cs`, `FlexmaLayout.cs` | `Assets/Flexma/Runtime/` | Runtime: layer ids and sizing, Flexma's auto layout. Ships with the game. |
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
- **Find layers by name**, and code against the layers rather than restyling them. Layer names are the Flexma layer names, so ask the designer (or rename in Flexma) for clear, unique names on what code touches.

  ```csharp
  var screen = Instantiate(upgradesPrefab, canvas.transform);
  var coins = screen.transform.Find("Camp window/Header/Coins/Value").GetComponent<TMP_Text>();
  coins.text = gm.Coins.ToString("N0");          // FlexmaLayout re-hugs the pill
  var buy = screen.transform.Find("…/Buy button").gameObject.AddComponent<Button>();
  buy.onClick.AddListener(Buy);
  ```

- **Never restyle in code.** That means colors, sizes, sprites, fonts or positions. Change the design in Flexma and import again; otherwise Unity and Flexma drift apart again, which is the problem this skill exists to fix.
- **Lists.** Import the row or card component as its own prefab and `Instantiate` one per item under the list frame (an auto-layout frame). `FlexmaLayout` places them as Flexma would. Delete the sample rows the design shows, or hide them.
- **States** (selected tab, disabled button) are separate layers or components in the design. Switch them with `SetActive`, or swap prefab instances, not by tinting.
- **Buttons** need a `Button` (and an `Image` as its target graphic, which a layer with a fill already has). Add them in code or on the prefab; re-imports keep added components.

### 6. Re-import after design changes

- Run the same call again. Layers are matched by their Flexma id, so the prefab updates in place.
  - Components and child objects added in Unity stay.
  - Layers deleted in Flexma are removed, with anything added under them.
- Run the check again and compare with the last numbers.

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
| Re-import made new objects instead of updating | The layers were recreated in Flexma (new ids), e.g. by deleting and redrawing; references to the old ones need re-linking |
