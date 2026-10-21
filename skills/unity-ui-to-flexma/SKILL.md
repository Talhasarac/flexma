---
name: unity-ui-to-flexma
description: Export a Unity project's UI screens (menus, HUD, popups, shops, inventories, phone apps, settings panels, in-world monitors) into Flexma as editable layers, with one page per scene, one frame per screen state and repeated prefabs or UXML templates as components, then keep them in sync on later exports. Use whenever someone wants their game's or Unity app's UI in Flexma, asks to export, sync, push or import Unity canvases, UI documents, screens or menus to Flexma, wants designers to work on the existing in-game UI, or needs to check Flexma against what Unity shows, even if they never name this skill. Covers uGUI (with TextMeshPro) and UI Toolkit (UIDocument, UXML, USS).
---

# Unity UI → Flexma

This skill exports every screen a Unity project actually uses into a Flexma file, as real layers (frames, images, text, masks, constraints, auto layout, components), not screenshots. It reads uGUI canvases (with TextMeshPro) and, when switched on, UI Toolkit documents (UIDocument, UXML, USS). Re-running it updates the same layers in place, so comments, ids, frame positions and anything designers added in Flexma survive.

The work is done by five editor scripts that ship with this skill in `scripts/`. Your job is to install them, find out how the project shows its screens at runtime, run the export, and check the result against Unity.

Load the `flexma` skill as well. It has the Flexma node spec, which you need when reading results or writing hooks.

## What you need

- **The Unity Editor** with the project open, and a way to drive it. A Unity MCP server with an execute-code tool works best. Otherwise the user clicks the buttons in the exporter window, or you run Unity in batch mode with the project closed.
- **The Flexma MCP server**, to find or create the file and to take screenshots.
- **A Flexma API token** for the uploader, made in Flexma under AI Agents > Connected agents & API tokens. The user pastes it into the exporter window, or sets the `FLEXMA_TOKEN` environment variable for batch mode. Never ask for it in chat, and never read it back, log it, or write it into a file or URL.
- **Unity packages:** TextMeshPro (part of uGUI in Unity 6) and Newtonsoft Json (`com.unity.nuget.newtonsoft-json`). If Newtonsoft is missing, ask before adding it to `Packages/manifest.json`.
- **Unity 2021.3 or newer**, or **2022.3 or newer** for UI Toolkit. The scripts were built and tested on Unity 6 (6000.3) with HDRP. They don't depend on the render pipeline.

## Ground rules

- **Never modify or save the user's scenes.** The exporter opens scenes additively, copies each canvas into a temporary scene, works only on the copy, and closes everything without saving. If a screen exports wrong, fix it with a hook (below) or in the exporter, never by editing the scene. Some projects mark scenes read-only in AGENTS.md or CLAUDE.md, so read those files first.
- **Follow the project's version-control rules.** Don't commit unless the user asks you to.
- **Uploads edit a live, shared file.** People may be looking at it. The uploader saves a Flexma version before it deletes anything.

## Workflow

### 1. Install the exporter

Copy the five files from `scripts/` into an Editor folder, normally `Assets/Editor/Flexma/`:

| File | Role |
|---|---|
| `FlexmaUIExporter.cs` | Reads the scenes' canvases and writes `Library/FlexmaExport/` (`pages.json`, `images/`, `images.json`, `fonts.json`, `excluded.txt`, `summary.txt`) |
| `FlexmaUIToolkitExport.cs` | The UI Toolkit half of the exporter: UIDocuments into the same pages, plus Unity's own renders in `renders/` |
| `FlexmaUploader.cs` | Uploads images and fonts (with a hash cache), creates pages, and upserts layers through `upload_nodes` |
| `FlexmaClient.cs` | A small JSON-RPC client for Flexma's `/mcp` endpoint |
| `FlexmaUIExporterWindow.cs` | The Tools > Flexma > UI Exporter window, plus `ExportAndUpload()`, `Include()` and `Batch()` entry points |

If the project already has these files, compare them with the bundled ones before overwriting, and keep any project-specific changes. Let Unity compile, then check the console for errors.

### 2. Choose the scenes and the Flexma file

Ask which scenes hold the UI. The window defaults to the enabled scenes in Build Settings, and each scene becomes one Flexma page. Find the target file with `list_files`, or make one with `create_file` and give the user the link.

