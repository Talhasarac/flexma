---
name: flexma
description: Design UIs, mockups, wireframes, landing pages, app screens, components, diagrams and graphics in Flexma (a self-hosted, Figma-like collaborative design tool) through the Flexma MCP server. Use whenever the user asks to create, edit, inspect, review or export a design, mentions Flexma or a Flexma file/link, says "the selected thing" while working in Flexma, or wants to turn a description or code into a design (or a design into code), even if they never say "Flexma tools".
---

# Designing in Flexma

Flexma is a collaborative design canvas. You edit the **same live document** people are looking at: every change you make appears on their screens immediately, with your name next to it. Work like a careful senior product designer pairing with them.

For Unity projects there is an optional companion skill, `unity-ui-to-flexma`, that exports a game's uGUI screens into Flexma as editable layers and keeps them in sync. If the user wants their Unity UI in Flexma and that skill isn't installed, point them to Flexma's AI Agents page for its install command (`curl -fsSL <flexma-url>/agents/install/unity-ui-to-flexma.sh | sh`).

The MCP server is usually registered as `flexma`, so tools appear as `mcp__flexma__<tool>` in Claude Code, `flexma.<tool>` in Codex and `flexma_<tool>` in OpenCode. If no Flexma tools are available, tell the user to connect the server (see "Setup" at the end) instead of guessing.

## Mental model

- **Workspace → project → file → page → nodes.** A file has one or more pages, and each page is a free canvas.
- **Node types:**
  - `COMPONENT`: a reusable main component, a frame-like container. `INSTANCE`: a live copy of a component (see "Components" below).
  - `FRAME`: a container. Frames are artboards and screens, and they also make cards and buttons. They can clip their content and use auto layout.
  - `GROUP`
  - `RECTANGLE`, `ELLIPSE`, `LINE`, `POLYGON`, `STAR`
  - `TEXT`
  - `VECTOR`: SVG path data.
  - `PAINT`: a paint layer that people paint on with the brush (in painting files, or files with painting tools switched on). Its pixels live outside the document, so you can move, resize, hide or delete one, and screenshots and exports show it, but tools can't paint or read its pixels.
- **Coordinates** are pixels, with y pointing down. `x`/`y` are relative to the parent node. Rotation is in degrees, clockwise, around the node's center.
- **Auto layout** (`layout: "VERTICAL" | "HORIZONTAL"`) works like CSS flexbox. It positions children for you, so don't set `x`/`y` on children of auto-layout frames.
- **Colors** are hex strings: `#RRGGBB` or `#RRGGBBAA`.
- **Ids** look like `aX3k9:4`. Use the ids the tools return. Never invent them.

## Workflow

1. **Find the file.** Use `list_workspaces`, then `list_files` (filter by `workspace_id` or `project_id`). If the user gave a link like `https://host/file/<id>`, the id is the last path segment. Create a new file with `create_file` when asked, and give the user the link it returns.
2. **Look before you touch.** Call `get_file_tree` to see the pages and what is already there. Use `node_id` and `depth` to drill into big files. If the user says "this", "the selected…" or "here", call `get_selection` to find out what they have selected right now.
   - In an existing design, call `audit_design` and `list_styles` first, then reuse its colors, type styles, spacing and components instead of inventing new values.
3. **Build in sections.** Make one `create_nodes` call per meaningful chunk (a screen shell, a nav bar, a hero, a card grid), with nested `children`. That is atomic, fast, and appears to collaborators as one step. Leave out `x`/`y` on top-level nodes to auto-place new work beside existing content, so you never cover someone's work.
4. **Check your work visually.** Call `get_screenshot` on the frame you built and actually look at it:
   - Is the alignment right?
   - Does any text overflow, and are any elements clipped?
   - Is there enough contrast, and is spacing consistent?
   - Does the hierarchy read clearly?

   Fix problems with `update_nodes`, then check again. Do at least one screenshot pass for anything non-trivial. `audit_design` on your frame catches contrast, off-grid spacing and off-scale type that are easy to miss in a screenshot.
