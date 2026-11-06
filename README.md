<p align="center">
  <img src="web/public/favicon.svg" width="64" height="64" alt="Flexma logo">
</p>

<h1 align="center">Flexma</h1>

<p align="center">
  <b>A self-hosted design canvas for teams and their AI agents.</b><br>
  Real-time multiplayer like Figma. Runs on your own server. Claude Code, Codex and OpenCode edit designs live through MCP.
</p>

<p align="center">
  <a href="#quick-start">Quick start</a> ·
  <a href="#connect-ai-agents">AI agents</a> ·
  <a href="#unity">Unity</a> ·
  <a href="#development">Development</a> ·
  <a href="docs/api.md">API</a>
</p>

<p align="center">
  <img alt="License: AGPL-3.0-or-later" src="https://img.shields.io/badge/license-AGPL--3.0--or--later-blue">
  <img alt="Rust" src="https://img.shields.io/badge/server%20%26%20engine-Rust-orange">
  <img alt="WebGPU" src="https://img.shields.io/badge/renderer-WebGPU%20%2F%20WebGL2-6f42c1">
  <img alt="MCP" src="https://img.shields.io/badge/agents-MCP-black">
</p>

---

## What is Flexma?

Flexma is a collaborative design tool you host yourself. It's a single Rust binary with an SQLite database, so you can run it with one `docker compose up`.

People and AI agents work on the same live document. When an agent edits a file through MCP, everyone with the file open sees the changes appear, labelled with the agent's name. Agents can also read and resolve comments, take screenshots to check their work, and undo their own changes.

**Good fit if you want:**

- a Figma-style editor on your own hardware, with your data in one SQLite file
- AI coding agents that design screens, fix review comments and export assets for you
- a design-to-game pipeline: Flexma frames to Unity uGUI prefabs, and Unity screens back into Flexma

## Features

### Editor

- **Infinite canvas** with pages, frames, groups, rectangles, ellipses, lines, polygons, stars, pen-drawn vectors, text and images
- **GPU rendering:** the scene renderer is Rust compiled to WebAssembly, drawing with WebGPU and falling back to WebGL2
- **Auto layout:** flexbox-style frames with hug, fill and fixed sizing, gap, padding and alignment
- **Styling:** solid, gradient and image fills, strokes, corner radii, drop shadows, opacity, rotation, flips
- **Constraints** (like Unity anchors): pin, center, stretch or scale layers as their parent resizes
- **Game UI images:** 9-slice scaling and multiply tint
- **Components and instances:** overrides, nested instances, an Assets panel, detach, "go to main component"
- **Shared color and text styles** that update every layer using them
- **Fonts:** upload `.ttf`/`.otf` or add any of ~1,950 Google Fonts families, served by your own server
- **Painting:** GPU brushes (pencil, chalk, charcoal, bristle, palette knife, spray and more), smudge and color mixing, pen pressure, stabilizer. Several people can paint the same layer at once.
- **Version history** with automatic snapshots, named versions and one-click restore
- Snapping and smart guides, Figma-style keyboard shortcuts, undo/redo, copy/paste
- **Export** to PNG and SVG

### Collaboration

- Live cursors, selections and presence
- Workspaces with owner, editor and viewer roles, invite links, projects and files
- Comments pinned to the canvas or to layers, with threads and resolve
- Public view-only share links that can expire, be reset or be turned off

### AI agents

- Built-in **MCP server** for Claude Code, Codex, OpenCode or any MCP client
- OAuth browser sign-in for desktop agents, API tokens for headless ones
- Agents can upload images and fonts, place any of 1,900 built-in icons, import SVG as editable vectors, fill components with data, audit a design for contrast and consistency, and manage shared styles
- An installable **Flexma skill** teaches agents the tool and good design practice. Your server tells agents when their copy is out of date.

### Self-hosting

- One Rust binary and SQLite (WAL)
- Docker Compose with automatic HTTPS through Caddy
- Full backups from Admin → Backups, restored with `flexma restore`
- Invite-only by default

## Quick start

You need a server with Docker and a domain whose DNS A record points at it.

```bash
git clone https://github.com/Talhasarac/flexma.git
cd flexma
cp .env.example .env          # set FLEXMA_DOMAIN=design.example.com
docker compose up -d --build
```

Caddy gets a TLS certificate automatically.

On first start the server prints a one-time setup link, so nobody else can claim a fresh instance:

