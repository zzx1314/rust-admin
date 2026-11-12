#!/usr/bin/env bash
#
# Start the API with the secrets it requires.
#
# `config/config.toml` is checked in and never holds secrets, so
# APP_JWT_SECRET must come from the environment (see src/config/mod.rs).
# This wrapper exports it and then runs `cargo run`.
#
# APP_JWT_SECRET is generated per run when unset: it only signs user tokens,
# and a fresh value merely forces a re-login.
#
# APP_REDIS_PASSWORD is only needed when Redis enforces AUTH (the checked-in
# docker-compose redis does); otherwise login fails with
# `Redis set failed: NOAUTH`. When unset, the local Redis at localhost:6379 is
# probed and the dev password is used only if it asks for one.
#
# 不想装 Redis？`APP_TOKEN_STORE=memory ./run.sh` 改用进程内存存会话
# （重启后需重新登录），此时不会再连 Redis。
#
# Usage:
#   ./run.sh                 # debug build
#   ./run.sh --release       # any extra args are forwarded to `cargo run`
set -euo pipefail

cd "$(dirname "$0")"

# 本地 secrets（不入库，.gitignore 忽略）：
#   APP_DOG__BUSINESS_API_TOKEN —— 业务平台内部接口 + 反向控制信道的共享令牌
# 存在即自动加载，不存在的机器照旧靠手工 export。
if [[ -f .env.local ]]; then
  set -a
  # shellcheck disable=SC1091
  source .env.local
  set +a
fi

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

# `APP_TOKEN_STORE=memory`（或 config.toml 里 `token_store = "memory"`）时不需要
# Redis，上面的 AUTH 探测也跟着跳过，本机没装 redis-server 也能起服务。
token_store_kind() {
  local value=""
  if [[ -f config/config.toml ]]; then
    value="$(awk '
      /^\[/ { section = $0; next }
      section != "[redis]" && $1 == "token_store" { gsub(/[" ]/, "", $3); print $3 }
    ' config/config.toml | tail -1)"
  fi
  echo "${APP_TOKEN_STORE:-${value:-redis}}"
}

if [[ "$(token_store_kind)" == "memory" ]]; then
  echo "APP_TOKEN_STORE=memory（会话存进程内存，本次不连 Redis）"
  echo "APP_JWT_SECRET=${APP_JWT_SECRET:0:8}… (exported value or generated for this run)"
  echo
  exec cargo run "$@"
fi

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
echo "APP_REDIS_PASSWORD=${APP_REDIS_PASSWORD:+<set>} ($redis_password_source)"
echo

exec cargo run "$@"