Have the user open **Tools > Flexma > UI Exporter** and paste the file link (`https://<host>/file/<id>`) and their token. The server address comes from the link. The link and scene list are saved per project, and the token is saved per machine.

Then choose what to export. uGUI canvases are on by default; UI Toolkit documents are off until you switch them on. If the project has UIDocuments (search for `UIDocument` components, or `.uxml` files outside `Packages/`), ask whether to include them, then tick **Include > UI Toolkit documents** or run `FlexmaUIExporterWindow.Include(canvases: true, uiToolkit: true);`. A project that only uses UI Toolkit can leave canvases off. Both kinds go into the same page per scene.

### 3. Survey how the game shows its screens

Don't skip this step. The exporter sees the scene as it was saved. Anything a script does at runtime is invisible to it unless you account for it. Read the UI scripts and the buttons' onClick targets, and look for these patterns:

| Pattern in the project | What to do |
|---|---|
| A panel is moved, scaled or rotated when it opens (a phone or tablet going full screen, a map zooming in) | Add a `PoseScreen` hook. Ask the user how each screen is usually seen, because that's how designers want it in Flexma. |
| A bezel, mask or frame is hidden for some screens | Use `HideGraphic` in a `PoseScreen` hook |
| Custom `BaseMeshEffect`s or UI shaders (glows, halos, gradients) | Add an `AddEffects` hook that maps them to Flexma shadows or blurs, or accept that they're missing |
| `RawImage`s fed by RenderTextures or camera feeds at runtime | These export as whatever texture is assigned in the scene, or nothing. Tell the user, or have them assign a representative texture. |
| Text filled in at runtime (localization, player names, prices) | The export shows the text saved in the scene. Mention it, and don't invent content. |
| Editor overlays or debug canvases | In the hooks' static constructor, add their names with `FlexmaUIExporter.SkippedCanvasNames.Add("…")` |
| Screens that exist only in prefabs spawned at runtime and in no scene | They aren't exported. Offer to export them by putting the prefab into a scratch scene that you never save. |

For UI Toolkit, look at the scripts that hold a `UIDocument` (`rootVisualElement.Q(...)`) as well:

| Pattern in the project | What to do |
|---|---|
| Elements switched on by script (`style.display = DisplayStyle.Flex`, `RemoveFromClassList("hidden")`) | Already handled: an element hidden with `display: none` gets its own frame when a UI Toolkit script uses its name or one of its classes as a string. Check `excluded.txt` for the ones that weren't found. |
| Text, list items or images filled in at runtime (`label.text = …`, ListView `itemsSource`, `Image.sprite`) | The export shows what the UXML holds. Fill in representative content with a `PoseDocument` hook if the user wants it. |
| A screen state shown by adding a class or changing styles from code | Add the class in a `PoseDocument` hook, keyed on `ctx.Screen`, or accept the default state |
| UIDocuments with no Source Asset that build their UI in code | They're skipped with a warning; they need a hook that builds the tree, or nothing |
| Custom controls that draw with `generateVisualContent` (charts, gauges) | Their own drawing is missing; the layer's description says so |
| The same UIDocument placed many times (a prefab in each level chunk) | Exported once per scene; the copies are listed in `excluded.txt` |

Write hooks in `Assets/Editor/Flexma/FlexmaProjectHooks.cs` (see "Project hooks" below). Show the user what you found before the first upload.

### 4. Export and upload

Choose one of these:

- **Agent with Unity execute-code:** run `return FlexmaUIExporterWindow.ExportAndUpload(true);`. It uses the window's saved settings (including what `Include()` chose), shows no dialogs, and returns a summary without the token. Pass `false` to export without uploading. Exporting a big scene can take longer than an execute-code call waits; if the call times out, the export usually still finishes, so read `summary.txt` before running it again.
- **The user, by hand:** click Export and upload in the window.
- **Batch, with the project closed in the Editor:**

```bash
FLEXMA_TOKEN=... Unity -batchmode -quit -projectPath . -executeMethod FlexmaUIExporterWindow.Batch -flexmaFile https://host/file/<id> -flexmaScenes Assets/Scenes/Menu.unity,Assets/Scenes/Game.unity
```

Add `-flexmaUIToolkit` to include UI Toolkit documents, and `-flexmaNoCanvases` to leave uGUI canvases out.

