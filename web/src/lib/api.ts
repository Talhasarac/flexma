/**
 * Typed client for the Flexma HTTP API (see docs/api.md).
 *
 * Every function returns the decoded JSON body (unwrapping `{ user }` envelopes)
 * and throws an `ApiError` (with `status` and the server's `error` message) on
 * any non-2xx response or network failure (status 0).
 *
 * Usage:
 *   import { api, type FileInfo } from '../lib/api';
 *   const file = await api.files.get(id);
 */

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

export type Role = 'owner' | 'editor' | 'viewer';

export interface User {
  id: string;
  email: string;
  name: string;
  isAdmin: boolean;
  /** Hex color used for avatars and multiplayer cursors. */
  color: string;
}

export interface SetupInfo {
  needsSetup: boolean;
  allowSignup: boolean;
  version: string;
  instanceName: string;
}

export interface Workspace {
  id: string;
  name: string;
  role: Role;
  memberCount: number;
  createdAt: string;
}

export interface Member {
  userId: string;
  email: string;
  name: string;
  color: string;
  role: Role;
}

export interface Invite {
  id: string;
  email: string | null;
  role: Role;
  token: string;
  /** Full join link. */
  url: string;
  createdAt: string;
  expiresAt: string | null;
}

export interface InviteInfo {
  workspaceName: string;
  role: Role;
  invitedBy: string;
}

export interface Project {
  id: string;
  workspaceId: string;
  name: string;
  fileCount: number;
  createdAt: string;
}

export interface FileInfo {
  id: string;
  name: string;
  projectId: string;
  projectName: string;
  workspaceId: string;
  /** The current user's role on this file. Viewers open files read-only. */
  role: Role;
  createdAt: string;
  updatedAt: string;
  createdBy: string;
}

/** A font file in a workspace (one face: family + weight + style). */
export interface WorkspaceFont {
  id: string;
  family: string;
  weight: number;
  /** Equal to `weight` unless it's a variable font with a weight range. */
  weightMax: number;
  italic: boolean;
  source: 'upload' | 'google';
  format: 'ttf' | 'otf';
  size: number;
  createdBy: string | null;
  createdAt: string;
}

/** One family in the Google Fonts catalogue (served by Flexma, most popular first). */
export interface GoogleFontFamily {
  family: string;
  category: string;
  /** e.g. "400", "700", "400i". */
  styles: string[];
  popularity: number;
}

/** A file's public view-only link (`GET/PUT /api/files/:id/share`). */
export interface ShareLink {
  enabled: boolean;
  token?: string;
  url?: string;
  createdBy?: string | null;
  createdAt?: string;
  expiresAt?: string | null;
}

export interface ShareUpdate {
  enabled: boolean;
  /** Days until the link stops working; 0 = never; omit to keep the current expiry. */
  expiresInDays?: number;
  /** Issue a new token; the old link stops working immediately. */
  regenerate?: boolean;
}

/** What's behind a public share link (no sign-in needed). */
export interface SharedFileInfo {
  fileId: string;
  name: string;
  workspaceName: string;
  sharedBy: string | null;
  updatedAt: string;
  expiresAt: string | null;
}

export interface ApiToken {
  id: string;
  name: string;
  prefix: string;
  createdAt: string;
  lastUsedAt: string | null;
}

/** Response of `POST /api/tokens`; `token` is only ever shown once. */
export interface CreatedApiToken {
  id: string;
  name: string;
  prefix: string;
  token: string;
}

/** Setup snippets for AI agents. */
export interface AgentSetup {
  mcpUrl: string;
  skillInstallCommand: string;
  /** OAuth (browser sign-in) variants — recommended. */
  claudeCommand: string;
  codexConfig: string;
  codexCommand: string;
  /** Bearer-token variants for headless agents; contain `$FLEXMA_TOKEN`. */
  claudeTokenCommand: string;
  codexTokenCommand: string;
  codexTokenConfig: string;
  /** Windows PowerShell variants. */
  windows?: {
    skillInstallCommand: string;
    claudeCommand: string;
    codexCommand: string;
    claudeTokenCommand: string;
    codexTokenCommand: string;
  };
}

export interface FileVersion {
  id: string;
  /** Null for automatic snapshots. */
  label: string | null;
  version: number;
  createdAt: string;
  createdBy: string | null;
}

