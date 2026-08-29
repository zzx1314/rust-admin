#!/usr/bin/env bash

set -Eeuo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REGISTRY="${REGISTRY:-}"
TAG="${TAG:-latest}"
PUSH=false
TARGET="all"

usage() {
  cat <<'EOF'
用法:
  ./build-docker.sh [选项]

选项:
  --frontend             只构建前端镜像
  --backend              只构建后端镜像
  --all                  构建前后端镜像（默认）
  --tag TAG              设置镜像标签，默认 latest
  --registry REGISTRY    设置镜像仓库前缀，例如 registry.example.com/team
  --push                 构建完成后推送镜像
  -h, --help             显示帮助

示例:
  ./build-docker.sh
  ./build-docker.sh --frontend --tag v1.0.0
  ./build-docker.sh --all --registry registry.example.com/team --tag v1.0.0 --push
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --frontend)
      TARGET="frontend"
      shift
      ;;
    --backend)
      TARGET="backend"
      shift
      ;;
    --all)
      TARGET="all"
      shift
      ;;
    --tag)
      [[ $# -ge 2 ]] || { echo "--tag 需要一个值" >&2; exit 1; }
      TAG="$2"
      shift 2
      ;;
    --registry)
      [[ $# -ge 2 ]] || { echo "--registry 需要一个值" >&2; exit 1; }
      REGISTRY="${2%/}"
      shift 2
      ;;
    --push)
      PUSH=true
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "未知参数: $1" >&2
      usage >&2
      exit 1
      ;;
  esac
done

image_name() {
  local service="$1"
  if [[ -n "$REGISTRY" ]]; then
    printf '%s/rust-admin-%s:%s' "$REGISTRY" "$service" "$TAG"
  else
    printf 'rust-admin-%s:%s' "$service" "$TAG"
  fi
}

build_image() {
  local service="$1"
  local context="$ROOT_DIR/$service"
  local image
  image="$(image_name "$service")"

  echo "构建 $service 镜像: $image"
  docker build --pull -t "$image" "$context"

  if [[ "$PUSH" == true ]]; then
    echo "推送镜像: $image"
    docker push "$image"
  fi
}

case "$TARGET" in
  frontend)
    build_image frontend
    ;;
  backend)
    build_image backend
    ;;
  all)
    build_image backend
    build_image frontend
    ;;
esac

echo "Docker 构建完成"
