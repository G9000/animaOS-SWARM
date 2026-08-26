# anima-telegram-gateway

A thin Rust host that bridges Telegram to the anima daemon HTTP API. It is a
pure client of the daemon: it polls Telegram for messages, forwards them to a
daemon agent's run endpoint, sends the replies back, and pushes proactive
assistant messages from the daemon outbox to authorized chats. It contains no
agent/engine logic.

## Environment variables

| Variable | Required | Default | Description |
| --- | --- | --- | --- |
| `TELEGRAM_BOT_TOKEN` | yes | — | Bot token from @BotFather. The gateway fails fast at startup if missing. |
| `ANIMAOS_RS_DAEMON_URL` | no | `http://127.0.0.1:8080` | Base URL of the anima daemon HTTP API. |
| `ANIMAOS_RS_ASSISTANT_NAME` | no | `assistant` | Agent name to resolve to an id at startup via `GET /api/agents`. Startup fails if no agent with this name exists. |
| `TELEGRAM_ALLOWED_CHAT_IDS` | no | _(empty)_ | Comma-separated i64 chat id allowlist — the auth mechanism. While empty, every incoming chat gets an "unauthorized" reply that includes its chat id, and outbox pushes are skipped. |
| `TELEGRAM_OUTBOX_POLL_SECS` | no | `15` | Interval between assistant outbox polls. |

## Running

Start the anima daemon first (it serves the API this gateway consumes), then:

```sh
bun x nx run telegram-gateway:dev
```

Other targets: `bun x nx run telegram-gateway:build`,
`bun x nx run telegram-gateway:test`, `bun x nx run telegram-gateway:lint`.

## First run

1. Create a bot with [@BotFather](https://t.me/BotFather) and copy the token
   into `TELEGRAM_BOT_TOKEN`.
2. Start the gateway with the allowlist unset.
3. Message your bot from Telegram. The gateway replies with an unauthorized
   notice that includes your chat id (it is also logged).
4. Set `TELEGRAM_ALLOWED_CHAT_IDS` to that id (comma-separate multiple ids)
   and restart the gateway.
