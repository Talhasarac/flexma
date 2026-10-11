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
| GET | `/api/files/:id/thumbnail.svg` | – | SVG preview of the first page |
| GET | `/api/files/:id/export.svg?node=<id>&page=<id>` | – | SVG export |
| GET | `/api/files/:id/export.png?node=<id>&page=<id>&scale=2` | – | PNG export |
| POST | `/api/files/:id/assets` | raw image bytes, `Content-Type: image/*` | `{ id, url, width, height }` |
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
| GET | `/api/share/:token/thumbnail.svg` | preview of the first page (cacheable for 60 s) |
| GET | `/api/share/:token/assets/:assetId` | image bytes for the file's workspace |

A dead, expired or unknown token returns 404. The web app serves the viewer at `/s/:token`
and sends signed-in workspace members on to the full editor.

## Version history

| Method | Path | Body | Response |
|---|---|---|---|
| GET | `/api/files/:id/versions` | – | `[{ id, label, version, createdAt, createdBy }]` (`label` is null for autosaves) |
| POST | `/api/files/:id/versions` | `{ label }` | `{ id }`: saves the current state as a named version |
| GET | `/api/files/:id/versions/:vid/preview.svg` | – | SVG preview |
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
