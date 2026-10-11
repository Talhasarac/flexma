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
| `alignItems` | `align-items` |
| `width: "fill"` | `flex: 1` along the layout direction, `align-self: stretch` across it |
| hug | `width: fit-content` |
| `cornerRadius` | `border-radius` |
| drop shadow | `box-shadow: x y blur spread color` |
| `strokeAlign: INSIDE` | `box-shadow: inset 0 0 0 <w> <color>`, or `border` with `box-sizing: border-box` |
| text props | font-* and `line-height` (px) |

Use `export_svg` for icons and illustrations.

## Code to design

Recreate components top-down:

1. Make the page or screen frame.
2. Make the layout containers as auto-layout frames.
3. Add the leaves: text, rectangles, vectors.

Name layers after the components (for example `Button / Primary`). Then compare against the original with `get_screenshot`.
