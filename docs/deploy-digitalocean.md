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
- Open the setup link to create the admin account and your first workspace. It's in the log (`docker compose logs flexma | grep setup`) and looks like `https://design.yourdomain.com/setup?token=…`. The token is also saved in the data directory: `docker compose exec flexma cat /data/setup-token`. It stops strangers from claiming a fresh instance, survives restarts, and is deleted once the first account exists. To choose it yourself, set `FLEXMA_SETUP_TOKEN`.
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

**Back up.** Admins can download a full backup from **Admin → Backups**: one .zip with a consistent copy
of the database (accounts, workspaces, designs, version history, comments), uploaded images and fonts.
It's made while the server runs, from a signed-in browser only (API and agent tokens can't fetch it).

Restore it with the `flexma restore` command. Stop the server first; the data it replaces is moved to a
`pre-restore-*` folder, not deleted:

```bash
systemctl stop flexma
FLEXMA_DATA_DIR=/var/lib/flexma /opt/flexma/flexma restore /root/flexma-backup-….zip
systemctl start flexma
```

With Docker, copy the zip into the volume and run `docker compose run --rm flexma flexma restore /data/<file>.zip`.

Don't copy `flexma.db` on its own while the server runs: recent changes live in `flexma.db-wal` until
SQLite merges them. To snapshot from the shell, use `sqlite3 flexma.db ".backup copy.db"` and also copy
`assets/` and `fonts/`.

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

### As a systemd service next to other software (e.g. a VPN)

`deploy/systemd/` has a ready setup that doesn't touch the firewall or Docker's
iptables rules, so it can share a small droplet with something like OpenVPN:

- `flexma.service` runs Flexma as its own user on `127.0.0.1:8080`, capped at
  350 MB of memory and first in line for the OOM killer, so it can't starve
  the other services.
- `flexma.env.example` goes to `/etc/flexma/flexma.env`. Set `FLEXMA_PUBLIC_URL`, and
  keep `FLEXMA_MAX_EXPORT_PX=4096` on 1 GB servers.
- `Caddyfile` serves HTTPS on port **8443** for when something else already owns 443
  (OpenVPN Access Server does). The certificate comes over port 80, which must be free.
  Claude Code and Codex connect fine on 8443, but a claude.ai custom connector
  couldn't reach it. Use the Cloudflare Tunnel below for claude.ai.
- `cloudflared.yml` publishes Flexma on standard HTTPS through a Cloudflare Tunnel
  instead, without opening any port on the server.

#### Cloudflare Tunnel

You need a domain on Cloudflare. On the server:

```bash
curl -fsSL https://pkg.cloudflare.com/cloudflare-main.gpg -o /usr/share/keyrings/cloudflare-main.gpg
echo "deb [signed-by=/usr/share/keyrings/cloudflare-main.gpg] https://pkg.cloudflare.com/cloudflared any main" > /etc/apt/sources.list.d/cloudflared.list
apt-get update && apt-get install -y cloudflared
cloudflared tunnel login                     # open the printed link, pick your domain
cloudflared tunnel create flexma             # prints the tunnel id
cloudflared tunnel route dns flexma design.example.com
mkdir -p /etc/cloudflared && cp ~/.cloudflared/<tunnel id>.json /etc/cloudflared/
```

Copy `cloudflared.yml` to `/etc/cloudflared/config.yml`, fill in the tunnel id and
hostname, then run `cloudflared service install`. Set
`FLEXMA_PUBLIC_URL=https://design.example.com` in `/etc/flexma/flexma.env` and
`systemctl restart flexma`. WebSockets and MCP streaming pass through the tunnel as is.

You can build the Linux binary on a Mac instead of the server:

```bash
brew install zig && cargo install cargo-zigbuild && rustup target add x86_64-unknown-linux-gnu
cargo zigbuild --release -p flexma-server --target x86_64-unknown-linux-gnu.2.31
```

Then copy `target/x86_64-unknown-linux-gnu/release/flexma` to `/opt/flexma/flexma` and `web/dist/` to `/opt/flexma/web/`, and run
`systemctl enable --now flexma`. The setup token is in `/var/lib/flexma/setup-token`, so open `https://<your url>/setup?token=<that token>`.