export interface CommentAuthor {
  id: string | null;
  name: string;
  color: string;
  isAgent: boolean;
}

export interface Comment {
  id: string;
  parentId: string | null;
  pageId: string | null;
  nodeId: string | null;
  x: number;
  y: number;
  author: CommentAuthor;
  body: string;
  resolved: boolean;
  createdAt: string;
  updatedAt: string;
}

export interface CommentThread extends Comment {
  replies: Comment[];
}

export interface NewComment {
  body: string;
  parentId?: string;
  pageId?: string;
  nodeId?: string | null;
  x?: number;
  y?: number;
}

export interface AdminUser {
  id: string;
  email: string;
  name: string;
  color: string;
  isAdmin: boolean;
  createdAt: string;
  workspaceCount: number;
  lastSignIn: string | null;
}

export interface InstanceSettings {
  allowSignup: boolean;
  instanceName: string;
  publicUrl: string | null;
  version: string;
}

export interface UploadedAsset {
  id: string;
  url: string;
  width: number;
  height: number;
}

export interface SignupInput {
  email: string;
  name: string;
  password: string;
  inviteToken?: string;
  /** Required for the first (admin) account; it's in the setup link from the server log. */
  setupToken?: string;
}

export interface LoginInput {
  email: string;
  password: string;
}

export interface UpdateMeInput {
  name?: string;
  password?: string;
  currentPassword?: string;
}

export interface FilePatch {
  name?: string;
  projectId?: string;
}

export interface ExportOptions {
  /** Export a single node. */
  node?: string;
  /** Export a specific page (defaults to the first page). */
  page?: string;
}

export interface PngExportOptions extends ExportOptions {
  /** Pixel density multiplier, e.g. 2 for @2x. */
  scale?: number;
}

// ---------------------------------------------------------------------------
// Transport
// ---------------------------------------------------------------------------

export class ApiError extends Error {
  readonly status: number;

  constructor(status: number, message: string) {
    super(message);
    this.name = 'ApiError';
    this.status = status;
  }
}

/** Human readable message for any thrown value. */
export function errorMessage(err: unknown, fallback = 'Something went wrong'): string {
  if (err instanceof Error && err.message) return err.message;
  if (typeof err === 'string' && err) return err;
  return fallback;
}

export function isApiError(err: unknown, status?: number): err is ApiError {
  return err instanceof ApiError && (status === undefined || err.status === status);
}

type UnauthorizedListener = () => void;
const unauthorizedListeners = new Set<UnauthorizedListener>();

/**
 * Subscribe to 401 responses from regular endpoints (not auth endpoints).
 * Used by the AuthProvider to notice expired sessions. Returns an unsubscribe fn.
 */
export function onUnauthorized(listener: UnauthorizedListener): () => void {
  unauthorizedListeners.add(listener);
  return () => {
    unauthorizedListeners.delete(listener);
  };
}

interface RequestOptions {
  body?: unknown;
  /** Raw body (e.g. image bytes); sent as-is with `contentType`. */
  rawBody?: BodyInit;
  contentType?: string;
  /** Set false for endpoints where a 401 is an expected answer. */
  reportUnauthorized?: boolean;
  signal?: AbortSignal;
}

async function request<T>(method: string, path: string, opts: RequestOptions = {}): Promise<T> {
  const headers: Record<string, string> = { Accept: 'application/json' };
  let body: BodyInit | undefined;
  if (opts.rawBody !== undefined) {
    body = opts.rawBody;
    if (opts.contentType) headers['Content-Type'] = opts.contentType;
  } else if (opts.body !== undefined) {
    body = JSON.stringify(opts.body);
    headers['Content-Type'] = 'application/json';
  }

  let res: Response;
  try {
    res = await fetch(path, {
      method,
      headers,
      body,
      credentials: 'same-origin',
      signal: opts.signal,
    });
  } catch (err) {
    if (err instanceof DOMException && err.name === 'AbortError') throw err;
    throw new ApiError(0, 'Could not reach the Flexma server. Check your connection.');
  }

  const text = await res.text();
  let data: unknown = undefined;
  if (text) {
    try {
      data = JSON.parse(text);
    } catch {
      data = undefined;
    }
  }

  if (!res.ok) {
    if (res.status === 401 && opts.reportUnauthorized !== false) {
      unauthorizedListeners.forEach((l) => l());
    }
    const serverMessage =
      data && typeof data === 'object' && 'error' in data && typeof data.error === 'string'
        ? data.error
        : '';
    throw new ApiError(res.status, serverMessage || defaultMessage(res.status, res.statusText));
  }

  return (data ?? {}) as T;
}

