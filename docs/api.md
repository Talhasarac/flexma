# Flexma HTTP API

All endpoints are JSON over HTTP under `/api`. Browser sessions authenticate with
the `flexma_session` cookie (HttpOnly, SameSite=Lax) set by login/signup.
API tokens (`Authorization: Bearer fxm_...`) are accepted everywhere a session is,
and are what AI agents use for MCP (`/mcp`).

Errors: non-2xx status with body `{ "error": "human readable message" }`.
Timestamps are RFC 3339 strings. Ids are opaque strings.

## Setup & auth

| Method | Path | Body | Response |
|---|---|---|---|
| GET | `/api/setup` | – | `{ needsSetup: bool, allowSignup: bool, version: string, instanceName: string }` |
| POST | `/api/auth/signup` | `{ email, name, password, inviteToken? }` | `{ user }` (sets cookie). First user becomes admin. After setup, signup requires `allowSignup` or a valid `inviteToken`. Password ≥ 8 chars. |
| POST | `/api/auth/login` | `{ email, password }` | `{ user }` (sets cookie) |
| POST | `/api/auth/logout` | – | `{}` |
| GET | `/api/me` | – | `{ user }` or 401 |
| PATCH | `/api/me` | `{ name?, password?, currentPassword? }` | `{ user }` |

`user = { id, email, name, isAdmin, color }` — `color` is a hex color used for avatars and multiplayer cursors.

## Workspaces (shared team spaces)

Roles: `owner` > `editor` > `viewer`. Viewers can open files read-only.

| Method | Path | Body | Response |
|---|---|---|---|
| GET | `/api/workspaces` | – | `[{ id, name, role, memberCount, createdAt }]` |
| POST | `/api/workspaces` | `{ name }` | workspace (caller becomes owner) |
| GET | `/api/workspaces/:id` | – | workspace |
| PATCH | `/api/workspaces/:id` | `{ name }` | workspace (owner) |
| DELETE | `/api/workspaces/:id` | – | `{}` (owner) |
| GET | `/api/workspaces/:id/members` | – | `[{ userId, email, name, color, role }]` |
| PATCH | `/api/workspaces/:id/members/:userId` | `{ role }` | `{}` (owner) |
| DELETE | `/api/workspaces/:id/members/:userId` | – | `{}` (owner, or self to leave) |
| GET | `/api/workspaces/:id/invites` | – | `[{ id, email, role, token, url, createdAt, expiresAt }]` (owner) |
| POST | `/api/workspaces/:id/invites` | `{ email?, role }` | invite (owner). `url` is the full join link. |
| DELETE | `/api/invites/:id` | – | `{}` |
| GET | `/api/invites/by-token/:token` | – | `{ workspaceName, role, invitedBy }` (public) |
| POST | `/api/invites/by-token/:token/accept` | – | `{ workspaceId }` |

## Projects & files

| Method | Path | Body | Response |
|---|---|---|---|
| GET | `/api/workspaces/:id/projects` | – | `[{ id, workspaceId, name, fileCount, createdAt }]` |
| POST | `/api/workspaces/:id/projects` | `{ name }` | project |
| PATCH | `/api/projects/:id` | `{ name }` | project |
| DELETE | `/api/projects/:id` | – | `{}` (deletes its files) |
| GET | `/api/projects/:id/files` | – | `[file]` |
| GET | `/api/workspaces/:id/files` | – | `[file]` most recently updated first (max 50) |
| POST | `/api/projects/:id/files` | `{ name }` | file |
| GET | `/api/files/:id` | – | file |
| PATCH | `/api/files/:id` | `{ name?, projectId? }` | file |
| DELETE | `/api/files/:id` | – | `{}` |
| POST | `/api/files/:id/duplicate` | – | file |
| GET | `/api/files/:id/thumbnail.png` | – | PNG preview of the first page, rendered with the workspace's fonts and cached per file version |
| GET | `/api/files/:id/export.svg?node=<id>&page=<id>` | – | SVG export |
| GET | `/api/files/:id/export.png?node=<id>&page=<id>&scale=2` | – | PNG export |
| POST | `/api/files/:id/assets` | raw image bytes, `Content-Type: image/*` | `{ id, url, width, height, mime }` |
| POST | `/api/image-uploads/:token` | raw image bytes, no sign-in | asset + `remainingUploads` + a ready-made `node` (`reused: true` when the workspace already had the same picture; its asset id comes back): upload through a link from the MCP `upload_image` tool (same rules as font upload links) |
| POST | `/api/node-uploads/:token` | JSON array of node specs (or `{nodes:[…]}`), no sign-in | `{ created, updated, deleted, matched, remainingUploads, nodes: [{id, name, type}], result }` (`nodes` = the resulting top-level nodes, in order; `matched` = top-level specs whose `key` updated an existing node): creates a node tree like `create_nodes`, or updates the nodes an earlier upload made from specs with the same `key`, as the agent that asked for the link via MCP `upload_nodes` (up to 50,000 nodes per batch) |
| GET | `/api/assets/:id` | – | image bytes |
| GET (WS) | `/api/files/:id/ws` | – | multiplayer socket, see below |

