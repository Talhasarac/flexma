<p align="center">
  <img src="web/public/favicon.svg" width="56" height="56" alt="Flexma">
</p>

<h1 align="center">Flexma</h1>

<p align="center"><b>The open-source, self-hosted design canvas for teams and their AI agents.</b><br>
Real-time multiplayer like Figma. Your own server, like Nextcloud. Claude Code and Codex built in through MCP.</p>

<p align="center">
  <a href="https://sillytoast.studio"><img src="docs/sillytoast.svg" width="18" height="18" alt=""> Made by <b>silly toast</b> studio</a>
</p>

---

## Features

**The editor**
- Infinite canvas: pages, frames, groups, rectangles, ellipses, lines, polygons, stars, pen-drawn vectors, text and images.
- Smooth GPU rendering. The scene renderer is written in Rust, compiled to WebAssembly, and drawn with WebGL2.
- Auto layout (flexbox-style frames) with hug, fill and fixed sizing, gap, padding and alignment.
- Fills (solid, linear gradient, image), strokes, corner radii, drop shadows, opacity, rotation and flips.
- Snapping and smart guides; resizing, rotating and multi-select; alt-drag to duplicate; drag layers into frames.
- Undo and redo, copy and paste, keyboard shortcuts that match Figma's, and a layers panel with drag-to-reorder.
- Version history: automatic snapshots and named versions, with one-click restore.
- Comments pinned to the canvas or to layers, with threads, replies and resolve. Agents can read and resolve them too.
- Components and instances, with overrides, nested instances, an Assets panel, detach, and "go to main component". Edits to a main component update every instance live, for people and agents alike.
- Export to PNG and SVG.

**Collaboration**
- Real-time multiplayer with live cursors, selections and presence.
- Shared workspaces with owner, editor and viewer roles, invite links, projects and files.
- Public view-only share links: anyone with the link can watch a file live and export it, without an account. Links can expire, be reset or be turned off.

**AI agents**
- A built-in MCP server lets Claude Code, Codex or any MCP client list, read, create, edit and screenshot designs live.
- Unlimited connections.
- OAuth browser sign-in, like Figma's hosted MCP, plus API tokens for headless agents.
- The Flexma skill teaches agents the platform and good design practice. It installs into Claude Code and Codex with one command.

**Self-hosting**
- A single Rust binary with an SQLite database.
- `docker compose up` with automatic HTTPS.
- See [Deploy on DigitalOcean](docs/deploy-digitalocean.md).

## Quick start (Docker)

```bash
cp .env.example .env              # set FLEXMA_DOMAIN
docker compose up -d --build
```

Open `https://<your domain>` and create the admin account.

## Connect your AI agents

```bash
# Claude Code: then run /mcp → flexma → Authenticate (opens your browser)
claude mcp add --scope user --transport http flexma https://<your domain>/mcp

# Codex
codex mcp add flexma --url https://<your domain>/mcp && codex mcp login flexma

# Teach them Flexma (installs ~/.claude/skills/flexma and ~/.codex/skills/flexma)
curl -fsSL https://<your domain>/agents/install.sh | sh
```

Then ask, for example: *"In my Flexma file 'Landing page', design a pricing section with three plans using auto layout, then screenshot it and fix anything that looks off."* Or: *"Address all open comments in 'Landing page' and resolve them."*

The MCP tools are:

| Category | Tools |
|---|---|
| Browse | `list_workspaces`, `list_files`, `create_project`, `create_file`, `create_page` |
| Read | `get_file_tree`, `get_nodes`, `find_nodes`, `get_selection` |
| Edit | `create_nodes`, `update_nodes`, `move_nodes`, `duplicate_nodes`, `delete_nodes` |
| Look | `get_screenshot`, `export_svg` |
| Design system | `list_components`, `create_component`, `detach_instance` (plus `{type:"instance"}` in `create_nodes`) |
| Feedback | `list_comments`, `add_comment`, `resolve_comment` |
| Safety | `save_version` (checkpoint before big edits; people restore from Version history) |

Agents edit through the same pipeline as people, so collaborators watch the changes appear live, labelled with the agent's name.

