//! Model Context Protocol endpoint (`POST /mcp`, Streamable HTTP transport with
//! JSON responses). Authenticated with personal API tokens; there is no limit on
//! tokens or concurrent connections. Every edit goes through the same document
//! actor as human edits, so collaborators watch agents work in real time.

use crate::api_files::{export_targets, get_file_json, insert_file};
use crate::auth::{authenticate, file_access, project_access, require_workspace, AuthUser, Role};
use crate::db::{new_id, now, random_string, ts};
use crate::error::AppError;
use crate::render;
use crate::rooms::{AgentInfo, RoomHandle};
use crate::state::AppState;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use flexma_doc::friendly::{build_create_ops, normalize_props};
use flexma_doc::geom::{fmt_num, Vec2};
use flexma_doc::{Document, Node, NodeId, NodeType, Op, Rect, ROOT_ID};
use rusqlite::{params, OptionalExtension};
use std::collections::HashMap;
use serde_json::{json, Map, Value};

const SUPPORTED_VERSIONS: &[&str] = &["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];

const INSTRUCTIONS: &str = "Flexma is a self-hosted collaborative design tool (like Figma). \
Hierarchy: workspace > project > file > page > nodes. Nodes: FRAME (container/artboard, supports auto layout), \
GROUP, RECTANGLE, ELLIPSE, TEXT, LINE, VECTOR (SVG path), POLYGON, STAR, COMPONENT (reusable main component) and \
INSTANCE (a copy of a component; its layers have ids like inst~layer and edits to them become overrides). Coordinates are pixels relative to the parent; \
y grows downward. Typical flow: list_workspaces -> list_files (or create_file) -> get_file_tree -> create_nodes / update_nodes \
-> get_screenshot to check your work visually. Prefer auto-layout frames (layout: VERTICAL|HORIZONTAL, gap, padding, \
justify, alignItems, width/height: number|\"fill\"|\"hug\") for UI. Colors are hex strings. Edits appear live to everyone \
in the file. People leave feedback as comments: list_comments -> fix -> add_comment(reply_to) -> resolve_comment. Call \
save_version before large or destructive edits. Load the `flexma` skill for the full node spec and design recipes.";

fn tool(name: &str, description: &str, schema: Value) -> Value {
    json!({ "name": name, "description": description, "inputSchema": schema })
}

fn tools() -> Value {
    let file_id = json!({ "type": "string", "description": "File id" });
    let node_spec = json!({
        "type": "object",
        "description": "Node spec. Required: type. Common: name, x, y, width|w, height|h (number, or \"fill\"/\"hug\" inside auto layout), fill (hex or paint or array), stroke, strokeWeight, radius (number or [tl,tr,br,bl]), opacity, rotation, visible, shadow {x,y,blur,spread,color}. FRAME: layout (VERTICAL|HORIZONTAL|NONE), gap, padding (number|[v,h]|[t,r,b,l]), justify (start|center|end|space-between), alignItems (start|center|end), clip, children[]. TEXT: text, size, weight (400|700|\"bold\"...), font, color, align (LEFT|CENTER|RIGHT), lineHeight (px), letterSpacing. LINE: x1,y1,x2,y2 or width+rotation. VECTOR: path (SVG d). POLYGON/STAR: points, innerRadius. Image fill: {type:'IMAGE', asset:'<asset id>', fit:'FILL'|'FIT'}.",
        "properties": { "type": { "type": "string" }, "children": { "type": "array", "items": { "type": "object" } } },
        "required": ["type"],
        "additionalProperties": true
    });
    json!([
        tool("list_workspaces", "List the workspaces (shared team spaces) you belong to, with their projects.", json!({ "type": "object", "properties": {} })),
        tool("list_files", "List design files. Filter by workspace_id or project_id; without filters returns your most recently edited files.", json!({
            "type": "object",
            "properties": { "workspace_id": { "type": "string" }, "project_id": { "type": "string" } }
        })),
        tool("create_project", "Create a project (folder of files) in a workspace.", json!({
            "type": "object",
            "properties": { "workspace_id": { "type": "string" }, "name": { "type": "string" } },
            "required": ["workspace_id", "name"]
        })),
        tool("create_file", "Create a new design file in a project. Returns its id and a browser link for the user.", json!({
            "type": "object",
            "properties": { "project_id": { "type": "string" }, "name": { "type": "string" } },
            "required": ["project_id", "name"]
        })),
        tool("get_file_tree", "Outline of a file: pages and the node tree (id, type, name, position, size, key styles). Use node_id to zoom into a subtree and depth to control detail.", json!({
            "type": "object",
            "properties": {
                "file_id": file_id,
                "page_id": { "type": "string", "description": "Page to outline (default: all pages, shallow)" },
                "node_id": { "type": "string", "description": "Outline only this subtree" },
                "depth": { "type": "integer", "description": "Max depth (default 4)" }
            },
            "required": ["file_id"]
        })),
        tool("get_nodes", "Full properties of specific nodes as JSON, including absoluteBounds.", json!({
            "type": "object",
            "properties": { "file_id": file_id, "node_ids": { "type": "array", "items": { "type": "string" } } },
            "required": ["file_id", "node_ids"]
        })),
        tool("create_nodes", "Create one or more nodes (with nested children) in a single atomic step. Top-level nodes go into parent_id (default: the first page). Without x/y on a page, new content is placed to the right of existing content. Returns the created ids as an outline.", json!({
            "type": "object",
            "properties": {
                "file_id": file_id,
                "parent_id": { "type": "string", "description": "Page, FRAME or GROUP id" },
                "index": { "type": "integer", "description": "Insert position among the parent's children (0 = back). Default: front-most." },
                "nodes": { "type": "array", "items": node_spec }
            },
            "required": ["file_id", "nodes"]
        })),
        tool("update_nodes", "Change properties of existing nodes. Each update is {id, ...props} using the same props as create_nodes (e.g. {id, fill:'#f00', text:'Hi', width:'fill'}). Use move_nodes to change parents.", json!({
            "type": "object",
            "properties": {
                "file_id": file_id,
                "updates": { "type": "array", "items": { "type": "object", "properties": { "id": { "type": "string" } }, "required": ["id"], "additionalProperties": true } }
            },
            "required": ["file_id", "updates"]
        })),
        tool("delete_nodes", "Delete nodes (and their children).", json!({
            "type": "object",
            "properties": { "file_id": file_id, "node_ids": { "type": "array", "items": { "type": "string" } } },
            "required": ["file_id", "node_ids"]
        })),
        tool("move_nodes", "Reparent and/or reorder nodes. Keeps their position on the canvas unless keep_position is false.", json!({
            "type": "object",
            "properties": {
                "file_id": file_id,
                "node_ids": { "type": "array", "items": { "type": "string" } },
                "parent_id": { "type": "string" },
                "index": { "type": "integer", "description": "Position among the new parent's children (0 = back). Default: front-most." },
                "keep_position": { "type": "boolean" }
            },
            "required": ["file_id", "node_ids", "parent_id"]
        })),
        tool("duplicate_nodes", "Duplicate nodes (with children) next to the originals, offset by dx/dy (default: to the right).", json!({
            "type": "object",
            "properties": {
                "file_id": file_id,
                "node_ids": { "type": "array", "items": { "type": "string" } },
                "dx": { "type": "number" }, "dy": { "type": "number" }
            },
            "required": ["file_id", "node_ids"]
        })),
        tool("find_nodes", "Search nodes by name or text content (case-insensitive) and/or type.", json!({
            "type": "object",
            "properties": {
                "file_id": file_id,
                "query": { "type": "string" },
                "type": { "type": "string" },
                "page_id": { "type": "string" },
                "limit": { "type": "integer" }
            },
            "required": ["file_id"]
        })),
        tool("create_page", "Add a page to a file.", json!({
            "type": "object",
            "properties": { "file_id": file_id, "name": { "type": "string" } },
            "required": ["file_id", "name"]
        })),
        tool("get_screenshot", "Render a node or page to PNG so you can see the design. Use it to verify your work.", json!({
            "type": "object",
            "properties": {
                "file_id": file_id,
                "node_id": { "type": "string" },
                "page_id": { "type": "string" },
                "scale": { "type": "number", "description": "Default 1; output is capped at 1600px on the long side" }
            },
            "required": ["file_id"]
        })),
        tool("export_svg", "Export a node or page as SVG markup.", json!({
            "type": "object",
            "properties": { "file_id": file_id, "node_id": { "type": "string" }, "page_id": { "type": "string" } },
            "required": ["file_id"]
        })),
        tool("share_file", "Create (or turn off) a public view-only link to a file, so anyone with the link can watch it live without an account. Returns the URL.", json!({
            "type": "object",
            "properties": {
                "file_id": file_id,
                "enabled": { "type": "boolean", "description": "Default true; false turns the link off" },
                "expires_in_days": { "type": "integer", "description": "Days until the link stops working; 0 = never. Omit to keep the current expiry (a new link never expires)." }
            },
            "required": ["file_id"]
        })),
        tool("list_fonts", "List the fonts available in a file's workspace (uploaded files and Google Fonts added to it) with their styles. Inter is always available. To use another Google Fonts family, add it first with add_font so everyone sees it and exports use it.", json!({
            "type": "object",
            "properties": { "file_id": file_id },
            "required": ["file_id"]
        })),
        tool("add_font", "Add a Google Fonts family (every weight and italic) to the file's workspace. The server downloads it once and serves it to everyone. Then set fontFamily to the family name.", json!({
            "type": "object",
            "properties": {
                "file_id": file_id,
                "family": { "type": "string", "description": "Exact Google Fonts family name, e.g. 'Poppins' or 'DM Serif Display'" }
            },
            "required": ["file_id", "family"]
        })),
        tool("upload_font", "Upload your own font files (.ttf or .otf, e.g. a brand or game font on the user's computer) to the file's workspace, so everyone sees them and exports use them. Returns a curl command with a short-lived upload link; run it once per font file. For Google Fonts, use add_font instead.", json!({
            "type": "object",
            "properties": {
                "file_id": file_id,
                "files": { "type": "integer", "description": "How many font files you'll upload (one per style: Regular, Bold, Italic…). Default 1, max 20." }
            },
            "required": ["file_id"]
        })),
        tool("upload_image", "Upload images (PNG, JPEG, GIF or WebP) from the user's computer, or ones you downloaded, into the file's workspace. Returns a curl command with a short-lived upload link; run it once per image, then place each with create_nodes {type:'image', asset:'<id>'}.", json!({
            "type": "object",
            "properties": {
                "file_id": file_id,
                "files": { "type": "integer", "description": "How many images you'll upload. Default 1, max 20." }
            },
            "required": ["file_id"]
        })),
        tool("list_images", "List images already uploaded to the file's workspace (newest first): asset id, size, type, who uploaded it, and whether this file uses it. Reuse them with {type:'image', asset:'<id>'}.", json!({
            "type": "object",
            "properties": { "file_id": file_id },
            "required": ["file_id"]
        })),
        tool("undo_agent_changes", "Undo your own recent changes in a file, newest first: layers you created are removed, layers you deleted come back and properties you edited return to their earlier values. Anything a person changed after you is kept. Your last 30 changes per file are remembered while the server runs.", json!({
            "type": "object",
            "properties": {
                "file_id": file_id,
                "steps": { "type": "integer", "description": "How many of your tool calls to undo (default 1, max 30)" }
            },
            "required": ["file_id"]
        })),
        tool("list_components", "List the main components in a file (the design system): id, name, size, page and instance count. Use them via create_nodes with {type:'instance', component:'<name or id>'}.", json!({
            "type": "object",
            "properties": { "file_id": file_id },
            "required": ["file_id"]
        })),
        tool("create_component", "Turn an existing FRAME or GROUP into a reusable main component (keeps its look and children). Returns the component id. Then place copies with create_nodes {type:'instance', component:'<name or id>', text:'…', overrides:{'<layer name>':{…}}}.", json!({
            "type": "object",
            "properties": { "file_id": file_id, "node_id": { "type": "string" }, "name": { "type": "string" } },
            "required": ["file_id", "node_id"]
        })),
        tool("detach_instance", "Convert an instance into a plain frame with editable copies of its layers (it stops following the main component).", json!({
            "type": "object",
            "properties": { "file_id": file_id, "node_id": { "type": "string" } },
            "required": ["file_id", "node_id"]
        })),
        tool("list_comments", "Read the comment threads on a file (open ones by default): who said what, where (page/node), and replies. Use it to find feedback to address.", json!({
            "type": "object",
            "properties": { "file_id": file_id, "include_resolved": { "type": "boolean" } },
            "required": ["file_id"]
        })),
        tool("add_comment", "Leave a comment pinned on the canvas (attached to node_id, or at x/y on a page), or reply to a thread with reply_to. Use comments to explain decisions, ask questions, or report what you changed.", json!({
            "type": "object",
            "properties": {
                "file_id": file_id,
                "body": { "type": "string" },
                "node_id": { "type": "string", "description": "Pin the comment to this node" },
                "x": { "type": "number" }, "y": { "type": "number" },
                "page_id": { "type": "string" },
                "reply_to": { "type": "string", "description": "Thread id to reply to" }
            },
            "required": ["file_id", "body"]
        })),
        tool("resolve_comment", "Mark a comment thread resolved (or reopen it with resolved=false) after addressing it.", json!({
            "type": "object",
            "properties": { "file_id": file_id, "comment_id": { "type": "string" }, "resolved": { "type": "boolean" } },
            "required": ["file_id", "comment_id"]
        })),
        tool("save_version", "Save a named version of the file to its version history (people can restore it from Main menu → Version history). Call this before large or destructive edits.", json!({
            "type": "object",
            "properties": { "file_id": file_id, "label": { "type": "string", "description": "e.g. 'Before agent redesign of pricing'" } },
            "required": ["file_id", "label"]
        })),
        tool("get_selection", "What people currently have selected in this file (live collaborators), so you can act on 'the selected thing'.", json!({
            "type": "object",
            "properties": { "file_id": file_id },
            "required": ["file_id"]
        })),
    ])
}

fn rpc_result(id: &Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn rpc_error(id: &Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

fn text_result(text: impl Into<String>) -> Value {
    json!({ "content": [{ "type": "text", "text": text.into() }] })
}

fn error_result(text: impl Into<String>) -> Value {
    json!({ "content": [{ "type": "text", "text": text.into() }], "isError": true })
}

pub async fn get_not_allowed() -> Response {
    (StatusCode::METHOD_NOT_ALLOWED, [(header::ALLOW, "POST")], "Use POST for MCP requests").into_response()
}

pub async fn handle(State(st): State<AppState>, headers: HeaderMap, body: Bytes) -> Response {
    let user = match authenticate(&st.db, &headers).await {
        Ok(Some(u)) => u,
        _ => {
            let meta = crate::oauth::resource_metadata_url(&st.base_url(&headers));
            return (
                StatusCode::UNAUTHORIZED,
                [(header::WWW_AUTHENTICATE, format!("Bearer realm=\"flexma\", resource_metadata=\"{meta}\""))],
                axum::Json(json!({ "error": "authentication required: connect with OAuth (your MCP client will open a browser) or send an API token from Flexma > AI Agents as 'Authorization: Bearer <token>'." })),
            )
                .into_response()
        }
    };
    let msg: Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(e) => return axum::Json(rpc_error(&Value::Null, -32700, &format!("parse error: {e}"))).into_response(),
    };
    let base = st.base_url(&headers);
    let ctx = Ctx { st: st.clone(), user, base };
    match msg {
        Value::Array(batch) => {
            let mut out = Vec::new();
            for m in batch {
                if let Some(r) = ctx.dispatch(m).await {
                    out.push(r);
                }
            }
            if out.is_empty() {
                StatusCode::ACCEPTED.into_response()
            } else {
                axum::Json(Value::Array(out)).into_response()
            }
        }
        m => match ctx.dispatch(m).await {
            Some(r) => axum::Json(r).into_response(),
            None => StatusCode::ACCEPTED.into_response(),
        },
    }
}

struct Ctx {
    st: AppState,
    user: AuthUser,
    base: String,
}

type ToolResult = Result<Value, String>;

fn s<'a>(args: &'a Value, key: &str) -> Result<&'a str, String> {
    args.get(key).and_then(Value::as_str).ok_or_else(|| format!("missing required argument '{key}'"))
}

