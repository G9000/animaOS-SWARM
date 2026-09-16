# One-owner companion on a VPS

This package builds the web UI and Rust daemon from this checkout. Caddy serves the UI over HTTPS, authenticates the owner, and proxies the API and streaming responses. The daemon listens only on loopback inside Caddy's network namespace. Only ports 80 and 443 are published.

The package is intended for one trusted owner, one daemon, and one VPS. It is not a multi-user service or a sandbox for hostile agent code. Anyone with the owner login can operate this workspace and its connected accounts. Do not scale either service or share these volumes between live daemons.

## Requirements and current verification

Use a Linux VPS with Docker Engine and Docker Compose v2.20 or newer, a domain pointed at its public IP, and inbound TCP 80/443 available. Keep SSH restricted separately. Allow outbound HTTPS for certificate issuance, model providers, and connectors. Building the Rust image is resource intensive; use a build machine if the VPS is too small. The build uses the locked dependencies; the Rust/Debian base tags receive updates, so record the resulting image IDs for rollback.

Both Docker images have built successfully on Linux containers. An isolated local smoke test verified daemon readiness, the actual Rust OS-keyring backend, workspace bootstrap/files, SQLite memory, authenticated gateway routing, and restoration of the agent, memory, workspace file, and a dummy credential after both a daemon restart and container removal/recreation with the same named volumes. The test published no ports and used no real credentials or model calls. Automated gateway checks also cover Origin/Fetch Metadata enforcement, exact OAuth callback exceptions, and SSE. Public TLS issuance, real provider/connector flows, VPS reboot, and off-site restore remain acceptance checks for your deployment. A healthy proxy alone does not establish a healthy daemon or a configured model.

## First installation

Clone or copy the repository to the VPS without local secrets, dependencies, or data. From the repository root:

```sh
cd deploy/vps
cp -n .env.example .env
chmod 600 .env
docker run --rm -it caddy:2.10.2-alpine caddy hash-password
openssl rand -hex 32
openssl rand -hex 32
openssl rand -hex 32
```

Edit `.env` with these values:

- `ANIMA_DOMAIN`: a bare hostname such as `companion.example.com`; no scheme, port, path, wildcard, or IP.
- `ACME_EMAIL`: your certificate contact email.
- `ANIMA_OWNER_USER`: one login name, using letters, numbers, or underscores.
- `ANIMA_OWNER_PASSWORD_HASH`: the Caddy hash, surrounded by **single quotes** so Compose preserves its `$` characters. Retain the password in your password manager.
- `ANIMA_INTERNAL_API_KEY`, `ANIMA_LOCAL_ADMIN_TOKEN`, `ANIMA_KEYRING_PASSWORD`: the three distinct random outputs. These are mandatory, with no public defaults. Keep the keyring password for the lifetime of the encrypted vault.
- At least one supported model API key, or connect ChatGPT from the UI after startup. The sample exposes OpenAI, Anthropic, and OpenRouter variables; other providers require adding their actual environment variables to the daemon service. No model provider key is sent to the gateway or included in the web build.

Do not paste these secrets into chat, commit `.env`, or print the fully interpolated Compose configuration. Docker administrators can inspect container environment variables; `.env` and VPS administrator access are part of the trust boundary. The keyring password is provided through the container environment and removed from the daemon child environment after unlocking.

```sh
docker compose config --quiet
docker compose build --pull
docker compose run --rm --no-deps gateway caddy validate --config /etc/caddy/Caddyfile --adapter caddyfile
docker compose up -d
docker compose ps
docker compose logs --tail=80 daemon
```

Open `https://YOUR_DOMAIN` and enter the owner login. Your browser remembers HTTP Basic authentication; use a private browser session on a shared device. There is no application-level logout or MFA in this package. Configure the companion and choose a real provider/model. The deterministic provider is a mock and does not call an AI service.

Never add an `8080` port mapping or change the daemon bind to `0.0.0.0`. The daemon's local-owner policy intentionally rejects non-loopback requests and forwarded headers. The gateway checks browser Origin and Fetch Metadata, requires a same-origin Origin on mutations and WebSocket upgrades, then translates the authenticated owner into separate internal API and admin credentials. Clients using the public API must use the owner Basic login and, on writes, `Origin: https://YOUR_DOMAIN`. Internal daemon keys do not bypass the gateway login.