```bash
docker compose logs flexma | grep setup
```

Open that link (`https://<your domain>/setup?token=…`) and create the admin account. The token is also saved in the data volume as `setup-token`.

For a step-by-step guide on a fresh VPS, see [Deploy on DigitalOcean](docs/deploy-digitalocean.md).

## Connect AI agents

Add Flexma as an MCP server:

```bash
# Claude Code (then run /mcp → flexma → Authenticate)
claude mcp add --scope user --transport http flexma https://<your domain>/mcp

# Codex
codex mcp add flexma --url https://<your domain>/mcp && codex mcp login flexma

# OpenCode
opencode mcp add flexma --url https://<your domain>/mcp && opencode mcp auth flexma
```

Install the Flexma skill, which teaches agents how to design in Flexma. It installs into Claude Code, Codex and OpenCode:

```bash
# macOS / Linux
curl -fsSL https://<your domain>/agents/install.sh | sh
```

```powershell
# Windows PowerShell
irm https://<your domain>/agents/install.ps1 | iex
```

The **AI Agents** page in the app shows these commands with your domain filled in.

Then ask your agent things like:

> In my Flexma file "Landing page", design a pricing section with three plans using auto layout, then screenshot it and fix anything that looks off.

> Address all open comments in "Landing page" and resolve them.

### MCP tools

| Category | Tools |
|---|---|
| Browse | `list_workspaces`, `list_files`, `create_project`, `create_file`, `create_page`, `share_file` |
| Read | `get_file_tree`, `get_nodes`, `find_nodes`, `get_selection` |
| Edit | `create_nodes`, `update_nodes`, `move_nodes`, `duplicate_nodes`, `delete_nodes`, `fill_with_data` |
| Components | `list_components`, `create_component`, `detach_instance` |
| Styles | `list_styles`, `create_styles`, `update_styles`, `delete_styles` |
| Assets | `upload_image`, `list_images`, `import_svg`, `search_icons`, `list_fonts`, `add_font`, `upload_font`, `upload_nodes` |
| Look and export | `get_screenshot`, `export_png`, `export_svg`, `export_unity` |
| Review | `audit_design`, `list_comments`, `add_comment`, `resolve_comment` |
| Safety | `save_version`, `undo_agent_changes` |

Agent edits go through the same pipeline as human edits, so they're synced, undoable and visible live.

## Unity

Two optional skills connect Flexma with Unity uGUI projects:

| Skill | Direction | What it does |
|---|---|---|
| `flexma-to-unity` | Flexma → Unity | Builds Flexma frames into uGUI prefabs from Flexma's own renders, so the game matches the design. Re-imports only what changed. |
| `unity-ui-to-flexma` | Unity → Flexma | Exports a game's existing screens (uGUI with TextMeshPro, or UI Toolkit) into Flexma as editable layers, one page per scene, and keeps them in sync. |

Install them the same way as the main skill:

```bash
curl -fsSL https://<your domain>/agents/install/flexma-to-unity.sh | sh
curl -fsSL https://<your domain>/agents/install/unity-ui-to-flexma.sh | sh
```

On Windows, use `irm https://<your domain>/agents/install/<skill>.ps1 | iex`.

## Configuration

Set these as environment variables (in Docker, through `docker-compose.yml` / `.env`).

| Variable | Default | Description |
|---|---|---|
| `FLEXMA_DOMAIN` | – | Docker Compose only: your public domain, used by Caddy and to build `FLEXMA_PUBLIC_URL` |
| `FLEXMA_PUBLIC_URL` | from the request | Public base URL, used for invite links, OAuth and MCP setup |
| `FLEXMA_ADDR` | `0.0.0.0:8080` | Listen address |
| `FLEXMA_DATA_DIR` | `./data` | SQLite database and uploads |
| `FLEXMA_WEB_DIR` | `./web/dist` | Built web app |
| `FLEXMA_ALLOW_SIGNUP` | `false` | Open sign-up. Otherwise invite-only after the first admin |
| `FLEXMA_INSTANCE_NAME` | `Flexma` | Name shown on the sign-in page |
| `FLEXMA_SETUP_TOKEN` | generated | Token needed to create the first admin account |
| `FLEXMA_MAX_UPLOAD_MB` | `25` | Upload size limit |
| `FLEXMA_MAX_EXPORT_PX` | `8192` | Longest side of a PNG export. Lower it on small servers. |
| `FLEXMA_SECURE_COOKIES` | `true` when the public URL is HTTPS | Mark the session cookie `Secure` |
| `FLEXMA_FONTS_DIR` | – | Extra fonts for server-side PNG rendering |

