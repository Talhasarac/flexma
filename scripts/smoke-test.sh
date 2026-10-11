#!/usr/bin/env bash
# End-to-end smoke test against a running Flexma server.
#
#   scripts/smoke-test.sh https://design.example.com you@example.com 'your password'
#
# Signs in, creates an API token, exercises the MCP endpoint (initialize,
# tools/list, create a file + nodes, render a screenshot), checks OAuth
# discovery, then revokes the token. Requires curl and python3.
set -euo pipefail

BASE="${1:?usage: smoke-test.sh BASE_URL EMAIL PASSWORD}"
EMAIL="${2:?email}"
PASSWORD="${3:?password}"
BASE="${BASE%/}"
JAR="$(mktemp)"
trap 'rm -f "$JAR"' EXIT
H_JSON='Content-Type: application/json'
H_ORIGIN="Origin: $BASE"

ok() { printf '  \033[32m✓\033[0m %s\n' "$1"; }
fail() { printf '  \033[31m✗\033[0m %s\n' "$1"; exit 1; }
json() { python3 -c "import sys,json; d=json.load(sys.stdin); print($1)"; }

echo "Flexma smoke test → $BASE"

curl -fsS "$BASE/api/health" >/dev/null && ok "server healthy" || fail "server not reachable"

curl -fsS -c "$JAR" -H "$H_JSON" -H "$H_ORIGIN" \
  -d "{\"email\":\"$EMAIL\",\"password\":\"$PASSWORD\"}" "$BASE/api/auth/login" >/dev/null \
  && ok "signed in as $EMAIL" || fail "login failed"

TOKEN_JSON=$(curl -fsS -b "$JAR" -H "$H_JSON" -H "$H_ORIGIN" -d '{"name":"smoke test"}' "$BASE/api/tokens")
TOKEN=$(echo "$TOKEN_JSON" | json 'd["token"]')
TOKEN_ID=$(echo "$TOKEN_JSON" | json 'd["id"]')
ok "created API token"

mcp() {
  curl -fsS -H "Authorization: Bearer $TOKEN" -H "$H_JSON" -d "$1" "$BASE/mcp"
}
call() {
  mcp "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/call\",\"params\":{\"name\":\"$1\",\"arguments\":$2}}"
}

mcp '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"smoke","version":"1"}}}' \
  | json 'd["result"]["serverInfo"]["name"]' | grep -q flexma && ok "MCP initialize" || fail "MCP initialize"

COUNT=$(mcp '{"jsonrpc":"2.0","id":2,"method":"tools/list"}' | json 'len(d["result"]["tools"])')
ok "MCP tools/list ($COUNT tools)"

PROJECT=$(call list_workspaces '{}' | json 'json.loads(d["result"]["content"][0]["text"])["workspaces"][0]["projects"][0]["id"]')
FILE_TEXT=$(call create_file "{\"project_id\":\"$PROJECT\",\"name\":\"Smoke test\"}" | json 'd["result"]["content"][0]["text"]')
FILE=$(echo "$FILE_TEXT" | sed -E 's/.*\(id ([^)]+)\).*/\1/')
ok "created file $FILE"

call create_nodes "{\"file_id\":\"$FILE\",\"nodes\":[{\"type\":\"frame\",\"name\":\"Smoke card\",\"layout\":\"vertical\",\"padding\":24,\"gap\":8,\"fill\":\"#ffffff\",\"radius\":12,\"children\":[{\"type\":\"text\",\"text\":\"Hello from the smoke test\",\"size\":20,\"weight\":700},{\"type\":\"rect\",\"w\":240,\"h\":80,\"fill\":\"#6d5dfc\",\"radius\":8}]}]}" \
  | json 'd["result"]["content"][0]["text"]' | grep -q "Created" && ok "MCP create_nodes" || fail "create_nodes"

call get_screenshot "{\"file_id\":\"$FILE\"}" | json 'd["result"]["content"][0]["type"]' | grep -q image \
  && ok "MCP get_screenshot (PNG render)" || fail "get_screenshot"

curl -fsS "$BASE/.well-known/oauth-protected-resource" | json 'd["resource"]' | grep -q "/mcp" \
  && ok "OAuth protected-resource metadata" || fail "OAuth metadata"
curl -fsS "$BASE/.well-known/oauth-authorization-server" | json 'd["registration_endpoint"]' | grep -q "/oauth/register" \
  && ok "OAuth authorization-server metadata" || fail "OAuth AS metadata"

curl -fsS -b "$JAR" -H "$H_ORIGIN" -X DELETE "$BASE/api/files/$FILE" >/dev/null && ok "deleted test file"
curl -fsS -b "$JAR" -H "$H_ORIGIN" -X DELETE "$BASE/api/tokens/$TOKEN_ID" >/dev/null && ok "revoked token"
echo "All good."
