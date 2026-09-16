#!/bin/sh
set -eu
umask 077

: "${ANIMA_KEYRING_PASSWORD:?Keyring password is required}"
: "${ANIMAOS_RS_API_KEY:?Daemon API key is required}"
: "${ANIMA_LOCAL_ADMIN_TOKEN:?Local owner token is required}"
if [ "${#ANIMA_KEYRING_PASSWORD}" -lt 32 ] || [ "${#ANIMAOS_RS_API_KEY}" -lt 32 ] || [ "${#ANIMA_LOCAL_ADMIN_TOKEN}" -lt 32 ]; then
  echo 'Deployment secrets must each contain at least 32 characters.' >&2
  exit 1
fi
if [ "$ANIMAOS_RS_API_KEY" = "$ANIMA_LOCAL_ADMIN_TOKEN" ] || [ "$ANIMA_KEYRING_PASSWORD" = "$ANIMAOS_RS_API_KEY" ] || [ "$ANIMA_KEYRING_PASSWORD" = "$ANIMA_LOCAL_ADMIN_TOKEN" ]; then
  echo 'API key, owner token, and keyring password must be distinct.' >&2
  exit 1
fi

mkdir -p "$XDG_RUNTIME_DIR" "$XDG_DATA_HOME/keyrings"
chmod 700 "$XDG_RUNTIME_DIR" "$XDG_DATA_HOME/keyrings"
if [ "${1:-}" != '--session' ]; then
  exec dbus-run-session -- "$0" --session
fi

# --unlock creates the login keyring on first boot and unlocks it thereafter.
# Do not print or put the password in argv. The persisted encrypted keyring and
# this exact password must be restored together.
printf '%s' "$ANIMA_KEYRING_PASSWORD" | gnome-keyring-daemon --unlock --components=secrets >/dev/null
unset ANIMA_KEYRING_PASSWORD

# Fail closed instead of accepting connectors into an unusable credential vault.
printf 'ready' | timeout 15 secret-tool store --label='Anima deployment vault probe' anima-deployment probe
probe=$(timeout 15 secret-tool lookup anima-deployment probe)
[ "$probe" = ready ] || { echo 'Credential vault verification failed.' >&2; exit 1; }
timeout 15 secret-tool clear anima-deployment probe
unset probe
exec /usr/local/bin/anima-daemon
