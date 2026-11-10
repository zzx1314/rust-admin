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
# Usage:
#   ./run.sh                 # debug build
#   ./run.sh --release       # any extra args are forwarded to `cargo run`
set -euo pipefail

cd "$(dirname "$0")"

DEFAULT_DEVICE_AUTH_SECRET="local-dev-device-secret-change-me"

random_hex() {
  if command -v openssl >/dev/null 2>&1; then
    openssl rand -hex 32
  else
    od -An -tx1 -N32 /dev/urandom | tr -d ' \n'
  fi
}

export APP_JWT_SECRET="${APP_JWT_SECRET:-$(random_hex)}"
export APP_DEVICE_AUTH_SECRET="${APP_DEVICE_AUTH_SECRET:-$DEFAULT_DEVICE_AUTH_SECRET}"

echo "APP_JWT_SECRET=${APP_JWT_SECRET:0:8}… (exported value or generated for this run)"
echo "APP_DEVICE_AUTH_SECRET=$APP_DEVICE_AUTH_SECRET"
echo "device client must sign with the same secret: DOG_AUTH_SECRET=$APP_DEVICE_AUTH_SECRET"
echo

exec cargo run "$@"
