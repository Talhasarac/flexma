# Flexma recipes

Copy these into `create_nodes` and adapt the copy, colors and sizes. Each one is a single node spec, and you can nest them.

## Screen shells

| Device | Frame |
|---|---|
| Desktop | `{ "type": "frame", "name": "Desktop / Home", "width": 1440, "layout": "vertical", "fill": "#ffffff" }` |
| Tablet | width 834 |
| Mobile | width 390, with `"padding": [0, 0, 34, 0]` for the home indicator |

Leave `height` out so the screen grows with its sections, or set 1024 (desktop) / 844 (mobile) for a fixed viewport.

## Navigation bar

```json
{ "type": "frame", "name": "Nav", "width": "fill", "layout": "horizontal", "padding": [20, 48],
  "justify": "space-between", "alignItems": "center", "fill": "#ffffff",
  "children": [
    { "type": "text", "name": "Logo", "text": "Acme", "size": 20, "weight": 800 },
    { "type": "frame", "name": "Links", "layout": "horizontal", "gap": 32, "fill": [],
      "children": [
        { "type": "text", "text": "Product", "size": 15, "color": "#444" },
        { "type": "text", "text": "Pricing", "size": 15, "color": "#444" },
        { "type": "text", "text": "Docs", "size": 15, "color": "#444" } ] },
    { "type": "frame", "name": "CTA", "layout": "horizontal", "padding": [10, 18], "radius": 999, "fill": "#111111",
      "children": [{ "type": "text", "text": "Get started", "size": 14, "weight": 600, "color": "#ffffff" }] }
  ] }
```

## Buttons

| Variant | Spec |
|---|---|
| Primary | `{ "type": "frame", "name": "Button / Primary", "layout": "horizontal", "padding": [12, 20], "radius": 10, "fill": "#6d5dfc", "justify": "center", "alignItems": "center", "children": [{ "type": "text", "text": "Continue", "weight": 600, "size": 15, "color": "#ffffff" }] }` |
| Secondary | the same with `"fill": "#ffffff", "stroke": "#d9d9de"` and dark text |
| Ghost | `"fill": []` with accent-colored text |

## Input field with label

```json
{ "type": "frame", "name": "Field / Email", "width": "fill", "layout": "vertical", "gap": 6, "fill": [],
  "children": [
    { "type": "text", "text": "Email", "size": 13, "weight": 600, "color": "#333" },
    { "type": "frame", "name": "Input", "width": "fill", "layout": "horizontal", "padding": [12, 14],
      "radius": 10, "fill": "#ffffff", "stroke": "#d9d9de",
      "children": [{ "type": "text", "text": "you@example.com", "size": 15, "color": "#9a9aa2" }] }
  ] }
```

## Card

```json
{ "type": "frame", "name": "Card", "width": 320, "layout": "vertical", "gap": 12, "padding": 20, "radius": 16,
  "fill": "#ffffff", "stroke": "#ececf0", "shadow": { "y": 8, "blur": 24, "color": "#0000000f" },
  "children": [
    { "type": "rect", "name": "Image: product shot", "width": "fill", "h": 180, "radius": 10, "fill": "#eef0f4" },
    { "type": "text", "text": "Card title", "size": 18, "weight": 700 },
    { "type": "text", "text": "Supporting copy that wraps across lines.", "width": "fill", "size": 14, "color": "#666", "lineHeight": 21 }
  ] }
```

## Grid of cards

Use a horizontal frame with `gap` and give the cards `width: "fill"` so they share the row equally:

```json
{ "type": "frame", "name": "Features", "width": "fill", "layout": "horizontal", "gap": 24, "padding": [64, 48], "fill": [],
  "children": [ { "...card...": "", "width": "fill" }, { "...": "" }, { "...": "" } ] }
```

For multi-row grids, stack several of these rows inside a vertical frame. Wrapping is not supported yet.

## Hero section

```json
{ "type": "frame", "name": "Hero", "width": "fill", "layout": "vertical", "alignItems": "center", "gap": 20,
  "padding": [96, 48], "fill": { "type": "LINEAR_GRADIENT", "angle": 90, "stops": [
    { "position": 0, "color": "#f6f5ff" }, { "position": 1, "color": "#ffffff" } ] },
  "children": [
    { "type": "text", "text": "Design together, in real time", "size": 56, "weight": 800, "align": "CENTER", "width": 800 },
    { "type": "text", "text": "A self-hosted canvas for teams and their AI agents.", "size": 20, "color": "#555", "align": "CENTER", "width": 640 },
    { "type": "frame", "layout": "horizontal", "gap": 12, "fill": [], "children": [ "…primary button…", "…secondary button…" ] }
  ] }
```

## Pricing table

Use a horizontal frame of plan cards, each a vertical frame with these children: plan name, price (size 40, weight 800), a feature list (a vertical frame of text rows, prefixed with "✓"), and a button. Highlight the recommended plan with the accent fill and white text, or with a 2px accent stroke.

## Icons

Use simple vectors on a 24×24 grid with a 2px stroke and no fill:

```json
{ "type": "vector", "name": "Icon / Check", "width": 24, "height": 24, "path": "M5 12 L10 17 L19 7",
  "fill": [], "stroke": "#111", "strokeWeight": 2 }
```

## Diagrams and flowcharts

- **Boxes:** auto-layout frames with a text child, with `padding: [10, 16]`, `radius: 8` and a stroke.
- **Connectors:** `line` nodes using `x1, y1, x2, y2`.
- **Layout:** place the boxes on a grid, e.g. 220px columns and 120px rows. Use `get_nodes` to read `absoluteBounds` when you connect boxes that already exist.

