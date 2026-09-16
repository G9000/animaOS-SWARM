FROM node:22-bookworm-slim AS web
COPY --from=oven/bun:1.3.8 /usr/local/bin/bun /usr/local/bin/bun
WORKDIR /build
COPY . .
ENV NX_DAEMON=false CI=1
RUN bun install --frozen-lockfile
RUN bun x nx run @animaOS-SWARM/sdk:build --skipNxCache
RUN bun x nx run @animaOS-SWARM/web:build --skipNxCache

FROM caddy:2.10.2-alpine
COPY --from=web /build/apps/web/dist /srv
COPY deploy/vps/Caddyfile /etc/caddy/Caddyfile