function defaultMessage(status: number, statusText: string): string {
  switch (status) {
    case 401:
      return 'You need to log in.';
    case 403:
      return "You don't have permission to do that.";
    case 404:
      return 'Not found.';
    case 413:
      return 'That file is too large.';
    default:
      return statusText ? `${status} ${statusText}` : `Request failed (${status})`;
  }
}

const enc = encodeURIComponent;

function withQuery(path: string, params: Record<string, string | number | undefined>): string {
  const qs = new URLSearchParams();
  for (const [key, value] of Object.entries(params)) {
    if (value !== undefined && value !== '') qs.set(key, String(value));
  }
  const s = qs.toString();
  return s ? `${path}?${s}` : path;
}

// ---------------------------------------------------------------------------
// URL helpers (for <img src>, downloads, sockets)
// ---------------------------------------------------------------------------

/** SVG preview of a file's first page. Pass `version` (e.g. `updatedAt`) to bust caches. */
export function thumbnailUrl(fileId: string, version?: string): string {
  return withQuery(`/api/files/${enc(fileId)}/thumbnail.png`, { v: version });
}

export function exportPngUrl(fileId: string, opts: PngExportOptions = {}): string {
  return withQuery(`/api/files/${enc(fileId)}/export.png`, {
    node: opts.node,
    page: opts.page,
    scale: opts.scale,
  });
}

export function exportSvgUrl(fileId: string, opts: ExportOptions = {}): string {
  return withQuery(`/api/files/${enc(fileId)}/export.svg`, { node: opts.node, page: opts.page });
}

export function assetUrl(assetId: string): string {
  return `/api/assets/${enc(assetId)}`;
}

/** Absolute ws:// or wss:// URL of a file's multiplayer socket. */
export function fileSocketUrl(fileId: string): string {
  const proto = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
  return `${proto}//${window.location.host}/api/files/${enc(fileId)}/ws`;
}

/** URLs behind a public share link. They work without signing in. */
export const shareUrls = {
  socket(token: string): string {
    const proto = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
    return `${proto}//${window.location.host}/api/share/${enc(token)}/ws`;
  },
  /** Prefix the engine appends asset ids to. */
  assetBase: (token: string) => `/api/share/${enc(token)}/assets/`,
  asset: (token: string, assetId: string) => `/api/share/${enc(token)}/assets/${enc(assetId)}`,
  exportPng: (token: string, opts: PngExportOptions = {}) =>
    withQuery(`/api/share/${enc(token)}/export.png`, { node: opts.node, page: opts.page, scale: opts.scale }),
  exportSvg: (token: string, opts: ExportOptions = {}) =>
    withQuery(`/api/share/${enc(token)}/export.svg`, { node: opts.node, page: opts.page }),
};

// ---------------------------------------------------------------------------
// Endpoints
// ---------------------------------------------------------------------------

type UserEnvelope = { user: User };
const unwrapUser = (r: UserEnvelope): User => r.user;

const auth = {
  setup: () => request<SetupInfo>('GET', '/api/setup', { reportUnauthorized: false }),
  signup: (input: SignupInput) =>
    request<UserEnvelope>('POST', '/api/auth/signup', { body: input, reportUnauthorized: false }).then(
      unwrapUser,
    ),
  login: (input: LoginInput) =>
    request<UserEnvelope>('POST', '/api/auth/login', { body: input, reportUnauthorized: false }).then(
      unwrapUser,
    ),
  logout: () => request<Record<string, never>>('POST', '/api/auth/logout', { reportUnauthorized: false }),
  /** Current user; throws ApiError(401) when logged out. */
  me: () => request<UserEnvelope>('GET', '/api/me', { reportUnauthorized: false }).then(unwrapUser),
  updateMe: (patch: UpdateMeInput) =>
    request<UserEnvelope>('PATCH', '/api/me', { body: patch, reportUnauthorized: false }).then(unwrapUser),
};