The exact GET OAuth callback paths are allowed to return from another site; their one-time state is validated by the daemon. All callback routes still require the owner login. The UI and API share one origin, and SSE is forwarded without response buffering. See the [Caddy reverse proxy documentation](https://caddyserver.com/docs/caddyfile/directives/reverse_proxy).

## Persistence and credentials

| Named volume | Contents | Recovery requirement |
| --- | --- | --- |
| `anima-companion_daemon_state` | `control-plane.json` and `memory.sqlite`, including SQLite sidecars and embedding state | Back up the whole volume with the daemon stopped |
| `anima-companion_workspace` | Workspace files and assets under `/workspace` | Keep the same `/workspace` path on restore |
| `anima-companion_daemon_home` | Encrypted GNOME keyrings under `.local/share/keyrings`, plus daemon home files | Restore together with the original keyring password |
| `anima-companion_caddy_data` | TLS certificates, private keys, and ACME account state | Treat backup as sensitive |
| `anima-companion_caddy_config` | Caddy runtime configuration storage | Restore with the same deployment |

The explicit JSON/SQLite paths make state durable even though `ANIMAOS_RS_PERSISTENCE_MODE=memory` selects the non-Postgres adapter. Agent state/conversation snapshots, jobs, goals, schedules, workspace settings, and connector records are in the control-plane snapshot. Durable memory uses SQLite; this is separate from workspace files and credential storage.

Telegram bot tokens, ChatGPT access/refresh tokens, OAuth application secrets, and Google/Microsoft connector tokens use the daemon's existing OS-keyring backend. This image supplies a private D-Bus session and GNOME Secret Service, unlocks the persisted login keyring at startup, and verifies a write/read/delete probe before starting the daemon. A wrong password or unusable vault causes startup to fail instead of silently using transient credentials. See [GNOME keyring startup](https://wiki.gnome.org/Projects/GnomeKeyring/RunningDaemon) and the [daemon manual](https://manpages.debian.org/bookworm/gnome-keyring/gnome-keyring-daemon.1.en.html).

Never remove the home volume or replace `ANIMA_KEYRING_PASSWORD` to troubleshoot an existing vault. Changing that environment value does not re-encrypt the keyring. Losing the password or vault requires reconnecting accounts; restoring control-plane JSON alone does not restore their secrets. Keep a separate protected copy of `.env` in addition to encrypted off-site backups. Model API keys supplied by environment are in `.env`, not in the keyring.

Compose sets `ANIMA_PUBLIC_BASE_URL=https://YOUR_DOMAIN`. The daemon uses this same origin for the Google Calendar/Gmail/Outlook callback URLs shown in connector setup and sent to providers. Register the exact URLs shown in the UI in your Google/Microsoft OAuth application; application credentials and provider consent are still required. Without this setting, local development retains `http://127.0.0.1:8080`. Invalid origins fail startup; use HTTPS without a path, credentials, query, or fragment. Changing your domain also requires updating provider registrations and restarting the daemon. Telegram and ChatGPT device-code authentication do not use these redirect callbacks. Live provider consent and restart behavior still need verification on your deployment.

## Acceptance checks before relying on it

1. Confirm `docker compose ps` shows **both** services healthy. `/ready` is the daemon's readiness endpoint. Check `docker compose logs --tail=80 daemon` if startup fails; never share logs containing secrets.
2. From another machine, `curl -I https://YOUR_DOMAIN` should return `401` without credentials, and TCP port 8080 must be unreachable. Sign in and send a real message; confirm a streaming response from the selected provider.
3. Create a memory and a workspace file, and connect Telegram if needed. Run `docker compose restart daemon`; check the conversation, memory, file, and connector status after it becomes healthy. Send a fresh Telegram message and verify exactly one response. The vault probe alone verifies availability, not the successful persistence of each provider's credential.
4. Reboot the VPS during a quiet period, reconnect, and repeat a real message. `restart: unless-stopped` restarts running services after Docker returns; manually stopped services remain stopped. Health checks report failure but Docker does not automatically restart a process merely because it is unhealthy.
5. Perform a backup and restore rehearsal on a separate machine. Stop the original daemon before starting restored Telegram polling so only one daemon owns the bot.

Persisted state is not seamless resumption of arbitrary execution. Running jobs recover into `needs_review` with an interrupted-attempt explanation; inspect external effects before retrying. A streaming request or child process cannot continue across a container restart, and in-progress OAuth handshakes may need to be started again. Mail sends with uncertain outcomes require checking Sent mail before another send. This package supplies process restart and durable data, not guaranteed exactly-once execution or high availability.

Delegated helpers cannot execute shell commands or manage background processes, even when the main companion has those tools. The helper's 120-second wait limit is not a universal cancellation guarantee: inspect status and external effects before retrying an interrupted or timed-out operation.

## Backup

Run these commands from `deploy/vps` during a quiet period. This stops services, so SQLite and keyring files are copied consistently. The archive includes `.env`, private keys, files, memory, and account credentials: encrypt it before copying it off the VPS, restrict access, and never commit it.

```sh
mkdir -p backups
chmod 700 backups
docker compose stop
docker run --rm \
  -v anima-companion_daemon_state:/snapshot/state:ro \
  -v anima-companion_workspace:/snapshot/workspace:ro \
  -v anima-companion_daemon_home:/snapshot/home:ro \
  -v anima-companion_caddy_data:/snapshot/caddy-data:ro \
  -v anima-companion_caddy_config:/snapshot/caddy-config:ro \
  -v "$PWD/.env":/snapshot/deployment.env:ro \
  -v "$PWD/backups":/backups \
  alpine:3.22 sh -c 'umask 077; tar -czf /backups/anima-$(date -u +%Y%m%dT%H%M%SZ).tar.gz -C /snapshot .'
docker compose up -d
```

Confirm the archive exists and can be listed with `tar -tzf backups/ARCHIVE.tar.gz` before considering the backup successful. Retain the repository revision and image IDs (`git rev-parse HEAD`, `docker compose images`) with each backup. Keep backups outside the live VPS as well.

## Restore to a fresh VPS

Use the same repository revision and deployment project name, with **empty new volumes**. Do not extract over a live installation. Keep the original keyring password. The following assumes the archive is `backups/restore.tar.gz`; run the first three commands only on the fresh installation with no existing `.env`. If moving to a new hostname, edit the restored `ANIMA_DOMAIN` and update DNS before starting services:

```sh
umask 077
tar -xOf backups/restore.tar.gz ./deployment.env > .env
chmod 600 .env
docker compose config --quiet
docker compose build
docker compose create
docker run --rm \
  -v anima-companion_daemon_state:/snapshot/state \
  -v anima-companion_workspace:/snapshot/workspace \
  -v anima-companion_daemon_home:/snapshot/home \
  -v anima-companion_caddy_data:/snapshot/caddy-data \
  -v anima-companion_caddy_config:/snapshot/caddy-config \
  -v "$PWD/backups":/backups:ro \
  alpine:3.22 tar -xzf /backups/restore.tar.gz -C /snapshot ./state ./workspace ./home ./caddy-data ./caddy-config
docker compose up -d
docker compose ps
```

The archive retains numeric ownership; the daemon runs as UID/GID 10001. Verify the acceptance checks before deleting any old installation or backup. Do not use `docker compose down -v`, which removes the named volumes.

## Updates and rollback

Make a verified backup first. Review and obtain the desired repository revision, then rebuild:

```sh
docker compose build --pull
docker compose down
docker compose up -d
docker compose ps
```

`down` without `-v` preserves volumes and deliberately recreates both services so the daemon joins the replacement gateway network namespace. Do not replace the gateway alone while leaving a running daemon attached to its old namespace. If an update fails, stop the services, return to the recorded repository/image revision, and restore a backup into fresh volumes if a storage-format change prevents reading the current data. Do not assume older binaries can read newer snapshots.

For a provider API-key change, edit `.env` and recreate the daemon with `docker compose up -d --force-recreate daemon`. For gateway login/internal-token changes, recreate both services using the update sequence. Vault-password rotation requires a deliberate keyring password migration and is not supplied by this package.

## Local gateway regression checks

With Bun 1.3.8 and Caddy 2.10.2 installed:

```sh
CADDY_BINARY=/absolute/path/to/caddy bun test deploy/vps/gateway.test.ts
```

Run this from the repository root. It launches Caddy only on loopback with test credentials and a harmless stub upstream, and checks authentication, forwarding-header removal, Origin/Fetch Metadata protection, exact callback exceptions, and SSE delivery. It does not validate live providers, the Linux keyring, Docker networking, or certificate issuance.