The summary looks like `3 pages, 95 frames, 2255 layers, 216 images, 19 fonts, 133 unused objects skipped`, followed by `upload: 0 layers created, 74 updated, 0 removed; …`. Read the warnings in `summary.txt`.

### 5. Check the result

1. Run `get_file_tree` on the file with `depth: 2` to see the pages and frames.
2. Take a `get_screenshot` contact sheet (`node_ids` with several frames, `scale` around 0.3) of the frames that matter most: the main menu, the HUD, the busiest panels, and every frame a hook posed. Look at each one. Check that nothing is blank or white where it should be dark, that text sits in place, that nothing is clipped wrong, and that images aren't stretched.
3. Read `excluded.txt`. It lists inactive objects that nothing references, which were skipped as unused. If one of them is actually a screen (switched on by name, by `Find`, or by an animation), say so.
4. For a pixel check, render Unity next to Flexma. Instantiate the canvas in a temporary additive scene as Screen Space - Camera, with its own camera, a solid background and no post-processing (on HDRP, turn off the Postprocess and Exposure frame settings). Render it at the frame size, then diff it against the screenshot. Apply poses only after the canvas has settled: render once, run `Canvas.ForceUpdateCanvases()`, then pose and render again. Close the scene without saving.
5. UI Toolkit frames come with Unity's render already: `renders/<scene>/<frame>.png` in the export folder, drawn from the same throwaway panel the layers were read from, at the panel's texture size (the frame size times the panel's scale). Diff it against a `get_screenshot` of the frame at `scale: 1`. Expect small differences in text antialiasing and bold text (see Known limits). `get_screenshot` currently draws strokes centered on the edge, so UI Toolkit borders (inside strokes) look half a border width wider there than in the editor; zoom into the editor in a browser if a border looks off.
6. For UI Toolkit, frames that kept Unity's positions instead of auto layout say why in `meta.unity.layout` (for example `kept Unity's positions: .label would be at 12,30 … instead of 12,31 …`). Read it with `get_nodes`, or search `pages.json` in the export folder for `kept Unity's positions`.

Fix the problems in hooks or exporter options, then export again. Repeat until the important screens match.

### 6. Later exports

Every layer has a stable `key` built from its GameObject's GlobalObjectId, and frames have `unity:<scene>/<frame name>`. Uploading again updates the same layers. Running an upload twice with no changes in Unity must print `0 layers created, 0 updated, 0 removed`. If the second run still shows updates, something isn't round-tripping, so find out what before you trust the file. The cause is either an exporter bug or a Flexma bug; report Flexma bugs with node ids and the before/after values from `get_nodes`.

- With "Keep frame positions" on, frames that designers moved stay where they put them. Only new frames are placed on the grid.
- Layers that no longer exist in Unity are removed from their frames.
- With "Replace old un-keyed copies" on, same-named frames from older exports that had no keys are deleted, after a version is saved.

## What you get in Flexma

- **One page per scene.** Each root canvas gets a "<Canvas> / Default" frame showing what is visible when the scene starts. Each panel that starts hidden but is switched on at runtime gets its own "<Canvas> / <Panel>" frame. Hidden panels are found three ways: an inactive object referenced by a script field or a UnityEvent, an inactive canvas that something references, or a big object whose own script calls `gameObject.SetActive(false)` in Awake or Start.
- **Frame description and meta.** Each frame's description and `meta.unity` record the scene, the hierarchy path, the state, what switches the panel on, and any pose note. Each layer's `meta.unity` has its path, components, prefab and onClick targets. Use `find_nodes` on meta to jump from Unity to a Flexma layer.
- **Components.** A prefab used two or more times with the same layer structure becomes a COMPONENT on a "Components" page. Its uses become INSTANCEs, and their differences (text, sprites, colors) become overrides. Nested component candidates stay as plain layers. Users can switch this off in the window. Switching it changes the layer structure, so the next upload rebuilds those layers.
- **Linear color space.** In a Linear-space project, pages get `linearBlending: true`, so translucent overlays and text edges match Unity.
- **UI Toolkit documents.** Each root UIDocument gets a "<Document> / Default" frame at its panel's size: the target texture's size divided by the panel's scale, or the exporter's screen size (1920×1080) through the Panel Settings scale mode for screen panels. Each element hidden with `display: none` that a UI Toolkit script names gets a "<Document> / <element>" frame showing that element and its ancestors, like a uGUI panel frame. A UIDocument that starts inactive or disabled but that a script references gets its frames too. Layer keys come from the UIDocument's GlobalObjectId plus the element's path (`Type#name` or `Type.class`, with `[n]` for repeats), so renaming or reordering unnamed elements changes their keys.

How Unity features map to Flexma:

| Unity | Flexma |
|---|---|
| RectTransform anchors and pivots | Position, size, rotation and `constraints` |
| Image (simple, sliced, tiled, filled, preserve aspect) | Image fill with `slice` / `fit:"TILE"` / `filled` / `fit` |
| Sprites packed in an atlas or sharing one texture | One uploaded texture with a `crop` per sprite |
| `Image.color`, plus a Selectable's ColorTint normal or disabled color | Fill `tint` |
| RawImage | Image fill (with uvRect as a crop) |
| TextMeshPro / Text | TEXT: font, weight, size, spacing, alignment, `wrap`, `overflow`, `autoSize`, outline as stroke, underlay as shadow |
| Mask on a sprite / RectMask2D | `clip:"alpha"` (the sprite's shape) or a rectangle clip |
| GridLayoutGroup | `layout:"grid"` with an editable column and row count |
| Other layout groups | The positions they produce, rebuilt for each screen state |
| CanvasGroup alpha | Opacity |
| Shadow / Outline | DROP_SHADOWs (an Outline becomes four hard shadows) |
| Fonts | Uploaded from the project's TTF/OTF, otherwise matched from Google Fonts (Arial and Liberation become Arimo, unknown fonts become Inter) |

How UI Toolkit maps to Flexma. Everything is read from a live panel (`resolvedStyle` and the computed style), so the theme, the USS cascade and flexbox are Unity's own:

| UI Toolkit | Flexma |
|---|---|
| A flex container (`flex-direction`, `justify-content`, `align-items`, `flex-wrap`) | An auto-layout frame (`layout`, `justify`, `alignItems`, `layoutWrap`, `rowGap`), used only where Flexma's layout puts every child exactly where Unity did; otherwise the children keep their positions and `meta.unity.layout` says why |
| `padding` + `border-width`, and the children's margins | Frame `padding` and `gap`. Children whose margins differ from their siblings' (the default theme gives every Label margins and padding) sit in a transparent "<name> (margin)" frame padded by their margins (`meta.unity.box: "margin"`) |
| `space-around` / `space-evenly`, `row-reverse` / `column-reverse` | The same spacing as fixed padding and gap; the children in visual order |
| `flex-grow`, `align-items: stretch`, `width: 100%`, `width: auto` | `FILL` and `HUG` sizing where they reproduce Unity's sizes; other sizes (px, %, min/max) are fixed at their computed value |
| `position: absolute` with `left`/`right`/`top`/`bottom` | `absolute: true` and `constraints` (both insets: `STRETCH`, right or bottom only: `MAX`, percentages: `SCALE`) |
| `background-color`, `background-image` (sprite or texture, `-unity-slice-*` or the sprite's borders, `background-size`, `background-repeat`, `-unity-background-image-tint-color`) | Fills: a color, then an image with `slice` / `fit` / `TILE` / `tint` |
| `Image` element (`sprite`, `image`, `scaleMode`, `tintColor`, `uv`) | An image fill with `fit` and `crop` |
| `border-*-width`, `border-*-color` | An inside stroke when all four sides match, otherwise one "border-<side>" rectangle per side |
| `border-*-radius` | `radius`, per corner |
| `opacity`, `visibility: hidden`, `overflow: hidden` | `opacity`, `visible: false`, `clip` |
| `translate`, `rotate`, `scale` | Position, `rotation` and size |
| Label / TextElement text | TEXT in the element's content box: font, size, weight, italic, color, `-unity-text-align`, `white-space`, `text-overflow` (with `overflow: hidden`), letter spacing, `-unity-text-outline-*` as stroke, `text-shadow` as a drop shadow, Unity 6.2+ `-unity-text-auto-size` as `autoSize` |
| `<ui:Instance template="…">` used two or more times | A component on the Components page, with instances and text/color overrides |
| Unity 6's default theme font (NotInter) | Inter |

## Project hooks

Hooks are static events that the exporter calls. Subscribe to them from an `[InitializeOnLoad]` editor class in the project. They only ever change the temporary copy, never the scene.

- **`FlexmaUIExporter.PoseScreen(ScreenContext ctx)`** runs once per frame, after that frame's objects are switched on and before layout runs.
  - It gives you `ctx.Canvas` (the original canvas) and `ctx.Screen` (the original panel, or null for Default).
  - `ctx.ShowsInside(t)` checks whether the shown panel is `t` or inside it.
  - `ctx.MoveTo(original, pose)` and `ctx.HideGraphic(graphic)` change the copy, and are undone before the next frame. `ctx.Copy(t)` handles anything else, but you must undo those changes yourself.
  - Setting `ctx.Note` adds a pose note to the frame's description and meta.
- **`FlexmaUIExporter.AddEffects(LayerContext layer)`** runs once per layer, with `layer.Layer` (the original object), `layer.Rect` (in local units) and `layer.Scale` (local units to Flexma px). Append effects to `layer.Effects`: `{type:"DROP_SHADOW", x, y, blur, spread, color}` (several stack, the first one lowest) or `{type:"LAYER_BLUR", radius}`.

Find objects through `ctx.Canvas` or `layer.Layer`, never with `FindObjectOfType` / `FindAnyObjectByType`. During an export the scene's copies exist too, and a global search can return a copy. Read field values instead of guessing them.

Example, with made-up project types (`PhoneUI`, `IconGlow`):

```csharp
using Newtonsoft.Json.Linq;
using UnityEditor;
using UnityEngine;

[InitializeOnLoad]
static class FlexmaProjectHooks
{
    static FlexmaProjectHooks()
    {
        FlexmaUIExporter.PoseScreen += PoseScreen;
        FlexmaUIExporter.AddEffects += AddEffects;
        FlexmaUIExporter.SkippedCanvasNames.Add("Debug Console");
    }

    // Phone apps open full screen: the phone moves to its full-screen anchor and the bezel hides.
    static void PoseScreen(FlexmaUIExporter.ScreenContext ctx)
    {
        var phone = ctx.Canvas.GetComponentInChildren<PhoneUI>(true);
        if ((phone == null) || (ctx.Screen == null))
        {
            return;
        }
        foreach (var app in phone.fullScreenApps)
        {
            if (ctx.ShowsInside(app.transform))
            {
                ctx.MoveTo(phone.body, phone.fullScreenAnchor);
                ctx.HideGraphic(phone.bezel);
                ctx.Note = "shown full screen, the way the phone opens it";
                return;
            }
        }
    }

    // A custom glow mesh effect, drawn in Flexma as a soft shadow.
    static void AddEffects(FlexmaUIExporter.LayerContext layer)
    {
        var glow = layer.Layer.GetComponent<IconGlow>();
        if ((glow == null) || !glow.enabled)
        {
            return;
        }
        float size = Mathf.Min(layer.Rect.width, layer.Rect.height) * glow.size * layer.Scale;
        layer.Effects.Add(new JObject
        {
            ["type"] = "DROP_SHADOW", ["x"] = 0, ["y"] = 0, ["blur"] = size, ["spread"] = 0,
            ["color"] = "#" + ColorUtility.ToHtmlStringRGBA(glow.color)
        });
    }
}
```

To check a hook, export, then take a screenshot of the frames it posed. The frame's `meta.unity.pose` shows the note.

UI Toolkit has its own hook, because there is no canvas to pose:

- **`FlexmaUIExporter.PoseDocument(DocumentContext ctx)`** runs once per UI Toolkit frame, after the document's copy is built and the element the frame shows is switched on, before layout.
  - It gives you `ctx.Document` (the original UIDocument, read only), `ctx.Root` (the copy's root element) and `ctx.Screen` (the element this frame shows, in the copy, or null for Default).
  - The copy is rebuilt from its UXML for every frame, so change it freely and don't undo anything: fill labels, add rows to a ScrollView, set an `Image` sprite, add the class a script would add.
  - Setting `ctx.Note` adds a pose note to the frame's description and meta.
- **`FlexmaUIExporter.SkippedDocumentNames`** leaves UIDocuments out by GameObject name, like `SkippedCanvasNames`.

```csharp
// In FlexmaProjectHooks (add using UnityEngine.UIElements;), subscribed in its static constructor:
// FlexmaUIExporter.PoseDocument += PoseDocument;
// Shows the shop with a sample item, the way ShopController fills it at runtime.
static void PoseDocument(FlexmaUIExporter.DocumentContext ctx)
{
    if (ctx.Document.name != "ShopUI")
    {
        return;
    }
    ctx.Root.Q<Label>("coins").text = "1,250";
    var list = ctx.Root.Q<ScrollView>("items");
    list.Add(new Label("Turbo kit  —  400"));
    ctx.Note = "sample coins and items";
}
```

## Already handled (useful when debugging)

The exporter already deals with these. Look at them first if a screen looks wrong:

- **ColorTint isn't stored on the Image.** Buttons tint their target graphic at runtime, so the exporter multiplies in the Selectable's normal or disabled color. Without that, white buttons export white.
- **Hidden panels keep stale layout positions.** Layout groups inside inactive panels never ran. The exporter switches on each screen in the copy and runs `LayoutRebuilder.ForceRebuildLayoutImmediate` before reading rects.
- **Atlas sprites report the atlas as `sprite.texture`.** Pixels are read from the source texture through the AssetDatabase.
- **TextMeshPro ignores one trailing line break, but legacy Text doesn't.** The exporter strips one trailing `\n` from TMP text only. Without that, labels sit half a line too high.
- **Unity blends translucent UI in linear light in Linear projects.** Without `linearBlending`, dark overlays look much darker in Flexma.
- **Some panels are active in the scene but hide themselves in Awake or Start.** If they're big (15 or more objects, or at least 15% of the screen), they get their own frame and are left out of Default.
- **UI Toolkit documents are laid out in a throwaway panel.** Each UIDocument is rebuilt from its UXML in the temporary scene with a copy of its Panel Settings that draws into a temporary texture, so the scene's panels, their render textures and the Game view are left alone, and no project script runs on the copy.
- **Flexma's auto layout has one gap and one padding per frame.** Every candidate auto layout is checked against Unity's computed rects with Flexma's own layout rules before it's used, first with `FILL`/`HUG`, then with fixed sizes, then with margin frames.

## Known limits

- World-space canvases export flat, at their canvas size. So do Unity 6.2+ world-space UI Toolkit panels, at the exporter's screen size.
- UI Toolkit: containers keep Unity's positions instead of auto layout when a child uses `align-self`, has a negative margin, is rotated, is `visibility: hidden` (Flexma's auto layout skips hidden children, Unity keeps their space), or overflows a centered container. `min-*`/`max-*` sizes, `filter`, transitions, vector images, `-unity-text-overflow-position` other than end, `word-spacing` and runtime RenderTexture backgrounds aren't exported.
- UI Toolkit bold: for a font with one weight (like the default theme's), Unity thickens the regular face, while Flexma draws the real bold (700), which looks heavier.
- UI Toolkit text measurement can differ by a pixel in the first export after a script reload, which shows up as a few updated layers on the next upload.
- Animations, tweens and runtime-generated content show their saved state.
- TextMeshPro SDF effects other than outline and underlay (bevel, glow, gradients from materials) aren't exported.
- Custom shaders and mesh effects need an `AddEffects` hook, or they're left out.

## Troubleshooting

| Symptom | Likely cause |
|---|---|
| Compile error about `Newtonsoft` | Add `com.unity.nuget.newtonsoft-json`, after asking the user |
| Upload fails with 401 or 403 | The token is wrong or revoked; have the user make a new one |
| "Export and upload" is greyed out | The window's file link isn't a full `https://host/file/<id>` link, or the token is empty |
| A panel is missing | Nothing references it (check `excluded.txt`), or it's spawned from a prefab at runtime |
| A frame shows the panel in the wrong place or size | The game poses it at runtime; add a `PoseScreen` hook |
| Every re-push updates the same N layers | Something doesn't round-trip. Compare the spec with `get_nodes` for one of those layers. |
| Hundreds of layers created and removed after a change | Expected when the Components switch is flipped or prefabs are restructured. Otherwise, the keys changed (objects recreated or the scene re-saved under a new GUID). |
| No UI Toolkit frames | Include > UI Toolkit documents is off, Unity is older than 2022.3 (see the warnings), or the UIDocuments have no Source Asset |
| A UI Toolkit screen state is missing | Its element isn't hidden with `display: none`, or no script names it (check `excluded.txt`); pose it with a `PoseDocument` hook |
| A UI Toolkit frame's layers have no auto layout | Read `meta.unity.layout` on the frame for the child that didn't line up |