### Admin commands

```bash
flexma reset-password EMAIL    # set a new password (prompts on stdin)
flexma restore FILE.zip        # restore a backup from the Admin page (stop the server first)
```

## Architecture

```
┌──────────────────────────── Browser ────────────────────────────┐
│  React UI (panels, dashboard)  ◄── versioned snapshots ──┐      │
│                                                          │      │
│  flexma-engine (Rust → wasm): document replica, scene,   │      │
│  hit testing, tools, undo, sync, WebGPU/WebGL2 renderer ─┘      │
└───────────────▲────────────────────────────────────▲────────────┘
                │ WebSocket (ops, presence)          │ HTTP (REST)
┌───────────────┴────────────────────────────────────┴────────────┐
│ flexma-server (Rust, axum): auth, workspaces, files, assets,    │
│ one actor per open document (authoritative, ordered ops)        │
│ MCP endpoint + OAuth 2.1  ◄── Claude Code / Codex / OpenCode    │
│ SQLite (WAL)                                                    │
└─────────────────────────────────────────────────────────────────┘
        flexma-doc (shared crate): nodes, ops, fractional indexing,
        auto layout, components, styles, SVG export
```

- **Multiplayer:** the server is authoritative. Last writer wins per property, and child order uses fractional indexing. Clients apply edits optimistically and ignore stale echoes.
- **Rendering:** a camera-independent display list of instanced SDF shapes with analytic anti-aliasing. Paths are tessellated with lyon, frames clip with the stencil buffer, and the canvas redraws only when something changes.
- **React bridge:** React reads JSON snapshots only when engine version counters change. Pointer input goes straight to wasm.
- **Network:** ops are coalesced per property and sent about every 33 ms. A drag is one undo step.
- **Painting:** a stroke is stored as points plus a brush, not pixels. Every client stamps the same dabs into 256 px GPU tiles in server order, so all copies match.

More background is in [`docs/research/performance.md`](docs/research/performance.md).

## Development

Requirements:

- Rust (stable) with `rustup target add wasm32-unknown-unknown`
- `cargo install wasm-bindgen-cli --version 0.2.129` (must match the pinned crate version)
- Node 20+

```bash
./scripts/build-wasm.sh                  # build the engine into web/src/engine/pkg
cargo run -p flexma-server               # API on :8080, data in ./data
cd web && npm install && npm run dev     # UI on :5173, proxies /api, /mcp and /oauth
```

Tests and type checks:

```bash
cargo test
cd web && npx tsc -b
```

### Repository layout

| Path | What |
|---|---|
| `crates/flexma-doc` | Document model, ops, layout, components, styles, SVG export. Shared by the server and the engine. |
| `crates/flexma-engine` | The wasm editor engine and renderer |
| `crates/flexma-server` | HTTP/WebSocket server, MCP, OAuth, SQLite, Unity export |
| `web/` | React app (dashboard and editor UI) |
| `skills/` | Agent skills: `flexma`, `flexma-to-unity`, `unity-ui-to-flexma` |
| `docs/` | API reference, deployment guide, performance research |
| `deploy/` | Caddyfile and systemd unit |

Contributors (human or agent) should read [`CLAUDE.md`](CLAUDE.md) first: it lists the invariants the sync, layout and storage code relies on.

## Contributing

Issues and pull requests are welcome. For larger changes, open an issue first so we can agree on the approach. Please run `cargo test` and `npx tsc -b` before sending a PR.

## License

[AGPL-3.0-or-later](LICENSE), the license Nextcloud and Penpot also use. If you run a modified Flexma for other people over a network, you must offer them its source.

Bundled third-party assets keep their own licenses, for example [Lucide icons](crates/flexma-server/assets/icons/LUCIDE-LICENSE) and [Twemoji](crates/flexma-server/assets/fonts/twemoji/LICENSE.md).

---

<p align="center">
  <a href="https://sillytoast.studio"><img src="docs/sillytoast.svg" width="28" height="28" alt="Silly Toast Studio"></a><br>
  <sub>Made by <a href="https://sillytoast.studio">Silly Toast Studio</a>. Serious about silly.</sub>
</p>