`file = { id, name, projectId, projectName, workspaceId, role, createdAt, updatedAt, createdBy }`

## Share links (public, view-only)

One link per file. Anyone with it can watch the file live, inspect layers and
export, without an account. Guests can't edit, comment or see comments.

| Method | Path | Body | Response |
|---|---|---|---|
| GET | `/api/files/:id/share` | – | `share` (any member) |
| PUT | `/api/files/:id/share` | `{ enabled, expiresInDays?, regenerate? }` | `share` (editors and owners) |

`share = { enabled: false } | { enabled: true, token, url, createdBy, createdAt, expiresAt }`

- `expiresInDays`: `0` means never. Leave it out to keep the current expiry; a new link never expires.
- `regenerate: true` issues a new token and keeps the expiry. The old URL stops working at once.
- Turning a link off or regenerating it disconnects everyone viewing through the old token.

Public endpoints (no sign-in; `:token` is the 32-character link token):

| Method | Path | Response |
|---|---|---|
| GET | `/api/share/:token` | `{ fileId, name, workspaceName, sharedBy, updatedAt, expiresAt }` |
| GET (WS) | `/api/share/:token/ws` | the multiplayer socket, joined as a read-only `Guest` peer (`isGuest: true`) |
| GET | `/api/share/:token/export.svg`, `/export.png` | same query parameters as the file exports |
| GET | `/api/share/:token/thumbnail.png` | preview of the first page (cacheable for 60 s) |
| GET | `/api/share/:token/fonts`, `/fonts/:fontId` | the workspace's fonts and font files, so guests see the right fonts |
| GET | `/api/share/:token/assets/:assetId` | image bytes for the file's workspace |

A dead, expired or unknown token returns 404. The web app serves the viewer at `/s/:token`
and sends signed-in workspace members on to the full editor.

## Fonts

Fonts belong to a workspace: everyone in it gets them in every file, and so do
share-link guests, PNG exports, thumbnails and agent screenshots. Google Fonts
are downloaded once by the server and served from it, so browsers never
contact Google.

| Method | Path | Body | Response |
|---|---|---|---|
| GET | `/api/workspaces/:id/fonts` | – | `[font]` (members) |
| POST | `/api/workspaces/:id/fonts` | raw `.ttf`/`.otf` bytes | `font`; replaces a face with the same family, weight and style (editors) |
| POST | `/api/workspaces/:id/fonts/google` | `{ family }` | `{ family, styles }`: every style of a Google Fonts family (editors) |
| DELETE | `/api/workspaces/:id/fonts?family=…` | – | `{}`: removes all styles of a family (editors) |
| GET | `/api/fonts/:fontId` | – | font file (members of its workspace) |
| POST | `/api/font-uploads/:token` | raw `.ttf`/`.otf` bytes, no sign-in | `font` + `remainingUploads`: upload through a link from the MCP `upload_font` tool (15 minutes, a set number of files) |
| GET | `/api/google-fonts` | – | `[{ family, category, styles, popularity }]`, most popular first, cached by the server for a week |
| GET | `/api/google-fonts/preview?family=…` | – | a tiny font with just the family name's glyphs, for pickers |

`font = { id, family, weight, weightMax, italic, source: "upload" | "google", format, size, createdBy, createdAt }`.
`weightMax` differs from `weight` only for variable fonts. WOFF/WOFF2 and `.ttc`
collections are rejected because the server renderer can't read them.

