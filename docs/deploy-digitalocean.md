# Deploy Flexma on a DigitalOcean droplet

This guide takes you from an empty droplet to `https://design.yourdomain.com` with HTTPS, multiplayer, and an MCP endpoint that Claude Code and Codex can sign in to from anywhere, the same way Figma's hosted MCP works.

## 1. Create the droplet

- **Image:** Ubuntu 24.04 LTS.
- **Size:** 2 GB RAM / 1 vCPU is enough to *run* Flexma for a small team.
  - *Building* the image compiles Rust with full optimizations, which wants about 3 GB of memory. Either pick a 4 GB droplet, or add swap on a smaller one (see below).
  - You can also build the image elsewhere and push it to a registry.
- **Networking:** allow SSH (22), HTTP (80) and HTTPS (443).

On droplets with 1–2 GB of RAM, add swap before building:

```bash
sudo fallocate -l 4G /swapfile && sudo chmod 600 /swapfile && sudo mkswap /swapfile && sudo swapon /swapfile
echo '/swapfile none swap sw 0 0' | sudo tee -a /etc/fstab
```

## 2. Point your domain at it

At your DNS provider (or DigitalOcean → Networking → Domains), create an **A record**:

| Name | Value |
|---|---|
| `design` | your droplet's IPv4 address |

Wait until `ping design.yourdomain.com` resolves to the droplet.

## 3. Install Docker

```bash
curl -fsSL https://get.docker.com | sh
```

## 4. Get Flexma and configure it

```bash
git clone <your flexma repo url> flexma && cd flexma
cp .env.example .env
nano .env          # set FLEXMA_DOMAIN=design.yourdomain.com
```

## 5. Start it

```bash
docker compose up -d --build
```

The first build takes a while (Rust plus the web app). When it finishes:

- Caddy gets a Let's Encrypt certificate automatically, and HTTP redirects to HTTPS.
- Open `https://design.yourdomain.com`. The first visit shows **setup**, where you create the admin account and your first workspace.
- Invite teammates from **Workspace settings → Invite people**. The invite link is the only way in unless you set `FLEXMA_ALLOW_SIGNUP=true`.

## 6. Connect Claude Code and Codex (remote MCP)

On your laptop (any machine), connect Claude Code:

```bash
claude mcp add --scope user --transport http flexma https://design.yourdomain.com/mcp
```

Then run `/mcp` in Claude Code and choose *Authenticate*. Your browser opens your Flexma sign-in page; approve access and you're done.

Connect Codex:

```bash
codex mcp add flexma --url https://design.yourdomain.com/mcp
codex mcp login flexma
```

For non-interactive `codex exec` runs, also add `default_tools_approval_mode = "approve"` under `[mcp_servers.flexma]` in `~/.codex/config.toml`. Otherwise Codex asks before every Flexma tool call.

Install the Flexma skill so your agents know how to design well with it:

```bash
curl -fsSL https://design.yourdomain.com/agents/install.sh | sh
```

Connections are unlimited, so every teammate and every agent session can connect at once. Each sign-in shows up under **AI Agents** as an `(OAuth)` token that you can revoke. For CI or headless agents, create a plain API token there and send it as `Authorization: Bearer <token>`.

## 7. Operations

**Logs:**

```bash
docker compose logs -f flexma
```

**Update:**

```bash
git pull && docker compose up -d --build
```

**Back up.** Everything lives in the `flexma-data` volume (a SQLite database plus uploaded images):

```bash
docker compose stop flexma
docker run --rm -v flexma_flexma-data:/data -v "$PWD":/backup alpine tar czf /backup/flexma-$(date +%F).tgz -C / data
docker compose start flexma
```

**Reset a password:**

```bash
docker compose exec -it flexma flexma reset-password you@example.com
```

**Firewall (optional):**

```bash
ufw allow OpenSSH && ufw allow 80 && ufw allow 443 && ufw enable
```

## Without Docker

Flexma is a single binary plus a static web folder. Build it with:

```bash
./scripts/build-wasm.sh && (cd web && npm ci && npm run build) && cargo build --release -p flexma-server
```

Then run `FLEXMA_PUBLIC_URL=https://… FLEXMA_WEB_DIR=web/dist FLEXMA_DATA_DIR=/var/lib/flexma ./target/release/flexma` behind any reverse proxy that supports WebSockets (Caddy, nginx, Traefik).

The proxy must pass `Host` (or `X-Forwarded-Host`) and `X-Forwarded-Proto`, or you must set `FLEXMA_PUBLIC_URL`. The OAuth and MCP URLs are built from these.