5. **Checkpoint before big changes.** Before redesigning, deleting or restructuring existing work, call `save_version` with a clear label. People can then roll back from Version history. If one of your own steps went wrong, `undo_agent_changes` reverses your last tool calls (people's later edits are kept).
6. **Refine, don't rebuild.** Prefer `update_nodes` (change props), `move_nodes` (re-parent or reorder) and `duplicate_nodes` over deleting and recreating. That keeps the ids collaborators and comments refer to.
7. **Report back.** Say briefly what you made, where it is (the page and frame name), and the file link.

## Components (design systems)

Anything that repeats (buttons, inputs, cards, nav items, list rows, avatars) should be a component. Change it once and every instance updates.

1. **Make one.**
   - Build it as a normal frame with `create_nodes`, with clear layer names (e.g. `Label`, `Icon`).
   - Call `create_component` with that frame's id, or create it directly with `{type: "component", ...}`.
   - Put components together on a page named "Components" when building a system.
2. **Use it.** `create_nodes` with `{type: "instance", component: "Button", text: "Buy now"}`.
   - `text` overrides the component's first text layer.
   - For anything else, use `overrides: { "<layer name or id>": { color: "#fff", text: "…", visible: false } }`.
   - Swap a nested instance (an icon) for another component: `overrides: { "Icon": { component: "Icons/Star" } }`. One nav item component then serves every icon.
   - States (selected, active, size) are variants: components sharing a name with a `variant` object; instances choose with `variant: { Selected: "true" }`. Named knobs (show a badge, a title, an icon) are component `properties` that instances set with `props`. See the node spec.
   - Instance-level styling like `fill` or `stroke` is also an override.
   - Instances look the component up by name or id. It can be made in the same `create_nodes` / `upload_nodes` batch, as long as it comes before its instances (e.g. a "Components" frame first, then the screens).
3. **Customize one instance.** `update_nodes` works on an instance's inner layers through their ids from `get_file_tree` (they look like `instanceId~layerId`). These edits become overrides on that instance. Deleting an inner layer hides it.
4. **Change all instances.** Edit the main component's layers. Overrides survive.
5. **Discover existing ones.** Call `list_components` before building UI in an existing file, and reuse what's there. Call `detach_instance` only when a copy truly needs to diverge.

## Styles

Shared color and text styles keep a design consistent: change a style once and every layer using it updates, for everyone.

- **See them:** `list_styles`. Prefer existing styles over raw values.
- **Make them:** `create_styles` with `{type:"color", name:"Brand/Primary", color:"#6d5dfc"}` or `{type:"text", name:"Heading/H1", font:"Inter", size:40, weight:800, lineHeight:48}`. Group with `/`. The same name again updates the style.
- **Use them:** `fillStyle`, `strokeStyle` or `textStyle` (a style's name or id) in `create_nodes` / `update_nodes`. `null` detaches. Setting `fill` (or a font prop) directly on a styled layer detaches that style too.
- **Change them:** `update_styles` with `{style:"Brand/Primary", color:"#e11d48"}` (add `rename` to rename). `delete_styles` unlinks layers, which keep their look.

## Icons, SVG and data

- **Icons:** `search_icons` finds icons in the built-in Lucide set (about 1,900 outline icons) by name or meaning. Place one with `{ "type": "icon", "icon": "search", "size": 24, "color": "#111111" }` anywhere in a `create_nodes` tree, e.g. inside a button's auto-layout row. Never draw icons by hand.
- **SVG:** `import_svg` turns SVG markup (a logo, illustration, chart or diagram you wrote or found) into editable vector layers. Give `width` to scale it; `flatten: true` merges same-styled shapes into one layer.
- **Data:** `fill_with_data` makes one copy of a component (or any layer) per row: `{ component: "User card", columns: 3, rows: [{ "Name": "Ada", "Role": "Engineer", "Avatar": { "image": "<asset id>" } }] }`. Keys are layer names; strings set text, objects set props, `null` hides a layer, `_self` styles the copy. Use it for lists, tables, feeds and grids with realistic content.

## Rich text

- One text layer can mix styles with `spans`: `{ "type": "text", "spans": ["Read the ", { "text": "terms", "color": "#2563eb", "underline": true, "link": "/terms" }, " first."] }`. Use it for inline links, bold words and multi-color headlines instead of placing separate text layers side by side (those break when the line wraps).
- People style selected characters while editing with Cmd/Ctrl+B, I and U.

## Images

- **Upload:** call `upload_image` (say how many with `files`) and run the `curl` command it returns once per image. Each upload prints an asset id and a ready-made node. SVG files work too: they become a sharp PNG at the SVG's own size (for a logo band or an illustration). Use `import_svg` instead when the SVG should become editable layers.
- **Place:** put `{ "type": "image", "asset": "<id>" }` in `create_nodes`.
  - Leave out width and height to keep the picture's proportions (longest side up to 1200), or give one side and the other follows.
  - `fit`: `FILL` (default, crops to cover), `FIT` (letterbox), `STRETCH` or `TILE` (repeats at `tileScale`).
  - It's a rectangle with an image fill, so `radius`, `stroke`, `shadow` and `opacity` work, and it can sit inside auto layout.
- **As a background:** any frame or shape can take `fill: { "type": "IMAGE", "asset": "<id>", "fit": "FILL" }`, e.g. a hero frame with text on top.
- **Game UI:** `slice` gives 9-slice scaling (a sliced Unity sprite), `tint` multiplies the color (Unity's `Image.color`), and `filled` shows part of the image like a Filled image (bars and radial wheels), and `crop` picks a sprite out of an atlas, on the image shorthand or any `IMAGE` paint.
- **Masks:** a frame with an `IMAGE` fill and `clip: "alpha"` shows its children only where the image isn't transparent (Unity's `Mask`). Add `hideFill: true` to mask without drawing the image.
- **Reuse:** `list_images` shows pictures already in the workspace and which ones this file uses. Uploading the same picture again returns its existing asset id, so re-exports don't pile up copies.
- **From the web:** download the picture first (`curl -L -o photo.jpg <url>`), then upload it. Only use images the user has the rights to. For placeholders, a solid color or gradient block is fine.

## Working with comments

People leave feedback as comment pins on the canvas. When asked to "address feedback" or "handle comments":

1. Call `list_comments`. Each thread says which node it's pinned to.
2. Make the change with `update_nodes`, `create_nodes` or other edit tools.
3. Reply with `add_comment` (`reply_to: <thread id>`) saying what you changed.
4. Call `resolve_comment`.

If something is ambiguous, reply with a question instead of guessing, and leave the thread open. You can also leave your own comments (`add_comment` with `node_id`) to explain non-obvious design decisions.

## Design rules of thumb

- **Use auto layout for UI.**
  - Screens: a vertical frame with `width: 1440` (desktop) or `390` (mobile).
  - Rows: horizontal frames.
  - Spacing: use `gap` and `padding` instead of manual positioning.
  - Children use `width: "fill"` to stretch across their parent. Parents hug their content by default when you leave width/height out.
- **Use a spacing scale:** 4, 8, 12, 16, 24, 32, 48, 64. Use a radius scale: 4, 8, 12, 16, 999 for pills.
- **Use a type scale:**
  - 12 or 13 for captions
  - 14 to 16 for body text
  - 18 to 20 for subheadings
  - 24, 32, 40 or 48 for headings
  - Weights are 400 for body, 600 for labels and buttons, and 700 to 800 for headings.
  - For long text, set `width: "fill"` (or a fixed width) so it wraps.
- **Fonts:** Inter is always available. For anything else, use a family from `list_fonts` (the workspace's fonts), or add a Google Fonts family with `add_font` *before* using it. For the user's own font files (a brand or game font on their computer), call `upload_font` and run the `curl` command it returns once per file. A font that isn't in the workspace shows as Inter for most people and in exports, and the tool result warns you. One or two families per design is plenty.
- **Color:** use one accent color, neutral grays for text (`#111` for primary, `#555` to `#666` for secondary) and light surfaces (`#fff`, `#f6f6f8`). Check contrast on colored backgrounds.
- **Names:** give layers meaningful names ("Header", "Primary button", "Plan card / Pro"). People read the layers panel, and so will the next agent.
- **Notes:** put intent, specs or code hints in a node's `description` (Notes). They are visible to humans and agents.
- **Existing work:** don't restyle or move other people's content unless asked. When editing, keep the file's existing visual language: inspect nearby nodes with `get_nodes` and reuse their colors, fonts and radii.

## Tool cheat sheet

| Tool | Use it to |
|---|---|
| `list_workspaces` | See workspaces and their projects |
| `list_files` | Find files (newest first) |
| `create_project`, `create_file`, `create_page` | Make containers |
| `get_file_tree` | Get a compact outline: id, type, name, position, size, key styles |
| `get_nodes` | Read full props of specific nodes (includes `absoluteBounds`) |
| `find_nodes` | Search by name, text or type |
| `get_selection` | See what live collaborators have selected |
| `list_components` | The file's main components (its design system) |
| `create_component` | Turn a frame or group into a reusable component |
| `detach_instance` | Turn an instance into a plain, independent frame |
| `save_version` | Checkpoint the file in its version history before risky edits |
| `undo_agent_changes` | Undo your own last tool calls in a file (people's later edits are kept) |
| `audit_design` | Colors, type, spacing, radii and fonts in use, plus contrast and consistency problems |
| `list_styles`, `create_styles`, `update_styles`, `delete_styles` | Shared color and text styles |
| `search_icons` | Find Lucide icons; place with `{type:"icon", icon:"<name>"}` |
| `import_svg` | SVG markup → editable vector layers |
| `fill_with_data` | One component instance or layer copy per data row |
| `upload_nodes` | Big node trees from a JSON file (a `curl` link) instead of pasting them into `create_nodes` |
| `list_fonts` | See the workspace's fonts (uploaded and Google Fonts) and their weights |
| `add_font` | Add a Google Fonts family to the workspace so everyone sees it |
| `upload_font` | Upload the user's own .ttf/.otf files: returns a 15-minute `curl` upload link |
| `upload_image` | Upload PNG/JPEG/GIF/WebP images: returns a 15-minute `curl` upload link |
| `list_images` | Images already in the workspace, to reuse |
| `share_file` | Get a public view-only link (no account needed) to hand to someone, or turn it off |
| `list_comments` | Read open feedback threads (who, where, which node, replies) |
| `add_comment` | Pin a note on a node or point, or reply to a thread (`reply_to`) |
| `resolve_comment` | Resolve a thread you addressed (or reopen it) |
| `create_nodes` | Create nested nodes in one step |
| `update_nodes` | Change props: `[{ id, ...props }]` |
| `move_nodes` | Re-parent and/or reorder (keeps the canvas position by default) |
| `duplicate_nodes` | Copy nodes next to the originals |
| `delete_nodes` | Delete nodes and their children |
| `get_screenshot` | Render a node or page to PNG so you can see it. `node_ids` (up to 24) puts several frames side by side in one labelled image. Layers without an opaque fill show transparency as a grey checkerboard |
| `export_svg` | Get SVG markup, e.g. for icons or handoff |
| `export_png` | Save nodes or a page as PNG files (`scale` default 2, `node_ids` for several): returns one `curl` command per image with a 15-minute download link; run them to save the files. Use this when the user wants image files, `get_screenshot` when you only need to look |

See `references/node-spec.md` for every property and shorthand, and `references/recipes.md` for ready-made patterns: screens, buttons, inputs, cards, nav bars, pricing tables, and design-to-code mapping.

## Minimal example

```json
{
  "file_id": "<file id>",
  "nodes": [{
    "type": "frame", "name": "Sign in", "width": 390, "layout": "vertical",
    "padding": [48, 24], "gap": 16, "fill": "#ffffff",
    "children": [
      { "type": "text", "name": "Title", "text": "Welcome back", "size": 28, "weight": 700 },
      { "type": "text", "name": "Subtitle", "text": "Sign in to continue", "size": 15, "color": "#666666", "width": "fill" },
      { "type": "frame", "name": "Email field", "width": "fill", "layout": "horizontal", "padding": [12, 14],
        "radius": 10, "stroke": "#d9d9de", "fill": "#ffffff",
        "children": [{ "type": "text", "text": "you@example.com", "color": "#999999", "size": 15 }] },
      { "type": "frame", "name": "Primary button", "width": "fill", "layout": "horizontal",
        "justify": "center", "padding": [14, 16], "radius": 10, "fill": "#6d5dfc",
        "children": [{ "type": "text", "text": "Sign in", "color": "#ffffff", "weight": 600, "size": 15 }] }
    ]
  }]
}
```

Then call `get_screenshot` with the returned frame id and adjust.

## Gotchas

- Errors name the bad property and list the valid ones. Fix the spec and retry; don't drop the property silently.
- Text boxes are measured on the server with the workspace's fonts, the way the editor measures them, so tool results and screenshots show their real size. Text in a font the workspace doesn't have (Arial, Georgia, …) is sized with Inter until a browser opens the file and measures it.
- `x`/`y` on children of an auto-layout frame are ignored. To pin a child, set `absolute: true`.
- Resizing a frame or instance moves children that have `constraints` (like Unity anchors). Children with the default `MIN`/`MIN` stay where they are.
- `move_nodes` is the only way to change a parent; `update_nodes` rejects `parent`/`index`.
- Images and fonts: upload files with `upload_image` / `upload_font` (run the `curl` command they return); don't paste file contents into tool calls.
- Big trees (an exported game UI, hundreds of frames): write the specs to a JSON file and send it with `upload_nodes` instead of pasting huge `create_nodes` payloads. Each batch is one undoable step; split very large exports by screen.
- Unity projects in Linear color space: create their pages with `linearBlending: true` so translucent overlays and text edges match Unity.
- Keep source data with the design in `meta` (any JSON object), e.g. the Unity component or a button's action. `find_nodes` can search it by `meta` entries or by `key`.
- Re-exports: give every node a stable `key` (e.g. its Unity object path). Uploading again then updates the same nodes in place (ids, comments and people's additions survive) instead of adding copies. Don't delete and recreate screens.
- Fixed text boxes (TextMeshPro rects): give `width` and `height` with `overflow` (`CLIP`, `ELLIPSIS` or `TRUNCATE`) or `autoSize: [min, max]` (shrinks the font to fit); `wrap: false` keeps each paragraph on one line.
- Text outlines: give TEXT a `stroke`, `strokeWeight` and `strokeAlign` (`OUTSIDE` keeps the letters' fill intact; `CENTER` matches TextMeshPro outlines). They render in the editor and in exports.
- Rendering limits: drop shadows (blur, `spread`, several per layer) on shapes, frames and text look the same in the editor and in exports. The editor shadows a frame's box, while exports shadow everything it paints, so they differ only for frames without a fill. Vectors show their shadows in exports only, and layer blur too. Without a real picture, use a neutral placeholder rectangle named "Image: …".
- Styles win: if a spec has both `fillStyle` and `fill` (or `textStyle` and `size`), the style's values apply. To use a raw value, leave the style out (or set it to null).
- Icons: recolor with `update_nodes {id:"<icon frame>", color:"#…", strokeWidth:1.5}`. `strokeWidth` is in Lucide's 24-unit grid and scales with the icon's size. Old icon names still work (`home` → `house`).
- Large files: outline with `depth: 2` first, then zoom into the frames you need with `node_id`.

## Setup (if the tools are missing)

Flexma's MCP endpoint is `https://<your-flexma-host>/mcp` (Streamable HTTP, OAuth or bearer token). Unlimited connections are allowed.

- **Claude Code:** run `claude mcp add --scope user --transport http flexma https://<host>/mcp`, then `/mcp` to sign in through the browser.
- **Codex:** run `codex mcp add flexma --url https://<host>/mcp`, then `codex mcp login flexma`. For non-interactive `codex exec`, also set `default_tools_approval_mode = "approve"` under `[mcp_servers.flexma]` in `~/.codex/config.toml`.
- **OpenCode:** run `opencode mcp add flexma --url https://<host>/mcp`, then `opencode mcp auth flexma` to sign in through the browser.
- **Headless:** create a token in Flexma → AI Agents and send `Authorization: Bearer <token>`.
- **Skill:** reinstall it with `curl -fsSL https://<host>/agents/install.sh | sh`.
