# Flexma node spec

This reference covers every property accepted by `create_nodes` and `update_nodes`.
Property names are case-sensitive. Shorthands (marked *alias*) are converted
to the canonical names. Unknown names return an error that lists the valid ones.

## Common to all nodes

| Property | Type | Default | Notes |
|---|---|---|---|
| `type` | string | required | `FRAME`, `GROUP`, `RECTANGLE`, `ELLIPSE`, `LINE`, `TEXT`, `VECTOR`, `POLYGON`, `STAR`, `COMPONENT`, `INSTANCE`, plus the shorthands `IMAGE` and `ICON` (see below). Also accepted, case-insensitive: `rect`, `circle`, `box`, `path`, `label`, `container`, `triangle`. |
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
| `fillStyle`, `strokeStyle` | style name or id \| null | none | Link to a shared color style (see `list_styles`); null detaches. |
| `fillRule` | `NONZERO` \| `EVENODD` | `NONZERO` | For vectors with holes. |
| `strokeCap` / `linecap` | `NONE` (butt) \| `ROUND` \| `SQUARE` | flat (round for lines) | Stroke ends. |
| `strokeJoin` / `linejoin` | `MITER` \| `ROUND` \| `BEVEL` | `MITER` | Stroke corners. |
| `flipX`, `flipY` | bool | false | |
| `description` | string | "" | Notes for collaborators and agents. |
| `key` | string | none | A stable id from the source of the design (e.g. a Unity object). Uploading a top-level spec with the same `key` again updates that node in place: see Re-uploads. |
| `absolute` / `layoutAbsolute` | bool | false | Take the node out of its auto-layout parent's flow. |
| `constraints` | `{horizontal, vertical}` \| string | `MIN` / `MIN` | How the node follows when its parent frame or instance is resized, like Unity anchors. See Constraints. |
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
{ "type": "IMAGE", "asset": "<asset id>", "tint": "#ffd166", "slice": 12 }   // tinted 9-slice, see IMAGE
{ "type": "IMAGE", "asset": "<asset id>", "filled": { "method": "RADIAL_360", "amount": 0.4, "origin": "TOP" } }   // cooldown wheel
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