## Architecture

```
┌───────────────────────────── Browser ─────────────────────────────┐
│  React UI (panels, dashboard)  ◄─ versioned snapshots ─┐          │
│                                                         │          │
│  flexma-engine (Rust → wasm): document replica, scene, │          │
│  hit testing, tools, undo, LWW sync, WebGL2 renderer ───┘          │
└───────────────▲──────────────────────────────────────▲────────────┘
                │ WebSocket (ops, presence)            │ HTTP (REST)
┌───────────────┴──────────────────────────────────────┴────────────┐
│ flexma-server (Rust, axum): auth, workspaces, files, assets,       │
│ one actor per open document (authoritative, ordered ops, snapshots)│
│ MCP endpoint + OAuth 2.1 ◄──── Claude Code / Codex / MCP clients   │
│ SQLite (WAL)                                                      │
└────────────────────────────────────────────────────────────────────┘
          flexma-doc (shared Rust crate): nodes, ops, fractional
          indexing, auto layout, SVG export, agent-friendly specs
```

Design choices follow Figma's published architecture and the research in [`docs/research/performance.md`](docs/research/performance.md):

- **Multiplayer:** server-authoritative, last-writer-wins per property, fractional indexing for child order. Clients apply edits optimistically and ignore stale echoes.
- **Rendering:** a camera-independent display list of instanced SDF shapes (rounded rects, ellipses, shadows) with analytic anti-aliasing. Path meshes are tessellated by lyon. Frames clip with the stencil buffer. Text is rasterized per node at power-of-two zoom buckets.
- **Frames:** the canvas renders only when something changes. Pan and zoom only change a uniform.
- **Bridge to React:** React reads JSON snapshots only when engine version counters change, through `useSyncExternalStore`. Pointer input goes straight to wasm.
- **Network:** ops are coalesced per property and sent about every 33 ms. A drag is a single undo step.

## Development

Requirements: Rust (stable), `rustup target add wasm32-unknown-unknown`, `cargo install wasm-bindgen-cli --version 0.2.129`, and Node 20+.

```bash
./scripts/build-wasm.sh                 # engine → web/src/engine/pkg
cargo run -p flexma-server              # API on :8080 (data in ./data)
cd web && npm install && npm run dev    # UI on :5173 (proxies /api, /mcp, /oauth)
```

Run the tests:

```bash
cargo test
cd web && npx tsc -b
```

| Path | What |
|---|---|
| `crates/flexma-doc` | Document model, ops, layout, SVG export (shared by the server and the engine) |
| `crates/flexma-engine` | wasm editor engine and WebGL2 renderer |
| `crates/flexma-server` | HTTP/WebSocket server, MCP, OAuth, SQLite |
| `web/` | React app (dashboard and editor UI) |
| `skills/flexma` | Agent skill for Claude Code and Codex |
| `docs/` | API reference, deployment guide, performance research |

## Configuration

| Variable | Default | Description |
|---|---|---|
| `FLEXMA_ADDR` | `0.0.0.0:8080` | Listen address |
| `FLEXMA_DATA_DIR` | `./data` | SQLite database and uploads |
| `FLEXMA_WEB_DIR` | `./web/dist` | Built web app |
| `FLEXMA_PUBLIC_URL` | from request | Public base URL, used for invite links, OAuth and MCP setup |
| `FLEXMA_ALLOW_SIGNUP` | `false` | Open sign-up. Otherwise invite-only after the first admin |
| `FLEXMA_INSTANCE_NAME` | `Flexma` | Shown on the sign-in page |
| `FLEXMA_MAX_UPLOAD_MB` | `25` | Image upload limit |
| `FLEXMA_FONTS_DIR` | – | Extra fonts for server-side PNG rendering |

## License

AGPL-3.0-or-later, the same license Nextcloud and Penpot use for self-hosted collaboration software.

---

<p align="center"><a href="https://sillytoast.studio"><img src="docs/sillytoast.svg" width="28" height="28" alt="Silly Toast Studio"></a><br><sub>Serious about silly.</sub></p>