fn opt_s<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(Value::as_str).filter(|v| !v.is_empty())
}

fn ids(args: &Value, key: &str) -> Result<Vec<String>, String> {
    let arr = args.get(key).and_then(Value::as_array).ok_or_else(|| format!("missing required array '{key}'"))?;
    Ok(arr.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
}

fn err_msg(e: AppError) -> String {
    e.message
}

impl Ctx {
    async fn dispatch(&self, m: Value) -> Option<Value> {
        let id = m.get("id").cloned();
        let method = m.get("method").and_then(Value::as_str).unwrap_or("");
        let params = m.get("params").cloned().unwrap_or(json!({}));
        let id = match id {
            Some(id) if !id.is_null() => id,
            _ => return None, // notification (e.g. notifications/initialized)
        };
        Some(match method {
            "initialize" => {
                let requested = params.get("protocolVersion").and_then(Value::as_str).unwrap_or("");
                let version = if SUPPORTED_VERSIONS.contains(&requested) { requested } else { SUPPORTED_VERSIONS[0] };
                rpc_result(
                    &id,
                    json!({
                        "protocolVersion": version,
                        "capabilities": { "tools": { "listChanged": false } },
                        "serverInfo": { "name": "flexma", "title": "Flexma", "version": env!("CARGO_PKG_VERSION") },
                        "instructions": INSTRUCTIONS,
                    }),
                )
            }
            "ping" => rpc_result(&id, json!({})),
            "tools/list" => rpc_result(&id, json!({ "tools": tools() })),
            "resources/list" => rpc_result(&id, json!({ "resources": [] })),
            "resources/templates/list" => rpc_result(&id, json!({ "resourceTemplates": [] })),
            "prompts/list" => rpc_result(&id, json!({ "prompts": [] })),
            "tools/call" => {
                let name = params.get("name").and_then(Value::as_str).unwrap_or("");
                let args = params.get("arguments").cloned().unwrap_or(json!({}));
                let result = match self.call_tool(name, &args).await {
                    Ok(v) => v,
                    Err(e) => error_result(e),
                };
                rpc_result(&id, result)
            }
            other => rpc_error(&id, -32601, &format!("method not found: {other}")),
        })
    }

    fn agent(&self) -> AgentInfo {
        let label = self.user.token_name.clone().unwrap_or_else(|| "AI agent".into());
        AgentInfo {
            key: format!("{}-{}", self.user.id, crate::db::sha256_hex(&label)[..8].to_string()),
            user_id: self.user.id.clone(),
            name: format!("{label} · {}", self.user.name),
            color: "#d97757".into(),
        }
    }

    async fn room(&self, file_id: &str, min: Role) -> Result<RoomHandle, String> {
        let uid = self.user.id.clone();
        let fid = file_id.to_string();
        self.st.db.call(move |c| file_access(c, &uid, &fid, min)).await.map_err(err_msg)?;
        self.st.rooms.get(file_id).await.map_err(err_msg)
    }

    async fn call_tool(&self, name: &str, args: &Value) -> ToolResult {
        match name {
            "list_workspaces" => self.list_workspaces().await,
            "list_files" => self.list_files(args).await,
            "create_project" => self.create_project(args).await,
            "create_file" => self.create_file(args).await,
            "get_file_tree" => self.get_file_tree(args).await,
            "get_nodes" => self.get_nodes(args).await,
            "create_nodes" => self.create_nodes(args).await,
            "update_nodes" => self.update_nodes(args).await,
            "delete_nodes" => self.delete_nodes(args).await,
            "move_nodes" => self.move_nodes(args).await,
            "duplicate_nodes" => self.duplicate_nodes(args).await,
            "find_nodes" => self.find_nodes(args).await,
            "create_page" => self.create_page(args).await,
            "get_screenshot" => self.get_screenshot(args).await,
            "export_svg" => self.export_svg(args).await,
            "get_selection" => self.get_selection(args).await,
            "save_version" => self.save_version(args).await,
            "list_comments" => self.list_comments(args).await,
            "list_components" => self.list_components(args).await,
            "share_file" => self.share_file(args).await,
            "list_fonts" => self.list_fonts(args).await,
            "add_font" => self.add_font(args).await,
            "upload_font" => self.upload_font(args).await,
            "undo_agent_changes" => self.undo_agent_changes(args).await,
            "upload_image" => self.upload_image(args).await,
            "list_images" => self.list_images(args).await,
            "create_component" => self.create_component(args).await,
            "detach_instance" => self.detach_instance(args).await,
            "add_comment" => self.add_comment(args).await,
            "resolve_comment" => self.resolve_comment(args).await,
            other => Err(format!("unknown tool '{other}'")),
        }
    }

    async fn list_workspaces(&self) -> ToolResult {
        let uid = self.user.id.clone();
        let v = self
            .st
            .db
            .call(move |c| {
                let mut stmt = c.prepare(
                    "SELECT w.id, w.name, m.role FROM workspaces w JOIN workspace_members m ON m.workspace_id = w.id
                     WHERE m.user_id = ?1 ORDER BY w.created_at",
                )?;
                let wss = stmt
                    .query_map(params![uid], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?)))?
                    .collect::<Result<Vec<_>, _>>()?;
                let mut out = Vec::new();
                for (id, name, role) in wss {
                    let mut ps = c.prepare(
                        "SELECT p.id, p.name, (SELECT COUNT(*) FROM files f WHERE f.project_id = p.id) FROM projects p
                         WHERE p.workspace_id = ?1 ORDER BY p.created_at",
                    )?;
                    let projects = ps
                        .query_map(params![id], |r| {
                            Ok(json!({ "id": r.get::<_, String>(0)?, "name": r.get::<_, String>(1)?, "fileCount": r.get::<_, i64>(2)? }))
                        })?
                        .collect::<Result<Vec<_>, _>>()?;
                    out.push(json!({ "id": id, "name": name, "role": role, "projects": projects }));
                }
                Ok(out)
            })
            .await
            .map_err(err_msg)?;
        Ok(text_result(serde_json::to_string_pretty(&json!({ "user": self.user.name, "workspaces": v })).unwrap()))
    }

    async fn list_files(&self, args: &Value) -> ToolResult {
        let uid = self.user.id.clone();
        let ws = opt_s(args, "workspace_id").map(str::to_string);
        let pj = opt_s(args, "project_id").map(str::to_string);
        let base = self.base.clone();
        let v = self
            .st
            .db
            .call(move |c| {
                let (sql, p): (String, String) = if let Some(p) = pj {
                    project_access(c, &uid, &p, Role::Viewer)?;
                    ("p.id = ?2".into(), p)
                } else if let Some(w) = ws {
                    require_workspace(c, &uid, &w, Role::Viewer)?;
                    ("p.workspace_id = ?2".into(), w)
                } else {
                    ("?2 = ?2".into(), String::new())
                };
                let mut stmt = c.prepare(&format!(
                    "SELECT f.id, f.name, p.id, p.name, p.workspace_id, f.updated_at FROM files f
                     JOIN projects p ON p.id = f.project_id
                     JOIN workspace_members m ON m.workspace_id = p.workspace_id AND m.user_id = ?1
                     WHERE {sql} ORDER BY f.updated_at DESC LIMIT 100"
                ))?;
                let rows = stmt
                    .query_map(params![uid, p], |r| {
                        let id: String = r.get(0)?;
                        Ok(json!({
                            "id": id,
                            "name": r.get::<_, String>(1)?,
                            "projectId": r.get::<_, String>(2)?,
                            "projectName": r.get::<_, String>(3)?,
                            "workspaceId": r.get::<_, String>(4)?,
                            "updatedAt": ts(r.get(5)?),
                            "url": format!("{base}/file/{id}"),
                        }))
                    })?
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(rows)
            })
            .await
            .map_err(err_msg)?;
        Ok(text_result(serde_json::to_string_pretty(&v).unwrap()))
    }

    async fn create_project(&self, args: &Value) -> ToolResult {
        let ws = s(args, "workspace_id")?.to_string();
        let name = s(args, "name")?.trim().to_string();
        if name.is_empty() {
            return Err("name must not be empty".into());
        }
        let uid = self.user.id.clone();
        let id = self
            .st
            .db
            .call(move |c| {
                require_workspace(c, &uid, &ws, Role::Editor)?;
                let id = new_id();
                c.execute(
                    "INSERT INTO projects (id, workspace_id, name, created_at) VALUES (?1, ?2, ?3, ?4)",
                    params![id, ws, name, now()],
                )?;
                Ok(id)
            })
            .await
            .map_err(err_msg)?;
        Ok(text_result(json!({ "projectId": id }).to_string()))
    }

    async fn create_file(&self, args: &Value) -> ToolResult {
        let pj = s(args, "project_id")?.to_string();
        let name = opt_s(args, "name").unwrap_or("Untitled").to_string();
        let uid = self.user.id.clone();
        let v = self
            .st
            .db
            .call(move |c| {
                project_access(c, &uid, &pj, Role::Editor)?;
                let id = insert_file(c, &pj, &name, &uid, None)?;
                get_file_json(c, &uid, &id)
            })
            .await
            .map_err(err_msg)?;
        let id = v["id"].as_str().unwrap_or_default();
        Ok(text_result(format!(
            "Created file '{}' (id {id}). Its first page is 0:1. Open it in the browser: {}/file/{id}",
            v["name"].as_str().unwrap_or_default(),
            self.base
        )))
    }

    async fn get_file_tree(&self, args: &Value) -> ToolResult {
        let fid = s(args, "file_id")?.to_string();
        let room = self.room(&fid, Role::Viewer).await?;
        let page = opt_s(args, "page_id").map(str::to_string);
        let node = opt_s(args, "node_id").map(str::to_string);
        let depth = args.get("depth").and_then(Value::as_u64).unwrap_or(4).clamp(1, 50) as usize;
        let url = format!("{}/file/{fid}", self.base);
        room.read(move |doc, peers| {
            let mut out = String::new();
            out.push_str(&format!("File {fid} — {url}\n"));
            let humans = peers.iter().filter(|p| !p.is_agent).count();
            if humans > 0 {
                out.push_str(&format!("{humans} people are viewing this file live.\n"));
            }
            if let Some(n) = node {
                if !doc.contains(&n) {
                    return Err(format!("node '{n}' not found"));
                }
                outline(doc, &n, 0, depth, &mut out);
                return Ok(out);
            }
            let pages: Vec<NodeId> = match &page {
                Some(p) => {
                    if doc.get(p).map(|n| n.kind) != Some(NodeType::Page) {
                        return Err(format!("page '{p}' not found"));
                    }
                    vec![p.clone()]
                }
                None => doc.pages().to_vec(),
            };
            out.push_str(&format!("Pages: {}\n", doc.pages().len()));
            let d = if page.is_none() && pages.len() > 1 { depth.min(2) } else { depth };
            for p in pages {
                outline(doc, &p, 0, d, &mut out);
            }
            Ok(out)
        })
        .await
        .map_err(err_msg)?
        .map(text_result)
    }

    async fn get_nodes(&self, args: &Value) -> ToolResult {
        let fid = s(args, "file_id")?.to_string();
        let wanted = ids(args, "node_ids")?;
        let room = self.room(&fid, Role::Viewer).await?;
        let v = room
            .read(move |doc, _| {
                let mut out = Vec::new();
                for id in wanted.iter().take(200) {
                    match doc.get(id) {
                        Some(n) => {
                            let mut j = n.to_json();
                            let b = doc.world_bounds(id);
                            j["absoluteBounds"] = json!({ "x": b.x, "y": b.y, "width": b.width, "height": b.height });
                            j["children"] = json!(doc.children(id));
                            out.push(j);
                        }
                        None => out.push(json!({ "id": id, "error": "not found" })),
                    }
                }
                out
            })
            .await
            .map_err(err_msg)?;
        Ok(text_result(serde_json::to_string_pretty(&v).unwrap()))
    }

    async fn create_nodes(&self, args: &Value) -> ToolResult {
        let fid = s(args, "file_id")?.to_string();
        let specs = args.get("nodes").and_then(Value::as_array).cloned().ok_or("missing required array 'nodes'")?;
        if specs.is_empty() {
            return Err("'nodes' is empty".into());
        }
        let parent = opt_s(args, "parent_id").map(str::to_string);
        let index = args.get("index").and_then(Value::as_u64).map(|v| v as usize);
        let room = self.room(&fid, Role::Editor).await?;
        let note = self.font_note(&fid, args).await;
        let mut specs = specs;
        self.size_images(&fid, &mut specs).await?;
        let prefix = format!("a{}", random_string(5));
        room.mutate(self.agent(), move |tx| {
            let parent = match parent {
                Some(p) => p,
                None => tx.doc.pages().first().cloned().ok_or("file has no pages")?,
            };
            let mut specs = specs;
            place_on_page(tx.doc, &parent, &mut specs);
            let mut n = 0u32;
            let mut gen = || {
                n += 1;
                format!("{prefix}:{n}")
            };
            let (ops, top) = build_create_ops(tx.doc, &parent, &specs, index, &mut gen)?;
            let count = ops.len();
            let errs = tx.apply_all(ops);
            if !errs.is_empty() {
                return Err(format!("some nodes could not be created: {errs:?}"));
            }
            tx.relayout();
            let mut out = format!("Created {count} node(s) in {parent}:\n");
            for id in &top {
                outline(tx.doc, id, 0, 6, &mut out);
            }
            out.push_str(&note);
            Ok(json!(out))
        })
        .await
        .map_err(err_msg)?
        .map(|v| text_result(v.as_str().unwrap_or_default()))
    }

    async fn update_nodes(&self, args: &Value) -> ToolResult {
        let fid = s(args, "file_id")?.to_string();
        let updates = args.get("updates").and_then(Value::as_array).cloned().ok_or("missing required array 'updates'")?;
        let room = self.room(&fid, Role::Editor).await?;
        let note = self.font_note(&fid, args).await;
        room.mutate(self.agent(), move |tx| {
            let mut ops = Vec::new();
            for (i, u) in updates.iter().enumerate() {
                let obj = u.as_object().ok_or(format!("updates[{i}] must be an object"))?;
                let id = obj.get("id").and_then(Value::as_str).ok_or(format!("updates[{i}] needs an 'id'"))?;
                let node = tx.doc.get(id).ok_or_else(|| format!("node '{id}' not found"))?;
                let mut raw = obj.clone();
                raw.remove("id");
                if raw.contains_key("parent") || raw.contains_key("index") {
                    return Err("use move_nodes to change a node's parent or order".into());
                }
                let mut props = normalize_props(node.kind, &raw).map_err(|e| format!("{id}: {e}"))?;
                if node.kind == NodeType::Text {
                    re_estimate_text(node, &mut props);
                }
                // Resizing an auto-layout frame by hand switches it to fixed sizing.
                if node.kind.is_frame_like() {
                    if props.contains_key("width") && !props.contains_key("layoutSizingHorizontal") && node.layout_sizing_horizontal == flexma_doc::SizingMode::Hug {
                        props.insert("layoutSizingHorizontal".into(), json!("FIXED"));
                    }
                    if props.contains_key("height") && !props.contains_key("layoutSizingVertical") && node.layout_sizing_vertical == flexma_doc::SizingMode::Hug {
                        props.insert("layoutSizingVertical".into(), json!("FIXED"));
                    }
                }
                ops.push(Op::Update { id: id.to_string(), props });
            }
            let n = ops.len();
            let errs = tx.apply_all(ops);
            tx.relayout();
            if errs.is_empty() {
                Ok(json!(format!("Updated {n} node(s).{note}")))
            } else {
                Ok(json!(format!("Updated {} of {n} node(s). Errors: {errs:?}{note}", n - errs.len())))
            }
        })
        .await
        .map_err(err_msg)?
        .map(|v| text_result(v.as_str().unwrap_or_default()))
    }

    async fn delete_nodes(&self, args: &Value) -> ToolResult {
        let fid = s(args, "file_id")?.to_string();
        let targets = ids(args, "node_ids")?;
        let room = self.room(&fid, Role::Editor).await?;
        room.mutate(self.agent(), move |tx| {
            let ops: Vec<Op> = targets.iter().filter(|id| tx.doc.contains(id)).map(|id| Op::Delete { id: id.clone() }).collect();
            let n = ops.len();
            let errs = tx.apply_all(ops);
            if errs.is_empty() {
                Ok(json!(format!("Deleted {n} node(s).")))
            } else {
                Err(format!("{errs:?}"))
            }
        })
        .await
        .map_err(err_msg)?
        .map(|v| text_result(v.as_str().unwrap_or_default()))
    }

    async fn move_nodes(&self, args: &Value) -> ToolResult {
        let fid = s(args, "file_id")?.to_string();
        let targets = ids(args, "node_ids")?;
        let parent = s(args, "parent_id")?.to_string();
        let index = args.get("index").and_then(Value::as_u64).map(|v| v as usize);
        let keep = args.get("keep_position").and_then(Value::as_bool).unwrap_or(true);
        let room = self.room(&fid, Role::Editor).await?;
        room.mutate(self.agent(), move |tx| {
            let pn = tx.doc.get(&parent).ok_or_else(|| format!("parent '{parent}' not found"))?;
            if !pn.kind.is_container() || pn.kind == NodeType::Document {
                return Err(format!("'{parent}' cannot contain nodes"));
            }
            let parent_inv = tx.doc.world_transform(&parent).invert().unwrap_or_default();
            let keys = {
                let sibs: Vec<&Node> = tx
                    .doc
                    .children(&parent)
                    .iter()
                    .filter(|c| !targets.contains(c))
                    .filter_map(|c| tx.doc.get(c))
                    .collect();
                let pos = index.unwrap_or(sibs.len()).min(sibs.len());
                let before = if pos == 0 { None } else { Some(sibs[pos - 1].index.clone()) };
                let after = sibs.get(pos).map(|n| n.index.clone());
                flexma_doc::fractional::keys_between(before.as_deref(), after.as_deref(), targets.len())
            };
            let mut ops = Vec::new();
            for (id, key) in targets.iter().zip(keys) {
                let n = tx.doc.get(id).ok_or_else(|| format!("node '{id}' not found"))?;
                let mut props = Map::new();
                props.insert("parent".into(), json!(parent));
                props.insert("index".into(), json!(key));
                if keep {
                    // Keep the node's top-left (pre-rotation) where it is on the canvas.
                    let world = tx.doc.world_transform(id);
                    let local_rot = flexma_doc::geom::local_transform(0.0, 0.0, n.width, n.height, n.rotation, n.flip_x, n.flip_y);
                    let origin = world.mul(&local_rot.invert().unwrap_or_default()).apply(Vec2::ZERO);
                    let p = parent_inv.apply(origin);
                    props.insert("x".into(), json!(p.x));
                    props.insert("y".into(), json!(p.y));
                }
                ops.push(Op::Update { id: id.clone(), props });
            }
            let n = ops.len();
            let errs = tx.apply_all(ops);
            tx.relayout();
            if errs.is_empty() {
                Ok(json!(format!("Moved {n} node(s) into {parent}.")))
            } else {
                Err(format!("{errs:?}"))
            }
        })
        .await
        .map_err(err_msg)?
        .map(|v| text_result(v.as_str().unwrap_or_default()))
    }

    async fn duplicate_nodes(&self, args: &Value) -> ToolResult {
        let fid = s(args, "file_id")?.to_string();
        let targets = ids(args, "node_ids")?;
        let dx = args.get("dx").and_then(Value::as_f64);
        let dy = args.get("dy").and_then(Value::as_f64).unwrap_or(0.0);
        let room = self.room(&fid, Role::Editor).await?;
        let prefix = format!("a{}", random_string(5));
        room.mutate(self.agent(), move |tx| {
            let mut counter = 0u32;
            let mut out = String::from("Duplicated:\n");
            let mut tops = Vec::new();
            for id in &targets {
                let n = tx.doc.get(id).ok_or_else(|| format!("node '{id}' not found"))?.clone();
                let parent = n.parent.clone().ok_or("cannot duplicate the root")?;
                let mut mapping = std::collections::HashMap::new();
                let mut nodes = Vec::new();
                tx.doc.walk(id, &mut |c| nodes.push(c.clone()));
                for c in &nodes {
                    counter += 1;
                    mapping.insert(c.id.clone(), format!("{prefix}:{counter}"));
                }
                let next_idx = {
                    let sibs = tx.doc.children(&parent);
                    let pos = sibs.iter().position(|s| s == id).unwrap_or(0);
                    let after = sibs.get(pos + 1).and_then(|s| tx.doc.get(s)).map(|s| s.index.clone());
                    flexma_doc::fractional::key_between(Some(&n.index), after.as_deref())
                };
                let mut ops = Vec::new();
                for c in nodes {
                    let mut c2 = c.clone();
                    c2.id = mapping[&c.id].clone();
                    if c.id == *id {
                        c2.index = next_idx.clone();
                        c2.x += dx.unwrap_or(n.width + 40.0);
                        c2.y += dy;
                    } else if let Some(p) = &c.parent {
                        c2.parent = mapping.get(p).cloned().or(Some(p.clone()));
                    }
                    ops.push(Op::Create { node: c2 });
                }
                let errs = tx.apply_all(ops);
                if !errs.is_empty() {
                    return Err(format!("{errs:?}"));
                }
                tops.push(mapping[id].clone());
            }
            tx.relayout();
            for t in &tops {
                outline(tx.doc, t, 0, 2, &mut out);
            }
            Ok(json!(out))
        })
        .await
        .map_err(err_msg)?
        .map(|v| text_result(v.as_str().unwrap_or_default()))
    }

    async fn find_nodes(&self, args: &Value) -> ToolResult {
        let fid = s(args, "file_id")?.to_string();
        let q = opt_s(args, "query").map(|s| s.to_lowercase());
        let ty = match opt_s(args, "type") {
            Some(t) => Some(flexma_doc::friendly::parse_type(t).ok_or_else(|| format!("unknown type '{t}'"))?),
            None => None,
        };
        let page = opt_s(args, "page_id").map(str::to_string);
        let limit = args.get("limit").and_then(Value::as_u64).unwrap_or(50).clamp(1, 500) as usize;
        let room = self.room(&fid, Role::Viewer).await?;
        let out = room
            .read(move |doc, _| {
                let mut out = String::new();
                let mut count = 0;
                let root = page.clone().unwrap_or_else(|| ROOT_ID.to_string());
                doc.walk(&root, &mut |n| {
                    if count >= limit || !n.kind.is_scene() {
                        return;
                    }
                    if let Some(t) = ty {
                        if n.kind != t {
                            return;
                        }
                    }
                    if let Some(q) = &q {
                        if !n.name.to_lowercase().contains(q) && !n.characters.to_lowercase().contains(q) {
                            return;
                        }
                    }
                    count += 1;
                    out.push_str(&node_line(doc, n));
                    out.push('\n');
                });
                if count == 0 {
                    "No matching nodes.".to_string()
                } else {
                    out
                }
            })
            .await
            .map_err(err_msg)?;
        Ok(text_result(out))
    }

    async fn create_page(&self, args: &Value) -> ToolResult {
        let fid = s(args, "file_id")?.to_string();
        let name = s(args, "name")?.to_string();
        let room = self.room(&fid, Role::Editor).await?;
        let id = format!("a{}:1", random_string(6));
        room.mutate(self.agent(), move |tx| {
            let mut page = Node::new(id.clone(), NodeType::Page);
            page.name = name;
            page.parent = Some(ROOT_ID.into());
            page.index = tx.doc.index_at_end(ROOT_ID);
            tx.apply(Op::Create { node: page })?;
            Ok(json!(format!("Created page {id}.")))
        })
        .await
        .map_err(err_msg)?
        .map(|v| text_result(v.as_str().unwrap_or_default()))
    }

    async fn get_screenshot(&self, args: &Value) -> ToolResult {
        let fid = s(args, "file_id")?.to_string();
        let node = opt_s(args, "node_id").map(str::to_string);
        let page = opt_s(args, "page_id").map(str::to_string);
        let scale = args.get("scale").and_then(Value::as_f64).unwrap_or(1.0) as f32;
        let room = self.room(&fid, Role::Viewer).await?;
        let data_dir = self.st.cfg.data_dir.clone();
        let (svg, bounds) = room
            .read(move |doc, _| {
                let (ids, bg) = export_targets(doc, node.as_deref(), page.as_deref())?;
                let bg = bg.or(Some(flexma_doc::Color::WHITE));
                render::export_svg(doc, &ids, bg, render::AssetMode::Inline(&data_dir)).ok_or_else(|| "nothing to render".to_string())
            })
            .await
            .map_err(err_msg)??;
        let f2 = fid.clone();
        let ws = self.st.db.call(move |c| crate::fonts::file_workspace(c, &f2)).await.map_err(err_msg)?;
        let fonts = crate::fonts::fontdb_for(&self.st, &ws).await;
        let (png, w, h) = tokio::task::spawn_blocking(move || render::svg_to_png(&svg, scale, 1600, fonts))
            .await
            .map_err(|e| e.to_string())??;
        use base64::Engine;
        let data = base64::engine::general_purpose::STANDARD.encode(png);
        Ok(json!({
            "content": [
                { "type": "image", "data": data, "mimeType": "image/png" },
                { "type": "text", "text": format!(
                    "Rendered {}×{} px of canvas area x={} y={} w={} h={}, with the workspace's fonts. Line breaks can differ slightly from the browser editor, which is the source of truth.",
                    w, h, fmt_num(bounds.x), fmt_num(bounds.y), fmt_num(bounds.width), fmt_num(bounds.height)
                ) }
            ]
        }))
    }

    async fn export_svg(&self, args: &Value) -> ToolResult {
        let fid = s(args, "file_id")?.to_string();
        let node = opt_s(args, "node_id").map(str::to_string);
        let page = opt_s(args, "page_id").map(str::to_string);
        let room = self.room(&fid, Role::Viewer).await?;
        let base = self.base.clone();
        let svg = room
            .read(move |doc, _| {
                let (ids, bg) = export_targets(doc, node.as_deref(), page.as_deref())?;
                render::export_svg(doc, &ids, bg, render::AssetMode::Url)
                    .map(|(s, _)| s.replace("href=\"/api/assets/", &format!("href=\"{base}/api/assets/")))
                    .ok_or_else(|| "nothing to export".to_string())
            })
            .await
            .map_err(err_msg)??;
        if svg.len() > 400_000 {
            return Ok(text_result(format!(
                "SVG is {} bytes; too large to return inline. Export a smaller node, or download {}/api/files/{fid}/export.svg in a browser.",
                svg.len(),
                self.base
            )));
        }
        Ok(text_result(svg))
    }

    async fn share_file(&self, args: &Value) -> ToolResult {
        let fid = s(args, "file_id")?.to_string();
        let enabled = args.get("enabled").and_then(Value::as_bool).unwrap_or(true);
        let days = args.get("expires_in_days").and_then(Value::as_u64).map(|d| d.min(3650) as u32);
        let uid = self.user.id.clone();
        let f2 = fid.clone();
        let (old, row) = self
            .st
            .db
            .call(move |c| crate::api_share::set_share(c, &uid, &f2, enabled, days, false))
            .await
            .map_err(err_msg)?;
        if let Some(t) = old {
            self.st.rooms.kick_share_if_open(&fid, &t).await;
        }
        Ok(text_result(match row {
            Some((token, _, _, expires)) => format!(
                "Anyone with this link can view the file live (no account needed){}: {}",
                expires.map(|e| format!(", until {}", ts(e))).unwrap_or_default(),
                crate::api_share::share_url(&self.base, &token)
            ),
            None => "Public link turned off.".to_string(),
        }))
    }

    /// A warning for fonts used in a create/update call that the workspace
    /// doesn't have (people would see Inter instead).
    async fn font_note(&self, file_id: &str, args: &Value) -> String {
        fn collect(v: &Value, out: &mut std::collections::BTreeSet<String>) {
            match v {
                Value::Object(m) => {
                    for (k, v) in m {
                        match (k.as_str(), v.as_str()) {
                            ("font" | "fontFamily", Some(f)) => {
                                out.insert(f.trim().to_string());
                            }
                            _ => collect(v, out),
                        }
                    }
                }
                Value::Array(a) => a.iter().for_each(|v| collect(v, out)),
                _ => {}
            }
        }
        let mut used = std::collections::BTreeSet::new();
        collect(args, &mut used);
        let known = |f: &str| crate::fonts::BUILTIN.iter().chain(crate::fonts::WEB_SAFE).any(|b| b.eq_ignore_ascii_case(f));
        used.retain(|f| !f.is_empty() && !known(f));
        if used.is_empty() {
            return String::new();
        }
        let fid = file_id.to_string();
        let available = self
            .st
            .db
            .call(move |c| crate::fonts::families(c, &crate::fonts::file_workspace(c, &fid)?))
            .await
            .unwrap_or_default();
        let missing: Vec<String> = used.into_iter().filter(|f| !available.iter().any(|a| a.eq_ignore_ascii_case(f))).collect();
        if missing.is_empty() {
            return String::new();
        }
        format!(
            "\n\nNote: {} isn't in this workspace's fonts, so most people (and exports) will see Inter instead. Add it from Google Fonts with add_font, upload the user's font files with upload_font, or pick one from list_fonts.",
            missing.join(", ")
        )
    }

    async fn list_fonts(&self, args: &Value) -> ToolResult {
        let fid = s(args, "file_id")?.to_string();
        let uid = self.user.id.clone();
        let rows = self
            .st
            .db
            .call(move |c| {
                let (ws, _, _) = file_access(c, &uid, &fid, Role::Viewer)?;
                crate::fonts::list_json(c, &ws)
            })
            .await
            .map_err(err_msg)?;
        let mut families: Vec<(String, String, Vec<String>)> = Vec::new();
        for r in rows {
            let family = r["family"].as_str().unwrap_or_default().to_string();
            let style = format!("{}{}", r["weight"], if r["italic"].as_bool().unwrap_or(false) { "i" } else { "" });
            match families.iter_mut().find(|(f, _, _)| *f == family) {
                Some((_, _, styles)) => styles.push(style),
                None => families.push((family, r["source"].as_str().unwrap_or_default().to_string(), vec![style])),
            }
        }
        let mut out = String::from("Workspace fonts:\n");
        if families.is_empty() {
            out.push_str("- (none yet)\n");
        }
        for (f, source, styles) in &families {
            let from = if source == "google" { "Google Fonts" } else { "uploaded" };
            out.push_str(&format!("- {f} ({from}): {}\n", styles.join(", ")));
        }
        out.push_str(
            "Always available: Inter (100–900, italics).\n\
             Common system fonts (Arial, Georgia, …) work on most computers but render as Inter in exports.\n\
             To use any other Google Fonts family, call add_font first; for the user's own font files, use upload_font.",
        );
        Ok(text_result(&out))
    }

    async fn add_font(&self, args: &Value) -> ToolResult {
        let fid = s(args, "file_id")?.to_string();
        let family = s(args, "family")?.to_string();
        let uid = self.user.id.clone();
        let ws = self
            .st
            .db
            .call(move |c| Ok(file_access(c, &uid, &fid, Role::Editor)?.0))
            .await
            .map_err(err_msg)?;
        let added = crate::api_fonts::add_google_family(&self.st, &ws, &self.user.id, &family).await.map_err(err_msg)?;
        let name = added["family"].as_str().unwrap_or(&family).to_string();
        let styles: Vec<&str> = added["styles"].as_array().map(|a| a.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
        Ok(text_result(&format!(
            "Added {name} to the workspace ({} {}: {}). Use it with fontFamily: \"{name}\" and a matching fontWeight.",
            styles.len(),
            if styles.len() == 1 { "style" } else { "styles" },
            styles.join(", ")
        )))
    }

    async fn upload_font(&self, args: &Value) -> ToolResult {
        let fid = s(args, "file_id")?.to_string();
        let max = crate::uploads::MAX_FILES;
        let files = args.get("files").and_then(Value::as_u64).unwrap_or(1).clamp(1, max as u64) as u32;
        let uid = self.user.id.clone();
        let ws = self
            .st
            .db
            .call(move |c| Ok(file_access(c, &uid, &fid, Role::Editor)?.0))
            .await
            .map_err(err_msg)?;
        let token = crate::uploads::create(crate::uploads::Kind::Font, &ws, &self.user.id, files);
        let url = format!("{}/api/font-uploads/{token}", self.base);
        Ok(text_result(format!(
            "Upload link ready. It works for {files} font {} over the next 15 minutes. Run this once per file:\n\n\
             curl -sS --data-binary @\"/path/to/Font-Regular.ttf\" {url}\n\n\
             Each upload prints the family, weight and style it added (and how many uploads are left); use that family name as fontFamily. \
             One style per file, .ttf or .otf (WOFF/WOFF2 and .ttc collections aren't accepted). \
             A file with the same family, weight and style replaces the existing one. \
             Only upload fonts the user is licensed to use on the web. Don't share this link.",
            if files == 1 { "file" } else { "files" }
        )))
    }

    /// Give `{type:"image"}` specs without a size the image's own proportions
    /// (longest side at most 1200 px), and check their assets exist here.
    async fn size_images(&self, file_id: &str, specs: &mut [Value]) -> Result<(), String> {
        fn collect<'a>(specs: &'a mut [Value], out: &mut Vec<&'a mut serde_json::Map<String, Value>>) {
            for s in specs {
                let Some(o) = s.as_object_mut() else { continue };
                let is_image = o.get("type").and_then(Value::as_str).map(flexma_doc::friendly::is_image_type).unwrap_or(false);
                if is_image {
                    out.push(o);
                } else if let Some(Value::Array(children)) = o.get_mut("children") {
                    collect(children, out);
                }
            }
        }
        let mut images = Vec::new();
        collect(specs, &mut images);
        if images.is_empty() {
            return Ok(());
        }
        let ids: Vec<String> = images
            .iter()
            .filter_map(|o| ["asset", "src", "image"].iter().find_map(|k| o.get(*k).and_then(Value::as_str)).map(str::to_string))
            .collect();
        let fid = file_id.to_string();
        let sizes: HashMap<String, (f64, f64)> = self
            .st
            .db
            .call(move |c| {
                let ws = crate::fonts::file_workspace(c, &fid)?;
                let mut out = HashMap::new();
                for id in ids {
                    let row: Option<(i64, i64)> = c
                        .query_row("SELECT width, height FROM assets WHERE id = ?1 AND workspace_id = ?2", params![id, ws], |r| {
                            Ok((r.get::<_, Option<i64>>(0)?.unwrap_or(0), r.get::<_, Option<i64>>(1)?.unwrap_or(0)))
                        })
                        .optional()?;
                    if let Some((w, h)) = row {
                        out.insert(id, (w as f64, h as f64));
                    }
                }
                Ok(out)
            })
            .await
            .map_err(err_msg)?;
        for o in images {
            let id = ["asset", "src", "image"].iter().find_map(|k| o.get(*k).and_then(Value::as_str)).unwrap_or_default().to_string();
            let &(w, h) = sizes.get(&id).ok_or_else(|| format!("image asset '{id}' isn't in this workspace; upload it with upload_image or pick one from list_images"))?;
            if w <= 0.0 || h <= 0.0 {
                continue;
            }
            let num = |k: &str, alt: &str| o.get(k).or_else(|| o.get(alt)).and_then(Value::as_f64);
            match (num("width", "w"), num("height", "h")) {
                (None, None) => {
                    let s = (1200.0_f64 / w.max(h)).min(1.0);
                    o.insert("width".into(), json!((w * s).round()));
                    o.insert("height".into(), json!((h * s).round()));
                }
                (Some(width), None) if !o.contains_key("height") && !o.contains_key("h") => {
                    o.insert("height".into(), json!((width * h / w).round()));
                }
                (None, Some(height)) if !o.contains_key("width") && !o.contains_key("w") => {
                    o.insert("width".into(), json!((height * w / h).round()));
                }
                _ => {}
            }
        }
        Ok(())
    }

    async fn upload_image(&self, args: &Value) -> ToolResult {
        let fid = s(args, "file_id")?.to_string();
        let max = crate::uploads::MAX_FILES;
        let files = args.get("files").and_then(Value::as_u64).unwrap_or(1).clamp(1, max as u64) as u32;
        let (uid, f2) = (self.user.id.clone(), fid.clone());
        let ws = self
            .st
            .db
            .call(move |c| Ok(file_access(c, &uid, &f2, Role::Editor)?.0))
            .await
            .map_err(err_msg)?;
        let token = crate::uploads::create(crate::uploads::Kind::Image { file_id: fid }, &ws, &self.user.id, files);
        let url = format!("{}/api/image-uploads/{token}", self.base);
        let mb = self.st.cfg.max_upload_bytes / (1024 * 1024);
        Ok(text_result(format!(
            "Upload link ready. It works for {files} {} over the next 15 minutes. Run this once per image:\n\n\
             curl -sS --data-binary @\"/path/to/photo.jpg\" {url}\n\n\
             Each upload prints the new asset id, its pixel size and a ready-made node like {{\"type\":\"image\",\"asset\":\"<id>\"}}. \
             Put that node in create_nodes (leave out width/height to use the image's proportions), or use the asset as a fill on any frame or shape: \
             {{\"type\":\"IMAGE\",\"asset\":\"<id>\",\"fit\":\"FILL\"}} (FILL crops to cover, FIT letterboxes, STRETCH distorts). \
             PNG, JPEG, GIF or WebP, up to {mb} MB each. For a picture on the web, download it first (curl -L -o photo.jpg <url>), then upload it. \
             Only use images the user has the rights to. Don't share this link.",
            if files == 1 { "image" } else { "images" }
        )))
    }

    async fn list_images(&self, args: &Value) -> ToolResult {
        let fid = s(args, "file_id")?.to_string();
        let room = self.room(&fid, Role::Viewer).await?;
        let used: std::collections::HashSet<String> = room
            .read(|doc, _| {
                doc.nodes()
                    .flat_map(|n| n.fills.iter().chain(n.strokes.iter()))
                    .filter_map(|p| match p {
                        flexma_doc::Paint::Image { asset, .. } => Some(asset.clone()),
                        _ => None,
                    })
                    .collect()
            })
            .await
            .map_err(err_msg)?;
        let f2 = fid.clone();
        let rows: Vec<(String, i64, i64, String, i64, Option<String>, i64)> = self
            .st
            .db
            .call(move |c| {
                let ws = crate::fonts::file_workspace(c, &f2)?;
                let mut stmt = c.prepare(
                    "SELECT a.id, COALESCE(a.width, 0), COALESCE(a.height, 0), a.mime, a.size, u.name, a.created_at
                     FROM assets a LEFT JOIN users u ON u.id = a.created_by
                     WHERE a.workspace_id = ?1 ORDER BY a.created_at DESC LIMIT 50",
                )?;
                let rows = stmt
                    .query_map(params![ws], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?)))?
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(rows)
            })
            .await
            .map_err(err_msg)?;
        if rows.is_empty() {
            return Ok(text_result("No images in this workspace yet. Add some with upload_image."));
        }
        let mut out = String::from("Images in this workspace (newest first):\n");
        for (id, w, h, mime, size, by, at) in rows {
            let kind = mime.trim_start_matches("image/").to_uppercase();
            out.push_str(&format!(
                "- {id}: {w}×{h} {kind}, {} KB, uploaded{} {}{}\n",
                (size + 1023) / 1024,
                by.map(|b| format!(" by {b}")).unwrap_or_default(),
                ts(at),
                if used.contains(&id) { " (used in this file)" } else { "" }
            ));
        }
        out.push_str("Place one with create_nodes {type:'image', asset:'<id>'}.");
        Ok(text_result(out))
    }

    async fn undo_agent_changes(&self, args: &Value) -> ToolResult {
        let fid = s(args, "file_id")?.to_string();
        let steps = args.get("steps").and_then(Value::as_u64).unwrap_or(1).clamp(1, 30) as usize;
        let room = self.room(&fid, Role::Editor).await?;
        room.undo_agent(self.agent(), steps)
            .await
            .map_err(err_msg)?
            .map(|v| text_result(v.as_str().unwrap_or_default()))
    }

    async fn list_components(&self, args: &Value) -> ToolResult {
        let fid = s(args, "file_id")?.to_string();
        let room = self.room(&fid, Role::Viewer).await?;
        let out = room
            .read(|doc, _| {
                let mut out = String::new();
                doc.walk(ROOT_ID, &mut |n| {
                    if n.kind == NodeType::Component {
                        let page = doc.page_of(&n.id).and_then(|p| doc.get(&p)).map(|p| p.name.clone()).unwrap_or_default();
                        let count = flexma_doc::components::instances_of(doc, &n.id).len();
                        out.push_str(&format!(
                            "COMPONENT \"{}\" [{}] {}×{} on page \"{}\" — {} instance(s){}\n",
                            n.name,
                            n.id,
                            fmt_num(n.width),
                            fmt_num(n.height),
                            page,
                            count,
                            if n.description.is_empty() { String::new() } else { format!(" — {}", n.description) }
                        ));
                        for c in doc.children(&n.id) {
                            if let Some(cn) = doc.get(c) {
                                out.push_str(&format!("    layer {} \"{}\" [{}]\n", cn.kind.as_str(), cn.name, cn.id));
                            }
                        }
                    }
                });
                if out.is_empty() {
                    "No components yet. Make one with create_component (from a frame) or create_nodes {type:'component', …}.".to_string()
                } else {
                    out
                }
            })
            .await
            .map_err(err_msg)?;
        Ok(text_result(out))
    }

    async fn create_component(&self, args: &Value) -> ToolResult {
        let fid = s(args, "file_id")?.to_string();
        let node = s(args, "node_id")?.to_string();
        let name = opt_s(args, "name").map(str::to_string);
        let room = self.room(&fid, Role::Editor).await?;
        let new_id = format!("a{}:1", random_string(6));
        room.mutate(self.agent(), move |tx| {
            let mut ops = flexma_doc::components::convert_to_component_ops(tx.doc, &node, &new_id)?;
            if let (Some(name), Some(Op::Create { node: c })) = (name, ops.first_mut()) {
                c.name = name;
            }
            let errs = tx.apply_all(ops);
            if !errs.is_empty() {
                return Err(format!("{errs:?}"));
            }
            tx.relayout();
            let name = tx.doc.get(&new_id).map(|n| n.name.clone()).unwrap_or_default();
            Ok(json!(format!("Created component \"{name}\" [{new_id}] (replaces {node}). Place it with create_nodes {{type:'instance', component:'{name}'}}.")))
        })
        .await
        .map_err(err_msg)?
        .map(|v| text_result(v.as_str().unwrap_or_default()))
    }

    async fn detach_instance(&self, args: &Value) -> ToolResult {
        let fid = s(args, "file_id")?.to_string();
        let node = s(args, "node_id")?.to_string();
        let room = self.room(&fid, Role::Editor).await?;
        let prefix = format!("a{}", random_string(5));
        room.mutate(self.agent(), move |tx| {
            let mut n = 0u32;
            let mut gen = || {
                n += 1;
                format!("{prefix}:{n}")
            };
            let (ops, frame) = flexma_doc::components::detach_ops(tx.doc, &node, &mut gen)?;
            let errs = tx.apply_all(ops);
            if !errs.is_empty() {
                return Err(format!("{errs:?}"));
            }
            tx.relayout();
            Ok(json!(format!("Detached into frame [{frame}].")))
        })
        .await
        .map_err(err_msg)?
        .map(|v| text_result(v.as_str().unwrap_or_default()))
    }

    async fn list_comments(&self, args: &Value) -> ToolResult {
        let fid = s(args, "file_id")?.to_string();
        let include = args.get("include_resolved").and_then(Value::as_bool).unwrap_or(false);
        let room = self.room(&fid, Role::Viewer).await?;
        let f2 = fid.clone();
        let threads = self
            .st
            .db
            .call(move |c| crate::api_comments::load_threads(c, &f2, include))
            .await
            .map_err(err_msg)?;
        if threads.is_empty() {
            return Ok(text_result(if include { "No comments on this file." } else { "No open comments on this file." }));
        }
        let text = room
            .read(move |doc, _| {
                let mut out = String::new();
                for t in &threads {
                    let at = match t["nodeId"].as_str().and_then(|n| doc.get(n)) {
                        Some(n) => format!("on {} \"{}\" [{}]", n.kind.as_str(), n.name, n.id),
                        None => format!("at {},{}", fmt_num(t["x"].as_f64().unwrap_or(0.0)), fmt_num(t["y"].as_f64().unwrap_or(0.0))),
                    };
                    let page = t["pageId"].as_str().and_then(|p| doc.get(p)).map(|p| p.name.clone()).unwrap_or_default();
                    out.push_str(&format!(
                        "Thread {} {}{} — page \"{}\" {}\n  {}: {}\n",
                        t["id"].as_str().unwrap_or_default(),
                        if t["resolved"].as_bool().unwrap_or(false) { "(resolved) " } else { "" },
                        if t["author"]["isAgent"].as_bool().unwrap_or(false) { "[agent]" } else { "" },
                        page,
                        at,
                        t["author"]["name"].as_str().unwrap_or_default(),
                        t["body"].as_str().unwrap_or_default()
                    ));
                    for r in t["replies"].as_array().into_iter().flatten() {
                        out.push_str(&format!("    ↳ {}: {}\n", r["author"]["name"].as_str().unwrap_or_default(), r["body"].as_str().unwrap_or_default()));
                    }
                }
                out
            })
            .await
            .map_err(err_msg)?;
        Ok(text_result(text))
    }

    async fn add_comment(&self, args: &Value) -> ToolResult {
        let fid = s(args, "file_id")?.to_string();
        let body = s(args, "body")?.to_string();
        let room = self.room(&fid, Role::Viewer).await?;
        let node = opt_s(args, "node_id").map(str::to_string);
        let reply_to = opt_s(args, "reply_to").map(str::to_string);
        let page_arg = opt_s(args, "page_id").map(str::to_string);
        let (ax, ay) = (args.get("x").and_then(Value::as_f64), args.get("y").and_then(Value::as_f64));
        // Resolve position: node's top-right corner, or explicit x/y.
        let (page, x, y) = room
            .read(move |doc, _| -> Result<(Option<String>, f64, f64), String> {
                if let Some(n) = &node {
                    let b = doc.world_bounds(n);
                    if b.is_empty() {
                        return Err(format!("node '{n}' not found"));
                    }
                    // Node-attached pins are stored relative to the node's top-left: put it on the top-right corner.
                    return Ok((doc.page_of(n), b.width, 0.0));
                }
                let page = page_arg.or_else(|| doc.pages().first().cloned());
                Ok((page, ax.unwrap_or(0.0), ay.unwrap_or(0.0)))
            })
            .await
            .map_err(err_msg)??;
        let agent = self.agent();
        let author = crate::api_comments::Author { user_id: self.user.id.clone(), name: agent.name.clone(), color: agent.color.clone(), is_agent: true };
        let f2 = fid.clone();
        let node2 = opt_s(args, "node_id").map(str::to_string);
        let id = self
            .st
            .db
            .call(move |c| {
                crate::api_comments::insert_comment(
                    c,
                    &f2,
                    &author,
                    crate::api_comments::NewComment { body, parent_id: reply_to, page_id: page, node_id: node2, x, y },
                )
            })
            .await
            .map_err(err_msg)?;
        room.broadcast(json!({ "t": "comments" })).await;
        Ok(text_result(format!("Comment {id} posted. Collaborators see it on the canvas.")))
    }

    async fn resolve_comment(&self, args: &Value) -> ToolResult {
        let fid = s(args, "file_id")?.to_string();
        let cid = s(args, "comment_id")?.to_string();
        let resolved = args.get("resolved").and_then(Value::as_bool).unwrap_or(true);
        let room = self.room(&fid, Role::Editor).await?;
        let f2 = fid.clone();
        let n = self
            .st
            .db
            .call(move |c| {
                Ok(c.execute(
                    "UPDATE comments SET resolved = ?1, updated_at = ?2 WHERE id = ?3 AND file_id = ?4",
                    params![resolved as i64, now(), cid, f2],
                )?)
            })
            .await
            .map_err(err_msg)?;
        if n == 0 {
            return Err("comment not found".into());
        }
        room.broadcast(json!({ "t": "comments" })).await;
        Ok(text_result(if resolved { "Thread resolved." } else { "Thread reopened." }))
    }

    async fn save_version(&self, args: &Value) -> ToolResult {
        let fid = s(args, "file_id")?.to_string();
        let label: String = s(args, "label")?.trim().chars().take(120).collect();
        let room = self.room(&fid, Role::Editor).await?;
        room.flush().await.map_err(err_msg)?;
        let agent = self.agent();
        let (uid, l2, f2) = (self.user.id.clone(), format!("{label} ({})", agent.name.split(" · ").next().unwrap_or("agent")), fid.clone());
        self.st
            .db
            .call(move |c| {
                c.execute(
                    "INSERT INTO file_versions (id, file_id, label, document, version, created_by, created_at)
                     SELECT ?1, id, ?2, document, version, ?3, ?4 FROM files WHERE id = ?5",
                    params![new_id(), l2, uid, now(), f2],
                )?;
                Ok(())
            })
            .await
            .map_err(err_msg)?;
        Ok(text_result(format!("Saved version \"{label}\". People can restore it from Main menu → Version history.")))
    }

    async fn get_selection(&self, args: &Value) -> ToolResult {
        let fid = s(args, "file_id")?.to_string();
        let room = self.room(&fid, Role::Viewer).await?;
        let out = room
            .read(|doc, peers| {
                let mut out = String::new();
                let humans: Vec<_> = peers.iter().filter(|p| !p.is_agent).collect();
                if humans.is_empty() {
                    return "Nobody has this file open right now, so there is no live selection.".to_string();
                }
                for p in humans {
                    let page = p.page.as_deref().and_then(|pg| doc.get(pg)).map(|n| n.name.clone()).unwrap_or_default();
                    out.push_str(&format!("{} (page \"{}\"): ", p.name, page));
                    if p.selection.is_empty() {
                        out.push_str("nothing selected\n");
                    } else {
                        out.push('\n');
                        for id in &p.selection {
                            if let Some(n) = doc.get(id) {
                                out.push_str("  ");
                                out.push_str(&node_line(doc, n));
                                out.push('\n');
                            }
                        }
                    }
                }
                out
            })
            .await
            .map_err(err_msg)?;
        Ok(text_result(out))
    }
}

