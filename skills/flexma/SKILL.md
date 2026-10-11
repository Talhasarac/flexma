---
name: flexma
description: Design UIs, mockups, wireframes, landing pages, app screens, components, diagrams and graphics in Flexma (a self-hosted, Figma-like collaborative design tool) through the Flexma MCP server. Use whenever the user asks to create, edit, inspect, review or export a design, mentions Flexma or a Flexma file/link, says "the selected thing" while working in Flexma, or wants to turn a description or code into a design (or a design into code), even if they never say "Flexma tools".
---

# Designing in Flexma

Flexma is a collaborative design canvas. You edit the **same live document** people are looking at: every change you make appears on their screens immediately, with your name next to it. Work like a careful senior product designer pairing with them.

The MCP server is usually registered as `flexma`, so tools appear as `mcp__flexma__<tool>` in Claude Code and `flexma.<tool>` in Codex. If no Flexma tools are available, tell the user to connect the server (see "Setup" at the end) instead of guessing.

## Mental model

- **Workspace → project → file → page → nodes.** A file has one or more pages, and each page is a free canvas.
- **Node types:**
  - `COMPONENT`: a reusable main component, a frame-like container. `INSTANCE`: a live copy of a component (see "Components" below).
  - `FRAME`: a container. Frames are artboards and screens, and they also make cards and buttons. They can clip their content and use auto layout.
  - `GROUP`
  - `RECTANGLE`, `ELLIPSE`, `LINE`, `POLYGON`, `STAR`
  - `TEXT`
  - `VECTOR`: SVG path data.
- **Coordinates** are pixels, with y pointing down. `x`/`y` are relative to the parent node. Rotation is in degrees, clockwise, around the node's center.
- **Auto layout** (`layout: "VERTICAL" | "HORIZONTAL"`) works like CSS flexbox. It positions children for you, so don't set `x`/`y` on children of auto-layout frames.
- **Colors** are hex strings: `#RRGGBB` or `#RRGGBBAA`.
- **Ids** look like `aX3k9:4`. Use the ids the tools return. Never invent them.

## Workflow

1. **Find the file.** Use `list_workspaces`, then `list_files` (filter by `workspace_id` or `project_id`). If the user gave a link like `https://host/file/<id>`, the id is the last path segment. Create a new file with `create_file` when asked, and give the user the link it returns.
2. **Look before you touch.** Call `get_file_tree` to see the pages and what is already there. Use `node_id` and `depth` to drill into big files. If the user says "this", "the selected…" or "here", call `get_selection` to find out what they have selected right now.
3. **Build in sections.** Make one `create_nodes` call per meaningful chunk (a screen shell, a nav bar, a hero, a card grid), with nested `children`. That is atomic, fast, and appears to collaborators as one step. Leave out `x`/`y` on top-level nodes to auto-place new work beside existing content, so you never cover someone's work.
4. **Check your work visually.** Call `get_screenshot` on the frame you built and actually look at it:
   - Is the alignment right?
   - Does any text overflow, and are any elements clipped?
   - Is there enough contrast, and is spacing consistent?
   - Does the hierarchy read clearly?

   Fix problems with `update_nodes`, then check again. Do at least one screenshot pass for anything non-trivial.
5. **Checkpoint before big changes.** Before redesigning, deleting or restructuring existing work, call `save_version` with a clear label. People can then roll back from Version history.
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
   - Instance-level styling like `fill` or `stroke` is also an override.
   - Create the component in an earlier call than its instances, since instances look the component up by name or id.
3. **Customize one instance.** `update_nodes` works on an instance's inner layers through their ids from `get_file_tree` (they look like `instanceId~layerId`). These edits become overrides on that instance. Deleting an inner layer hides it.
4. **Change all instances.** Edit the main component's layers. Overrides survive.
5. **Discover existing ones.** Call `list_components` before building UI in an existing file, and reuse what's there. Call `detach_instance` only when a copy truly needs to diverge.

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
- **Fonts:** Inter is always available. For anything else, use a family from `list_fonts` (the workspace's fonts), or add a Google Fonts family with `add_font` *before* using it. A font that isn't in the workspace shows as Inter for most people and in exports, and the tool result warns you. One or two families per design is plenty.
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
| `list_fonts` | See the workspace's fonts (uploaded and Google Fonts) and their weights |
| `add_font` | Add a Google Fonts family to the workspace so everyone sees it |
| `share_file` | Get a public view-only link (no account needed) to hand to someone, or turn it off |
| `list_comments` | Read open feedback threads (who, where, which node, replies) |
| `add_comment` | Pin a note on a node or point, or reply to a thread (`reply_to`) |
| `resolve_comment` | Resolve a thread you addressed (or reopen it) |
| `create_nodes` | Create nested nodes in one step |
| `update_nodes` | Change props: `[{ id, ...props }]` |
| `move_nodes` | Re-parent and/or reorder (keeps the canvas position by default) |
| `duplicate_nodes` | Copy nodes next to the originals |
| `delete_nodes` | Delete nodes and their children |
| `get_screenshot` | Render a node or page to PNG so you can see it |
| `export_svg` | Get SVG markup, e.g. for icons or handoff |

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
- Text boxes are sized by estimate on the server. The first browser that renders them re-measures them exactly, so screenshots can differ by a few pixels from what people see.
- `x`/`y` on children of an auto-layout frame are ignored. To pin a child, set `absolute: true`.
- `move_nodes` is the only way to change a parent; `update_nodes` rejects `parent`/`index`.
- Images: image fills reference uploaded asset ids (`{ "type": "IMAGE", "asset": "<id>" }`). You can't upload binaries through MCP yet. Use a placeholder rectangle with a neutral fill and name it "Image: …".
- Large files: outline with `depth: 2` first, then zoom into the frames you need with `node_id`.

## Setup (if the tools are missing)

Flexma's MCP endpoint is `https://<your-flexma-host>/mcp` (Streamable HTTP, OAuth or bearer token). Unlimited connections are allowed.

- **Claude Code:** run `claude mcp add --scope user --transport http flexma https://<host>/mcp`, then `/mcp` to sign in through the browser.
- **Codex:** run `codex mcp add flexma --url https://<host>/mcp`, then `codex mcp login flexma`. For non-interactive `codex exec`, also set `default_tools_approval_mode = "approve"` under `[mcp_servers.flexma]` in `~/.codex/config.toml`.
- **Headless:** create a token in Flexma → AI Agents and send `Authorization: Bearer <token>`.
- **Skill:** reinstall it with `curl -fsSL https://<host>/agents/install.sh | sh`.