Agents can't send font bytes through MCP, so `upload_font` returns an upload link
instead: it's tied to the workspace and the person who authorized the agent, works
for the number of files asked for (at most 100) within 15 minutes, and is kept in
memory only. A file that isn't a valid font doesn't use up an upload.

Open editors get `{ t: "fonts" }` over the socket when a workspace's fonts change.

## Version history

| Method | Path | Body | Response |
|---|---|---|---|
| GET | `/api/files/:id/versions` | – | `[{ id, label, version, createdAt, createdBy }]` (`label` is null for autosaves) |
| POST | `/api/files/:id/versions` | `{ label }` | `{ id }`: saves the current state as a named version |
| GET | `/api/files/:id/versions/:vid/preview.png` | – | PNG preview |
| POST | `/api/files/:id/versions/:vid/restore` | – | `{ restored }`: the current state is saved first, and open editors reload |

Autosaves are taken at most every 15 minutes of editing; the newest 200 are kept.

## Comments

| Method | Path | Body | Response |
|---|---|---|---|
| GET | `/api/files/:id/comments` | – | `[thread]` where `thread = comment + { replies: [comment] }` |
| POST | `/api/files/:id/comments` | `{ body, parentId?, pageId?, nodeId?, x?, y? }` | `{ id }` |
| PATCH | `/api/comments/:id` | `{ body?, resolved? }` | `{}` |
| DELETE | `/api/comments/:id` | – | `{}` |

`comment = { id, parentId, pageId, nodeId, x, y, author: { id, name, color, isAgent }, body, resolved, createdAt, updatedAt }`.

- When `nodeId` is set, `x`/`y` are relative to that node's top-left corner. Otherwise they are page coordinates.
- Open editors are notified with `{ t: "comments" }` over the socket.

## Admin backups

| Method | Path | Response |
|---|---|---|
| GET | `/api/admin/backup/info` | `{ databaseBytes, assetsFiles, assetsBytes, fontsFiles, fontsBytes }` |
| GET | `/api/admin/backup` | a .zip download: `flexma.db` (a consistent `VACUUM INTO` snapshot), `assets/`, `fonts/`, `README.txt` |

Admins only, and only with a browser session: API and agent tokens get 403. Restore with
`flexma restore <file.zip>` while the server is stopped.

## API tokens & AI agents

| Method | Path | Body | Response |
|---|---|---|---|
| GET | `/api/tokens` | – | `[{ id, name, prefix, createdAt, lastUsedAt }]` |
| POST | `/api/tokens` | `{ name }` | `{ id, name, prefix, token }` — `token` is shown only once |
| DELETE | `/api/tokens/:id` | – | `{}` |
| GET | `/api/agent-setup` | – | `{ mcpUrl, skillInstallCommand, claudeCommand, codexConfig, codexCommand }` with `$FLEXMA_TOKEN` placeholders |

There is no limit on the number of tokens or concurrent MCP connections.

## Multiplayer socket (`/api/files/:id/ws`)

JSON text frames.

Server → client:
- `{ t: "init", clientId, version, role, doc: DocSnapshot, peers: [peer] }`
- `{ t: "ops", from: clientId, batch: n, ops: [op] }` — `from` equal to your clientId means it is the ack of your batch `n`
- `{ t: "reject", batch: n, error }` (followed by corrective `ops` from `"server"`)
- `{ t: "presence", peer }`, `{ t: "leave", clientId }`
- `{ t: "error", error, fatal? }` (`fatal`: stop reconnecting, e.g. the share link was turned off)

Client → server:
- `{ t: "ops", batch: n, ops: [op] }`
- `{ t: "presence", cursor: [x, y] | null, selection: [ids], page: pageId }`

`peer = { clientId, userId, name, color, cursor, selection, page, isAgent, isGuest }`
`op = { op: "create", node } | { op: "update", id, props } | { op: "delete", id }`

## MCP

`POST /mcp` — Model Context Protocol, Streamable HTTP transport (JSON responses),
authenticated with `Authorization: Bearer <api token>`. See `skills/flexma/SKILL.md`
for the tool catalogue.