/// When creating directly on a page without coordinates, place new content to
/// the right of what's already there so agents never stack on top of work.
fn place_on_page(doc: &Document, parent: &str, specs: &mut [Value]) {
    if doc.get(parent).map(|n| n.kind) != Some(NodeType::Page) {
        return;
    }
    let mut bounds = Rect::empty();
    for c in doc.children(parent) {
        bounds = bounds.union(&doc.subtree_bounds(c));
    }
    let mut x = if bounds.is_empty() { 0.0 } else { (bounds.right() + 100.0).round() };
    let y = if bounds.is_empty() { 0.0 } else { bounds.y.round() };
    for spec in specs.iter_mut() {
        let Some(o) = spec.as_object_mut() else { continue };
        if o.contains_key("x") || o.contains_key("y") || o.contains_key("x1") {
            continue;
        }
        o.insert("x".into(), json!(x));
        o.insert("y".into(), json!(y));
        let w = o.get("width").or_else(|| o.get("w")).and_then(Value::as_f64).unwrap_or(400.0);
        x += w + 100.0;
    }
}

/// After text edits, re-estimate the box (the first browser that renders it
/// will measure it exactly and clear `needsMeasure`).
fn re_estimate_text(node: &Node, props: &mut Map<String, Value>) {
    const AFFECTS: &[&str] = &[
        "characters", "fontSize", "fontFamily", "fontWeight", "letterSpacing", "lineHeight", "textAutoResize", "width",
    ];
    if !AFFECTS.iter().any(|k| props.contains_key(*k)) {
        return;
    }
    let mut n = node.clone();
    for (k, v) in props.iter() {
        let _ = n.set_prop(k, v);
    }
    let (_, w, h) = flexma_doc::text::estimate_layout(&n);
    if n.text_auto_resize == flexma_doc::TextAutoResize::WidthAndHeight && !props.contains_key("width") {
        props.insert("width".into(), json!(w));
    }
    if n.text_auto_resize != flexma_doc::TextAutoResize::None {
        props.insert("height".into(), json!(h));
    }
    props.insert("needsMeasure".into(), json!(true));
}