const workspaces = {
  list: () => request<Workspace[]>('GET', '/api/workspaces'),
  create: (name: string) => request<Workspace>('POST', '/api/workspaces', { body: { name } }),
  get: (id: string) => request<Workspace>('GET', `/api/workspaces/${enc(id)}`),
  update: (id: string, patch: { name: string }) =>
    request<Workspace>('PATCH', `/api/workspaces/${enc(id)}`, { body: patch }),
  delete: (id: string) => request<Record<string, never>>('DELETE', `/api/workspaces/${enc(id)}`),

  listMembers: (id: string) => request<Member[]>('GET', `/api/workspaces/${enc(id)}/members`),
  updateMember: (id: string, userId: string, role: Role) =>
    request<Record<string, never>>('PATCH', `/api/workspaces/${enc(id)}/members/${enc(userId)}`, {
      body: { role },
    }),
  /** Owner removes a member, or a member removes themself (leave). */
  removeMember: (id: string, userId: string) =>
    request<Record<string, never>>('DELETE', `/api/workspaces/${enc(id)}/members/${enc(userId)}`),

  listInvites: (id: string) => request<Invite[]>('GET', `/api/workspaces/${enc(id)}/invites`),
  createInvite: (id: string, input: { role: Role; email?: string }) =>
    request<Invite>('POST', `/api/workspaces/${enc(id)}/invites`, { body: input }),
};

const invites = {
  delete: (id: string) => request<Record<string, never>>('DELETE', `/api/invites/${enc(id)}`),
  /** Public: works logged out. */
  getByToken: (token: string) =>
    request<InviteInfo>('GET', `/api/invites/by-token/${enc(token)}`, { reportUnauthorized: false }),
  accept: (token: string) =>
    request<{ workspaceId: string }>('POST', `/api/invites/by-token/${enc(token)}/accept`),
};

const projects = {
  list: (workspaceId: string) => request<Project[]>('GET', `/api/workspaces/${enc(workspaceId)}/projects`),
  create: (workspaceId: string, name: string) =>
    request<Project>('POST', `/api/workspaces/${enc(workspaceId)}/projects`, { body: { name } }),
  update: (id: string, patch: { name: string }) =>
    request<Project>('PATCH', `/api/projects/${enc(id)}`, { body: patch }),
  /** Deletes the project and all of its files. */
  delete: (id: string) => request<Record<string, never>>('DELETE', `/api/projects/${enc(id)}`),
  listFiles: (id: string) => request<FileInfo[]>('GET', `/api/projects/${enc(id)}/files`),
};

const files = {
  /** Most recently updated files in a workspace (max 50). */
  listRecent: (workspaceId: string) => request<FileInfo[]>('GET', `/api/workspaces/${enc(workspaceId)}/files`),
  list: (projectId: string) => projects.listFiles(projectId),
  create: (projectId: string, name: string) =>
    request<FileInfo>('POST', `/api/projects/${enc(projectId)}/files`, { body: { name } }),
  get: (id: string) => request<FileInfo>('GET', `/api/files/${enc(id)}`),
  update: (id: string, patch: FilePatch) => request<FileInfo>('PATCH', `/api/files/${enc(id)}`, { body: patch }),
  delete: (id: string) => request<Record<string, never>>('DELETE', `/api/files/${enc(id)}`),
  duplicate: (id: string) => request<FileInfo>('POST', `/api/files/${enc(id)}/duplicate`),
  /** Upload raw image bytes (Content-Type taken from the blob, e.g. image/png). */
  uploadAsset: (fileId: string, blob: Blob) =>
    request<UploadedAsset>('POST', `/api/files/${enc(fileId)}/assets`, {
      rawBody: blob,
      contentType: blob.type || 'application/octet-stream',
    }),
  thumbnailUrl,
  exportPngUrl,
  exportSvgUrl,
  socketUrl: fileSocketUrl,
  /** Version history (automatic snapshots + named versions). */
  versions: (id: string) => request<FileVersion[]>('GET', `/api/files/${enc(id)}/versions`),
  saveVersion: (id: string, label: string) =>
    request<{ id: string }>('POST', `/api/files/${enc(id)}/versions`, { body: { label } }),
  restoreVersion: (id: string, versionId: string) =>
    request<{ restored: string }>('POST', `/api/files/${enc(id)}/versions/${enc(versionId)}/restore`),
  versionPreviewUrl: (id: string, versionId: string) => `/api/files/${enc(id)}/versions/${enc(versionId)}/preview.png`,
};