- `blur` is the softness (0 is a hard copy, like Unity's Shadow). `spread` grows the shadow's shape first, or shrinks it when negative. Several shadows stack, the first one lowest.
- `LAYER_BLUR`, and shadows on vectors, show in exports and screenshots only.

## FRAME (auto layout)

| Property | Values | Notes |
|---|---|---|
| `layout` / `layoutMode` / `direction` | `VERTICAL` (`column`), `HORIZONTAL` (`row`), `NONE` | Turns on flexbox-like layout. |
| `gap` / `itemSpacing` | number | Space between children. |
| `padding` | number \| [v, h] \| [t, r, b, l] \| {top,right,bottom,left} | |
| `justify` / `primaryAxisAlign` | `start`, `center`, `end`, `space-between` | Along the layout direction. |
| `alignItems` / `counterAxisAlign` | `start`, `center`, `end` | Across the layout direction. |
| `clip` / `clipsContent` | bool, `"alpha"`, `"shape"` | Clip children to the frame (default true). `"alpha"` clips to the alpha of the frame's fills instead: see Alpha masks. `overflow: "hidden"` / `"visible"` also works. |
| `clipAlpha` | bool | The same as `clip: "alpha"` (turns clipping on). |
| `hideFill` | bool | Don't draw the fills. They still shape an alpha mask. |
| `layoutSizingHorizontal` / `layoutSizingVertical` | `FIXED`, `HUG`, `FILL` | Same as `width`/`height: "hug" \| "fill"`. |

Sizing rules:

- An auto-layout frame with no `width`/`height` hugs its children.
- Setting a number makes that axis `FIXED`.
- A child with `width: "fill"` takes the remaining space along a horizontal parent, or the full inner width in a vertical parent.
- If several children fill, they share the space equally.

## Constraints

`constraints` says how a node follows when its parent frame, component or instance changes size, like the anchors of a Unity RectTransform. Each axis takes one value:

| Value | Also accepted | When the parent grows by Δ |
|---|---|---|
| `MIN` (default) | `LEFT`, `TOP` | Stays put: keeps its left (top) margin. |
| `MAX` | `RIGHT`, `BOTTOM` | Moves by Δ: keeps its right (bottom) margin. |
| `CENTER` | | Moves by Δ/2. |
| `STRETCH` | `LEFT_RIGHT`, `TOP_BOTTOM` | Grows by Δ: keeps both margins. |
| `SCALE` | | Position and size scale with the parent. |

```json
"constraints": { "horizontal": "STRETCH", "vertical": "MAX" }   // a bottom bar
"constraints": "CENTER"                                         // both axes
"constraints": { "anchorMin": [0, 0], "anchorMax": [1, 0] }     // the same bottom bar as Unity anchors
```

- Resizing a frame with `update_nodes` moves its children by their constraints, like dragging its handle in the editor. A child you also update in the same call keeps the values you give it.
- Instances re-flow from their main component: resize an instance and its layers follow their constraints.
- Children placed by auto layout ignore constraints; `absolute` children use them. Children of groups always scale.
- Unity anchors use y-up fractions, which are converted for you: 0 becomes `MIN` (left/bottom → `MAX` vertically), 1 the opposite edge, 0.5 `CENTER`, 0 to 1 `STRETCH`, and anything else `SCALE`.

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
| `stroke`, `strokeWeight`, `strokeAlign` | Text outline. `OUTSIDE` (fill stays intact), `CENTER` (like TextMeshPro) or `INSIDE`. |
| `textStyle` | Link to a shared text style by name or id (sets font, size, weight, line height, letter spacing); null detaches. |
| `textAutoResize` | `WIDTH_AND_HEIGHT` (default: grows with the text), `HEIGHT` (fixed width, wraps; the default once you give a width), `NONE` (fixed box) |
| `wrap` / `textWrap` | Default true. `false`: only `\n` breaks lines, and long lines run past the box width (TextMeshPro's word wrapping off). |
| `overflow` / `textOverflow` | What happens to text that doesn't fit a fixed box: `VISIBLE` (default, draws past it), `CLIP` (cut at the box edges, TMP Masking), `ELLIPSIS` (drops what doesn't fit and ends with "…") or `TRUNCATE` (the same without "…"). |

For paragraphs, give the text a width (a number, or `"fill"` inside auto layout) so it wraps.

For a fixed box, like a TextMeshPro rect, give `width` and `height` plus an `overflow` mode, which makes it `NONE` (fixed) unless you set `textAutoResize` yourself:

```json
{ "type": "text", "text": "A long item name that won't fit", "width": 160, "height": 22, "wrap": false, "overflow": "ellipsis" }
```

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

## ICON

`{ "type": "icon", "icon": "shopping-cart", "size": 24, "color": "#111111", "strokeWidth": 2 }`

- `icon`: a Lucide icon name from `search_icons` (spaces work: "arrow right").
- Becomes a frame of `size`×`size` holding one vector with round caps and joins. Other props (x, y, name, opacity…) apply to the frame.
- `strokeWidth` (default 2) is in Lucide's 24-unit grid, so it scales with `size`: 2 at size 48 draws 4 px.
- Recolor later with `update_nodes {id:"<icon frame>", color, strokeWidth}`.

## IMAGE

`{ "type": "image", "asset": "<id>", "fit": "FILL", "radius": 12 }`

- `asset` (alias `src`): an id from `upload_image` or `list_images`. Required.
- `fit`: `FILL` (default, crops to cover), `FIT` (letterbox) or `STRETCH`; `cover` and `contain` work too.
- Width and height default to the image's own size, scaled so the longest side is at most 1200. Give one side and the other keeps the proportions.
- It becomes a `RECTANGLE` named "Image" with an `IMAGE` fill, so every rectangle prop works. To put an image behind content, give a frame an `IMAGE` fill instead.
- `tint`: a multiply color, like Unity's `Image.color`. `#ffffff` leaves the image unchanged; alpha fades it.
- `slice`: 9-slice scaling, so the corners keep their size while the edges and center stretch (buttons, panels, speech bubbles). Insets are image pixels: a number, `[v, h]`, `[top, right, bottom, left]` or `{top, right, bottom, left, scale, fillCenter}`.
  - `scale` is layer px per image px for the border (default 1, 0.5 for @2x art).
  - `fillCenter: false` keeps only the border.
  - `fit` is ignored while an image is sliced.
- `filled`: show only part of the image, like Unity's Filled image type (progress bars, cooldown wheels): `{ "method": "RADIAL_360", "amount": 0.75, "origin": "TOP", "clockwise": true }`.
  - `method`: `HORIZONTAL`, `VERTICAL`, `RADIAL_90`, `RADIAL_180` or `RADIAL_360`.
  - `amount`: 0 to 1.
  - `origin`: where the fill starts, by name or by Unity's `fillOrigin` number. `HORIZONTAL`: `LEFT`, `RIGHT`. `VERTICAL`: `BOTTOM`, `TOP`. `RADIAL_90` (pivot in that corner): `BOTTOM_LEFT`, `TOP_LEFT`, `TOP_RIGHT`, `BOTTOM_RIGHT`. `RADIAL_180` (pivot mid-edge): `BOTTOM`, `LEFT`, `TOP`, `RIGHT`. `RADIAL_360` (pivot in the center, sweep starts there): `BOTTOM`, `RIGHT`, `TOP`, `LEFT`. Defaults to the first.
  - `clockwise` (default true) sets the radial sweep direction.
- All of these also work on any `IMAGE` paint, e.g. a frame's background.

## Re-uploads (`key`)

Give every node of an export a `key` that stays the same between exports, such as a Unity object path or id. When a top-level spec's `key` matches a node already in the file, `create_nodes` and `upload_nodes` update that node instead of adding a copy:

- Changed properties are updated, and the node ids stay the same, so comments, links and instances keep working.
- Children are matched by `key` within their parent, or by name and type when they have no key. New children are created, and keyed children missing from the spec are deleted.
- Nodes that people added in Flexma (no `key`) are kept, on top of the exported ones.
- A top-level node without `x`/`y` keeps its position. One found on another page or frame moves to the given parent.
- Copies made in Flexma (duplicate, paste) drop their `key`.
- Keys must be unique among siblings. At the top level they should be unique in the file.

The result says how many nodes changed, were created and were deleted. A re-upload is one undoable step.

## Alpha masks

A frame with `clip: "alpha"` shows its children only where its fills aren't transparent, like a Unity `Mask` on an `Image`. The usual mask is an `IMAGE` fill with transparent areas (a star, a rounded sprite, a 9-sliced panel).

```json
{ "type": "frame", "name": "Avatar mask", "width": 96, "height": 96,
  "fill": { "type": "IMAGE", "asset": "<circle sprite>", "fit": "STRETCH" },
  "clip": "alpha", "hideFill": true,
  "children": [ { "type": "image", "asset": "<photo>", "width": 96, "height": 96 } ] }
```

- The cutoff is Unity's: any alpha above 0.001 shows the children fully. Soft edges don't fade the children.
- `hideFill: true` masks without drawing the sprite (Unity's Show Mask Graphic off). Leave it off to draw the sprite under the children.
- `tint`, `slice`, `fit` and fill `opacity` shape the mask as they shape the drawing. Solid and gradient fills mask by their alpha too.
- With no visible fill, an alpha mask hides all of its children.

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

- Style links as ids: `fillStyleId`, `strokeStyleId`, `textStyleId` (match them with `list_styles`)

`get_file_tree` returns one line per node, with style names and non-Inter fonts:

```
FRAME "Card" [a1b2:3] @0,0 360×323 fill #ffffff fillStyle "Surface" r 16 layout Vertical gap 12 pad 24
TEXT "Title" [a1b2:4] @24,24 200×29 font "Exo 2" 24px w700 "Settings" textStyle "Heading/H2"
```

The fields after the id are position (relative to the parent), size, then key styles.
