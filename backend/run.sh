#!/usr/bin/env bash
#
# Start the API with the secrets it requires.
#
# `config/config.toml` is checked in and never holds secrets, so
# APP_JWT_SECRET and APP_DEVICE_AUTH_SECRET must come from the environment
# (see src/config/mod.rs). This wrapper exports them and then runs `cargo run`.
#
# APP_JWT_SECRET is generated per run when unset: it only signs user tokens,
# and a fresh value merely forces a re-login.
#
# APP_DEVICE_AUTH_SECRET is the HMAC key shared with the device client
# (`--auth-secret` / DOG_AUTH_SECRET) for
# `GET /api/dogGateway/auth/{deviceId}`, so it is pinned to a stable local
# value instead of changing on every start. Export your own value to override.
#
# APP_REDIS_PASSWORD is only needed when Redis enforces AUTH (the checked-in
# docker-compose redis does); otherwise login fails with
# `Redis set failed: NOAUTH`. When unset, the local Redis at localhost:6379 is
# probed and the dev password is used only if it asks for one.
#
# Usage:
#   ./run.sh                 # debug build
#   ./run.sh --release       # any extra args are forwarded to `cargo run`
set -euo pipefail

cd "$(dirname "$0")"

DEFAULT_DEVICE_AUTH_SECRET="local-dev-device-secret-change-me"
DEV_REDIS_PASSWORD="123456"

random_hex() {
  if command -v openssl >/dev/null 2>&1; then
    openssl rand -hex 32
  else
    od -An -tx1 -N32 /dev/urandom | tr -d ' \n'
  fi
}

redis_setting() {
  local key="$1" default="$2" value=""
  if [[ -f config/config.toml ]]; then
    value="$(awk -v key="$key" '
      /^\[/ { section = $0; next }
      section == "[redis]" && $1 == key { gsub(/[" ]/, "", $3); print $3 }
    ' config/config.toml | tail -1)"
  fi
  echo "${value:-$default}"
}

redis_requires_auth() {
  local host="$1" port="$2" reply=""
  if ! { exec 3<>"/dev/tcp/${host}/${port}"; } 2>/dev/null; then
    return 1
  fi
  printf 'PING\r\n' >&3 2>/dev/null || { exec 3<&- 3>&-; return 1; }
  IFS= read -r -t 2 reply <&3 || true
  exec 3<&- 3>&-
  [[ "${reply}" == -NOAUTH* ]]
}

export APP_JWT_SECRET="${APP_JWT_SECRET:-$(random_hex)}"
export APP_DEVICE_AUTH_SECRET="${APP_DEVICE_AUTH_SECRET:-$DEFAULT_DEVICE_AUTH_SECRET}"

redis_password_source="exported"
if [[ -z "${APP_REDIS_PASSWORD:-}" ]]; then
  redis_host="$(redis_setting host localhost)"
  redis_port="$(redis_setting port 6379)"
  if redis_requires_auth "$redis_host" "$redis_port"; then
    APP_REDIS_PASSWORD="$DEV_REDIS_PASSWORD"
    redis_password_source="dev default, ${redis_host}:${redis_port} requires AUTH"
  else
    redis_password_source="none, ${redis_host}:${redis_port} did not ask for AUTH"
  fi
fi
export APP_REDIS_PASSWORD

echo "APP_JWT_SECRET=${APP_JWT_SECRET:0:8}… (exported value or generated for this run)"
echo "APP_DEVICE_AUTH_SECRET=$APP_DEVICE_AUTH_SECRET"
echo "device client must sign with the same secret: DOG_AUTH_SECRET=$APP_DEVICE_AUTH_SECRET"
echo "APP_REDIS_PASSWORD=${APP_REDIS_PASSWORD:+<set>} ($redis_password_source)"
echo

exec cargo run "$@"