const fonts = {
  /** Every face in a workspace, sorted by family. */
  list: (workspaceId: string) => request<WorkspaceFont[]>('GET', `/api/workspaces/${enc(workspaceId)}/fonts`),
  /** Upload one .ttf/.otf file (editors). Replaces a face with the same family, weight and style. */
  upload: (workspaceId: string, file: Blob) =>
    request<WorkspaceFont>('POST', `/api/workspaces/${enc(workspaceId)}/fonts`, {
      rawBody: file,
      contentType: 'application/octet-stream',
    }),
  /** Add every style of a Google Fonts family (editors). The server downloads and serves it. */
  addGoogle: (workspaceId: string, family: string) =>
    request<{ family: string; styles: string[] }>('POST', `/api/workspaces/${enc(workspaceId)}/fonts/google`, {
      body: { family },
    }),
  deleteFamily: (workspaceId: string, family: string) =>
    request<Record<string, never>>('DELETE', withQuery(`/api/workspaces/${enc(workspaceId)}/fonts`, { family })),
  fileUrl: (fontId: string) => `/api/fonts/${enc(fontId)}`,
  googleCatalogue: () => request<GoogleFontFamily[]>('GET', '/api/google-fonts'),
  /** A tiny font with just the family name's glyphs, for previews. */
  googlePreviewUrl: (family: string) => withQuery('/api/google-fonts/preview', { family }),
  /** For share-link guests. */
  shareList: (token: string) => request<WorkspaceFont[]>('GET', `/api/share/${enc(token)}/fonts`, { reportUnauthorized: false }),
  shareFileUrl: (token: string, fontId: string) => `/api/share/${enc(token)}/fonts/${enc(fontId)}`,
};

const share = {
  /** The file's public link, if one is on. Any member can read it. */
  get: (fileId: string) => request<ShareLink>('GET', `/api/files/${enc(fileId)}/share`),
  /** Turn the link on/off, change its expiry or regenerate it (editors only). */
  update: (fileId: string, body: ShareUpdate) => request<ShareLink>('PUT', `/api/files/${enc(fileId)}/share`, { body }),
  /** Public: the file behind a link. */
  info: (token: string) => request<SharedFileInfo>('GET', `/api/share/${enc(token)}`, { reportUnauthorized: false }),
  urls: shareUrls,
};

const comments = {
  list: (fileId: string) => request<CommentThread[]>('GET', `/api/files/${enc(fileId)}/comments`),
  create: (fileId: string, c: NewComment) => request<{ id: string }>('POST', `/api/files/${enc(fileId)}/comments`, { body: c }),
  update: (id: string, patch: { body?: string; resolved?: boolean }) =>
    request<Record<string, never>>('PATCH', `/api/comments/${enc(id)}`, { body: patch }),
  delete: (id: string) => request<Record<string, never>>('DELETE', `/api/comments/${enc(id)}`),
};

const admin = {
  users: () => request<AdminUser[]>('GET', '/api/admin/users'),
  updateUser: (id: string, patch: { isAdmin?: boolean }) => request<Record<string, never>>('PATCH', `/api/admin/users/${enc(id)}`, { body: patch }),
  deleteUser: (id: string) => request<Record<string, never>>('DELETE', `/api/admin/users/${enc(id)}`),
  resetPassword: (id: string) => request<{ password: string }>('POST', `/api/admin/users/${enc(id)}/reset-password`),
  settings: () => request<InstanceSettings>('GET', '/api/admin/settings'),
  updateSettings: (patch: { allowSignup?: boolean }) => request<InstanceSettings>('PATCH', '/api/admin/settings', { body: patch }),
};

const tokens = {
  list: () => request<ApiToken[]>('GET', '/api/tokens'),
  create: (name: string) => request<CreatedApiToken>('POST', '/api/tokens', { body: { name } }),
  delete: (id: string) => request<Record<string, never>>('DELETE', `/api/tokens/${enc(id)}`),
  agentSetup: () => request<AgentSetup>('GET', '/api/agent-setup'),
};

export const api = {
  /** Shorthands for the most common calls. */
  setup: auth.setup,
  me: auth.me,
  updateMe: auth.updateMe,
  agentSetup: tokens.agentSetup,

  auth,
  workspaces,
  invites,
  projects,
  files,
  tokens,
  comments,
  fonts,
  share,
  admin,
  assets: { url: assetUrl },
};

export type Api = typeof api;