## Design to code

Read the structure with `get_file_tree` and the exact values with `get_nodes`, then map them:

| Flexma | CSS |
|---|---|
| `layout: VERTICAL` / `HORIZONTAL` | `display: flex; flex-direction: column / row` |
| `gap` | `gap` |
| `padding` | `padding` |
| `justify` | `justify-content` |
| `alignItems` | `align-items` (`stretch` too) |
| `width: "fill"` | `flex: 1` along the layout direction, `align-self: stretch` across it |
| hug | `width: fit-content` |
| `cornerRadius` | `border-radius` |
| drop shadow | `box-shadow: x y blur spread color` |
| `strokeAlign: INSIDE` | `box-shadow: inset 0 0 0 <w> <color>`, or `border` with `box-sizing: border-box` |
| text props | font-* and `line-height` (px) |
| text `textRuns` (from `spans`) | inline `<span>`, `<strong>`, `<a>` inside one paragraph |
| `BACKGROUND_BLUR` | `backdrop-filter: blur(<radius>px)` |
| `absolute` + constraints | `position: absolute` with `top`/`right`/`bottom`/`left` |

Use `export_svg` for icons and illustrations.

## Code to design

Recreate components top-down:

1. Make the page or screen frame.
2. Make the layout containers as auto-layout frames.
3. Add the leaves: text, rectangles, vectors.

Name layers after the components (for example `Button / Primary`). Then compare against the original with `get_screenshot`.

From a website (HTML/CSS, Tailwind):

| CSS / Tailwind | Flexma |
|---|---|
| `display: flex` / `flex-col` | `layout: "HORIZONTAL"` / `"VERTICAL"` |
| `gap-*`, `p-*` | `gap`, `padding` |
| `items-stretch`, equal-height cards in a row | `alignItems: "stretch"` |
| `flex-1`, `w-full` | `width: "fill"` (in a row), `height: "fill"` (in a column) |
| `absolute inset-0` | `absolute: true`, `width: "fill"`, `height: "fill"` |
| `absolute top-2 right-2` | `absolute: true`, `x`/`y`, `constraints: { horizontal: "MAX", vertical: "MIN" }` |
| `rotate-3` on an absolute layer | `rotation: 3` (it doesn't change auto layout, like a CSS transform) |
| `backdrop-blur-md` + `bg-white/60` | `backgroundBlur: 12`, `fill: "#ffffff99"` |
| `<a>`, `<strong>`, colored `<span>` in a sentence | `spans` on one text layer |
| `.btn` with modifiers (`.btn-primary`, `aria-selected`) | One component with variants (`variant: { Kind: "Primary" }`) |
| A card with an optional badge or a slot for an icon | Component `properties` (boolean, text, instance swap), set per instance with `props` |
| Repeated markup (`map()` over data) | A component plus `fill_with_data` |
| `<img src="logo.svg">` | `upload_image` the SVG (a crisp PNG), or `import_svg` for editable layers |
| Shared CSS classes for type (`.h1 { color }`) | Text styles with a `color` |

- Download the site's images and SVGs first, then upload them.
- Re-importing a page after the code changed: use the same top-level `key` and `"prune": true`, so sections that were removed or restructured don't stay behind.

## Unity UI to design

To bring a Unity uGUI canvas into Flexma, read the hierarchy and components in Unity (for example through a Unity MCP server), upload the sprites, then rebuild each screen with one `create_nodes` call.

| Unity | Flexma |
|---|---|
| Canvas | A top-level `FRAME` at the Canvas Scaler's reference resolution. |
| RectTransform | `x`, `y`, `width`, `height` from the rect's corners in canvas units, relative to the parent's top-left corner. Unity's y grows upward, so `y = parentTop − childTop`. Copy the anchors as `constraints: { anchorMin, anchorMax }`. |
| Image, Simple | `{ "type": "image", "asset", "fit": "STRETCH" }`. Use `FIT` when Preserve Aspect is on. |
| Image, Sliced | Add `slice: { left, bottom, right, top }` from `sprite.border`, `scale: referencePixelsPerUnit ÷ (sprite.pixelsPerUnit × pixelsPerUnitMultiplier)` and `fillCenter`. |
| Image.color, RawImage.color | `tint` (its alpha fades the image). |
| Text, TextMeshProUGUI | `TEXT` with `text`, `size`, `color` and `align`. Upload the font asset's source .ttf/.otf with `upload_font`. |
| Horizontal or Vertical Layout Group | `layout`, `gap` (spacing), `padding`, and `justify` / `alignItems` from Child Alignment. |
| Content Size Fitter (Preferred Size) | `"hug"` on that axis. |
| Layout Element with flexible size | `"fill"` on that axis. |
| Grid Layout Group | A vertical frame of horizontal rows with fixed-size cells. |
| Mask, RectMask2D | `clip: true` on the frame. |
| CanvasGroup alpha | `opacity`. |
| Shadow, Outline | `shadow`, `stroke`. |
| Prefab, nested prefab | One `COMPONENT`, then instances. Per-instance changes go in `overrides`. |
| Inactive GameObject | `hidden: true`. |

- **Sprites:** export each sprite as a PNG, cropping atlas sprites to `sprite.textureRect`, and upload them with `upload_image` before building.
- **Check:** compare `get_screenshot` with a Game view capture at the same resolution, then resize the top frame to make sure the anchors behave.
