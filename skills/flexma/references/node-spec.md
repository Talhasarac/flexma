# Flexma node spec

This reference covers every property accepted by `create_nodes` and `update_nodes`.
Property names are case-sensitive. Shorthands (marked *alias*) are converted
to the canonical names. Unknown names return an error that lists the valid ones.

## Common to all nodes

| Property | Type | Default | Notes |
|---|---|---|---|
| `type` | string | required | `FRAME`, `GROUP`, `RECTANGLE`, `ELLIPSE`, `LINE`, `TEXT`, `VECTOR`, `POLYGON`, `STAR`, `IMAGE` (shorthand, see below). Also accepted, case-insensitive: `rect`, `circle`, `box`, `path`, `label`, `container`, `triangle`. |
| `name` | string | type name (text: its content) | Shown in the layers panel. |
| `x`, `y` | number | auto | Position relative to the parent. Ignored inside auto layout (unless `absolute: true`). Leave them out on top-level nodes to auto-place beside existing content. |
| `width` / `w`, `height` / `h` | number \| `"fill"` \| `"hug"` | 100 | `"fill"` stretches inside an auto-layout parent. `"hug"` (auto-layout frames, text) shrinks to fit the content. |
| `rotation` | number (deg) | 0 | Clockwise, around the center. |
| `opacity` | 0..1 | 1 | |
| `visible` / `hidden` | bool | true / false | |
| `locked` | bool | false | Locked nodes can't be selected on the canvas. |
| `fills` / `fill` / `background` | paint \| paint[] \| hex | type default | See Paints. `"none"` or `[]` removes all fills. |
| `strokes` / `stroke` / `border` | paint \| paint[] \| hex | none | |
| `strokeWeight` / `strokeWidth` / `borderWidth` | number | 1 | |
| `strokeAlign` | `INSIDE` \| `CENTER` \| `OUTSIDE` | `CENTER` | |
| `cornerRadius` / `radius` / `borderRadius` | number \| [tl,tr,br,bl] | 0 | An array sets `cornerRadii` (each corner separately). |
| `effects` / `shadow` | effect[] \| shadow object | none | See Effects. |
| `flipX`, `flipY` | bool | false | |
| `description` | string | "" | Notes for collaborators and agents. |
| `absolute` / `layoutAbsolute` | bool | false | Take the node out of its auto-layout parent's flow. |
| `children` | spec[] | | `FRAME` and `GROUP` only. |

Type defaults:

- `RECTANGLE`, `ELLIPSE`, `POLYGON`, `STAR`, `VECTOR`: light gray fill (`#d9d9d9`)
- `FRAME`: white fill, `clipsContent: true`
- `TEXT`: black fill
- `LINE`: 1px black stroke

## Paints

```json
"#ff0000"                                     // solid shorthand
"#ff000080"                                   // solid with alpha
{ "type": "SOLID", "color": "#6d5dfc", "opacity": 0.8, "visible": true }
{ "type": "LINEAR_GRADIENT", "angle": 90, "stops": [
    { "position": 0, "color": "#6d5dfc" }, { "position": 1, "color": "#e9488d" } ] }
{ "type": "IMAGE", "asset": "<asset id>", "fit": "FILL" }   // FILL | FIT | STRETCH
```

- Gradient `angle`: 0 runs left→right and 90 runs top→bottom. The canvas renders up to 4 stops.
- A list paints bottom to top, so the last paint is on top.

## Effects

```json
"shadow": { "x": 0, "y": 8, "blur": 24, "spread": 0, "color": "#0000001f" }
"effects": [
  { "type": "DROP_SHADOW", "x": 0, "y": 4, "blur": 12, "spread": 0, "color": "#00000040" },
  { "type": "LAYER_BLUR", "radius": 8 }
]
```

`LAYER_BLUR` currently affects exports only.

## FRAME (auto layout)

| Property | Values | Notes |
|---|---|---|
| `layout` / `layoutMode` / `direction` | `VERTICAL` (`column`), `HORIZONTAL` (`row`), `NONE` | Turns on flexbox-like layout. |
| `gap` / `itemSpacing` | number | Space between children. |
| `padding` | number \| [v, h] \| [t, r, b, l] \| {top,right,bottom,left} | |
| `justify` / `primaryAxisAlign` | `start`, `center`, `end`, `space-between` | Along the layout direction. |
| `alignItems` / `counterAxisAlign` | `start`, `center`, `end` | Across the layout direction. |
| `clip` / `clipsContent` | bool | Clip children to the frame (default true). |
| `layoutSizingHorizontal` / `layoutSizingVertical` | `FIXED`, `HUG`, `FILL` | Same as `width`/`height: "hug" \| "fill"`. |