fn first_fill_hex(n: &Node) -> Option<String> {
    n.fills.iter().find(|p| p.is_visible()).map(|p| match p {
        flexma_doc::Paint::Solid { color, .. } => color.to_hex(),
        flexma_doc::Paint::LinearGradient { .. } => "gradient".into(),
        flexma_doc::Paint::Image { asset, .. } => format!("image:{asset}"),
    })
}

fn node_line(doc: &Document, n: &Node) -> String {
    let mut s = format!("{} \"{}\" [{}]", n.kind.as_str(), n.name.replace('"', "'"), n.id);
    if n.kind == NodeType::Instance {
        let comp = n.component_id.as_deref().and_then(|c| doc.get(c)).map(|c| c.name.clone()).unwrap_or_else(|| "missing component".into());
        s.push_str(&format!(" of \"{comp}\""));
        let count = n.overrides.len();
        if count > 0 && !flexma_doc::node::is_derived_id(&n.id) {
            s.push_str(&format!(" ({count} override(s))"));
        }
    }
    if n.kind.is_scene() {
        s.push_str(&format!(" @{},{} {}×{}", fmt_num(n.x), fmt_num(n.y), fmt_num(n.width), fmt_num(n.height)));
    }
    if n.rotation != 0.0 {
        s.push_str(&format!(" rot {}°", fmt_num(n.rotation)));
    }
    if let Some(f) = first_fill_hex(n) {
        if n.kind != NodeType::Page {
            s.push_str(&format!(" fill {f}"));
        }
    }
    if n.has_visible_stroke() {
        s.push_str(&format!(" stroke {}px", fmt_num(n.stroke_weight)));
    }
    if n.corner_radius > 0.0 || n.corner_radii.is_some() {
        s.push_str(&format!(" r {}", fmt_num(n.corner_radius)));
    }
    if n.layout_mode != flexma_doc::LayoutMode::None {
        s.push_str(&format!(
            " layout {:?} gap {} pad {}",
            n.layout_mode,
            fmt_num(n.item_spacing),
            fmt_num(n.padding.top)
        ));
    }
    if n.kind == NodeType::Text {
        let t: String = n.characters.chars().take(80).collect();
        s.push_str(&format!(" {}px w{} \"{}\"", fmt_num(n.font_size), n.font_weight, t.replace('\n', "\\n")));
    }
    if !n.visible {
        s.push_str(" (hidden)");
    }
    if n.locked {
        s.push_str(" (locked)");
    }
    s
}

fn outline(doc: &Document, id: &str, level: usize, max_depth: usize, out: &mut String) {
    let Some(n) = doc.get(id) else { return };
    out.push_str(&"  ".repeat(level));
    out.push_str(&node_line(doc, n));
    out.push('\n');
    let kids = doc.children(id);
    if kids.is_empty() {
        return;
    }
    if level + 1 >= max_depth {
        out.push_str(&"  ".repeat(level + 1));
        out.push_str(&format!("… {} children (increase depth or use node_id)\n", kids.len()));
        return;
    }
    // Front-most last in storage; list back-to-front like a layers panel reversed.
    for c in kids {
        outline(doc, c, level + 1, max_depth, out);
    }
}
