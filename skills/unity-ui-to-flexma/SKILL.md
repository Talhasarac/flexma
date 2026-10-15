---
name: unity-ui-to-flexma
description: Export a Unity project's uGUI screens (menus, HUD, popups, shops, inventories, phone apps, settings panels) into Flexma as editable layers, with one page per scene, one frame per screen state and repeated prefabs as components, then keep them in sync on later exports. Use whenever someone wants their game's or Unity app's UI in Flexma, asks to export, sync, push or import Unity canvases, screens or menus to Flexma, wants designers to work on the existing in-game UI, or needs to check Flexma against what Unity shows, even if they never name this skill. Covers uGUI with TextMeshPro, not UI Toolkit.
---

# Unity UI → Flexma

This skill exports every uGUI screen a Unity project actually uses into a Flexma file, as real layers (frames, images, text, masks, constraints, auto-layout grids, components), not screenshots. Re-running it updates the same layers in place, so comments, ids, frame positions and anything designers added in Flexma survive.

The work is done by four editor scripts that ship with this skill in `scripts/`. Your job is to install them, find out how the project shows its screens at runtime, run the export, and check the result against Unity.

Load the `flexma` skill as well. It has the Flexma node spec, which you need when reading results or writing hooks.

## What you need

- **The Unity Editor** with the project open, and a way to drive it. A Unity MCP server with an execute-code tool works best. Otherwise the user clicks the buttons in the exporter window, or you run Unity in batch mode with the project closed.
- **The Flexma MCP server**, to find or create the file and to take screenshots.
- **A Flexma API token** for the uploader, made in Flexma under AI Agents > Connected agents & API tokens. The user pastes it into the exporter window, or sets the `FLEXMA_TOKEN` environment variable for batch mode. Never ask for it in chat, and never read it back, log it, or write it into a file or URL.
- **Unity packages:** TextMeshPro (part of uGUI in Unity 6) and Newtonsoft Json (`com.unity.nuget.newtonsoft-json`). If Newtonsoft is missing, ask before adding it to `Packages/manifest.json`.
- **Unity 2021.3 or newer.** The scripts were built and tested on Unity 6 with HDRP. They don't depend on the render pipeline.

## Ground rules

- **Never modify or save the user's scenes.** The exporter opens scenes additively, copies each canvas into a temporary scene, works only on the copy, and closes everything without saving. If a screen exports wrong, fix it with a hook (below) or in the exporter, never by editing the scene. Some projects mark scenes read-only in AGENTS.md or CLAUDE.md, so read those files first.
- **Follow the project's version-control rules.** Don't commit unless the user asks you to.
- **Uploads edit a live, shared file.** People may be looking at it. The uploader saves a Flexma version before it deletes anything.

## Workflow

### 1. Install the exporter

Copy the four files from `scripts/` into an Editor folder, normally `Assets/Editor/Flexma/`:

| File | Role |
|---|---|
| `FlexmaUIExporter.cs` | Reads the scenes and writes `Library/FlexmaExport/` (`pages.json`, `images/`, `images.json`, `fonts.json`, `excluded.txt`, `summary.txt`) |
| `FlexmaUploader.cs` | Uploads images and fonts (with a hash cache), creates pages, and upserts layers through `upload_nodes` |
| `FlexmaClient.cs` | A small JSON-RPC client for Flexma's `/mcp` endpoint |
| `FlexmaUIExporterWindow.cs` | The Tools > Flexma > UI Exporter window, plus `ExportAndUpload()` and `Batch()` entry points |

If the project already has these files, compare them with the bundled ones before overwriting, and keep any project-specific changes. Let Unity compile, then check the console for errors.

### 2. Choose the scenes and the Flexma file

Ask which scenes hold the UI. The window defaults to the enabled scenes in Build Settings, and each scene becomes one Flexma page. Find the target file with `list_files`, or make one with `create_file` and give the user the link.

Have the user open **Tools > Flexma > UI Exporter** and paste the file link (`https://<host>/file/<id>`) and their token. The server address comes from the link. The link and scene list are saved per project, and the token is saved per machine.

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

Write hooks in `Assets/Editor/Flexma/FlexmaProjectHooks.cs` (see "Project hooks" below). Show the user what you found before the first upload.

### 4. Export and upload

Choose one of these:

- **Agent with Unity execute-code:** run `return FlexmaUIExporterWindow.ExportAndUpload(true);`. It uses the window's saved settings, shows no dialogs, and returns a summary without the token. Pass `false` to export without uploading.
- **The user, by hand:** click Export and upload in the window.
- **Batch, with the project closed in the Editor:**

```bash
FLEXMA_TOKEN=... Unity -batchmode -quit -projectPath . -executeMethod FlexmaUIExporterWindow.Batch -flexmaFile https://host/file/<id> -flexmaScenes Assets/Scenes/Menu.unity,Assets/Scenes/Game.unity
```

The summary looks like `3 pages, 95 frames, 2255 layers, 216 images, 19 fonts, 133 unused objects skipped`, followed by `upload: 0 layers created, 74 updated, 0 removed; …`. Read the warnings in `summary.txt`.

### 5. Check the result

1. Run `get_file_tree` on the file with `depth: 2` to see the pages and frames.
2. Take a `get_screenshot` contact sheet (`node_ids` with several frames, `scale` around 0.3) of the frames that matter most: the main menu, the HUD, the busiest panels, and every frame a hook posed. Look at each one. Check that nothing is blank or white where it should be dark, that text sits in place, that nothing is clipped wrong, and that images aren't stretched.
3. Read `excluded.txt`. It lists inactive objects that nothing references, which were skipped as unused. If one of them is actually a screen (switched on by name, by `Find`, or by an animation), say so.
4. For a pixel check, render Unity next to Flexma. Instantiate the canvas in a temporary additive scene as Screen Space - Camera, with its own camera, a solid background and no post-processing (on HDRP, turn off the Postprocess and Exposure frame settings). Render it at the frame size, then diff it against the screenshot. Apply poses only after the canvas has settled: render once, run `Canvas.ForceUpdateCanvases()`, then pose and render again. Close the scene without saving.

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

## Already handled (useful when debugging)

The exporter already deals with these. Look at them first if a screen looks wrong:

- **ColorTint isn't stored on the Image.** Buttons tint their target graphic at runtime, so the exporter multiplies in the Selectable's normal or disabled color. Without that, white buttons export white.
- **Hidden panels keep stale layout positions.** Layout groups inside inactive panels never ran. The exporter switches on each screen in the copy and runs `LayoutRebuilder.ForceRebuildLayoutImmediate` before reading rects.
- **Atlas sprites report the atlas as `sprite.texture`.** Pixels are read from the source texture through the AssetDatabase.
- **TextMeshPro ignores one trailing line break, but legacy Text doesn't.** The exporter strips one trailing `\n` from TMP text only. Without that, labels sit half a line too high.
- **Unity blends translucent UI in linear light in Linear projects.** Without `linearBlending`, dark overlays look much darker in Flexma.
- **Some panels are active in the scene but hide themselves in Awake or Start.** If they're big (15 or more objects, or at least 15% of the screen), they get their own frame and are left out of Default.

## Known limits

- uGUI only. UI Toolkit (UXML/USS) isn't exported.
- World-space canvases export flat, at their canvas size.
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