Sizing rules:

- An auto-layout frame with no `width`/`height` hugs its children.
- Setting a number makes that axis `FIXED`.
- A child with `width: "fill"` takes the remaining space along a horizontal parent, or the full inner width in a vertical parent.
- If several children fill, they share the space equally.

## TEXT

| Property | Notes |
|---|---|
| `text` / `characters` / `content` | The string. `\n` makes a hard line break. |
| `size` / `fontSize` | px |
| `weight` / `fontWeight` | 100–900, or `thin`, `light`, `regular`, `medium`, `semibold`, `bold`, `extrabold`, `black` |
| `font` / `fontFamily` | Default `Inter`. Use a family from `list_fonts` (the workspace's fonts), or add one first: `add_font` for any Google Fonts family, `upload_font` for the user's own .ttf/.otf files. Fonts outside the workspace show as Inter for most people and in exports. |
| `color` | Shorthand for a text fill. |
| `align` / `textAlign` | `LEFT`, `CENTER`, `RIGHT` |
| `verticalAlign` / `textAlignVertical` | `TOP`, `CENTER`, `BOTTOM` |
| `lineHeight` | px (default 1.2 × size) |
| `letterSpacing` | px |
| `italic` | bool |
| `textDecoration` | `UNDERLINE`, `STRIKETHROUGH` or `null` |
| `textAutoResize` | `WIDTH_AND_HEIGHT` (default: grows with the text), `HEIGHT` (fixed width, wraps; the default once you give a width), `NONE` (fixed box) |

For paragraphs, give the text a width (a number, or `"fill"` inside auto layout) so it wraps.

## COMPONENT and INSTANCE

- **`COMPONENT`** behaves like a `FRAME`: same props, auto layout, children. It is the source of truth.
- **`INSTANCE`** fields:
  - `component` (alias `componentId`, `of`): the main component's id or name. Required when creating.
  - `text`: shorthand that overrides the component's first text layer.
  - `overrides`: `{ "<layer name or id>": { …props } }`. The key `""` targets the instance itself, though setting props directly on the instance does the same.
  - Any frame prop (`fill`, `radius`, `padding`, `layout`, …) set on the instance overrides the component's value. Unset props follow the component.
  - An instance can't have `children`. Its layers come from the component, and their ids look like `<instanceId>~<componentLayerId>`. Nested instances use `I~N~S`.
- **Editing.**
  - Updating a layer inside an instance stores an override; deleting one hides it.
  - Moving or reparenting layers inside an instance isn't allowed. Change the component instead.

## IMAGE

`{ "type": "image", "asset": "<id>", "fit": "FILL", "radius": 12 }`

- `asset` (alias `src`): an id from `upload_image` or `list_images`. Required.
- `fit`: `FILL` (default, crops to cover), `FIT` (letterbox) or `STRETCH`; `cover` and `contain` work too.
- Width and height default to the image's own size, scaled so the longest side is at most 1200. Give one side and the other keeps the proportions.
- It becomes a `RECTANGLE` named "Image" with an `IMAGE` fill, so every rectangle prop works. To put an image behind content, give a frame an `IMAGE` fill instead.

## LINE

Either give the endpoints, `{ "type": "line", "x1": 0, "y1": 0, "x2": 200, "y2": 0, "stroke": "#000", "strokeWeight": 2 }`,
or give `width` (the length) plus `rotation`.

## VECTOR

`{ "type": "vector", "path": "M0 0 L24 12 L0 24 Z", "fill": "#111" }`

- `path` (alias `d`, `pathData`) is SVG path data in local coordinates. Supported commands: M L H V C S Q T A Z, absolute or relative.
- Width and height default to the path's bounds. Resizing a vector scales its path.

## POLYGON / STAR

`pointCount` (alias `points`, default 5) and, for stars, `innerRadius` (0..1, default 0.382).

## Reading nodes

`get_nodes` returns every non-default property plus:

- `absoluteBounds`: `{x, y, width, height}` in page coordinates
- `children`: an id list, back to front

`get_file_tree` returns one line per node:

```
FRAME "Card" [a1b2:3] @0,0 360×323 fill #ffffff r 16 layout Vertical gap 12 pad 24
```

The fields after the id are position (relative to the parent), size, then key styles.
