#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Install Grapha from the latest macOS release artifact.

Usage:
  install.sh [--version <tag>] [--install-dir <dir>] [--repo <owner/name>]

Environment:
  GRAPHA_RELEASE_REPO   GitHub repository to download from (default: oops-rs/grapha)
  GRAPHA_INSTALL_DIR    Install destination (default: $HOME/.local/bin)

Notes:
  - This installer currently supports macOS release artifacts only.
  - Both `grapha` and `libGraphaSwiftBridge.dylib` are installed into the same directory.
EOF
}

require_cmd() {
  if ! command -v "$1" >/dev/null 2>&1; then
    echo "missing required command: $1" >&2
    exit 1
  fi
}

detect_asset_suffix() {
  local os arch
  os="$(uname -s)"
  arch="$(uname -m)"

  if [[ "${os}" != "Darwin" ]]; then
    echo "unsupported platform: ${os}. Use 'cargo install grapha' on non-macOS systems." >&2
    exit 1
  fi

  case "${arch}" in
    arm64|aarch64) echo "macos-arm64" ;;
    x86_64) echo "macos-x86_64" ;;
    *)
      echo "unsupported macOS architecture: ${arch}" >&2
      exit 1
      ;;
  esac
}

fetch_latest_tag() {
  local repo="$1"
  curl -fsSL "https://api.github.com/repos/${repo}/releases/latest" \
    | awk -F'"' '/"tag_name":/ { print $4; exit }'
}

repo="${GRAPHA_RELEASE_REPO:-oops-rs/grapha}"
install_dir="${GRAPHA_INSTALL_DIR:-$HOME/.local/bin}"
version=""

while [[ $# -gt 0 ]]; do
  case "$1" in
    --version)
      version="${2:-}"
      shift 2
      ;;
    --install-dir)
      install_dir="${2:-}"
      shift 2
      ;;
    --repo)
      repo="${2:-}"
      shift 2
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "unknown argument: $1" >&2
      usage >&2
      exit 1
      ;;
  esac
done

require_cmd curl
require_cmd tar
require_cmd install
require_cmd mktemp

asset_suffix="$(detect_asset_suffix)"

if [[ -z "${version}" ]]; then
  version="$(fetch_latest_tag "${repo}")"
  if [[ -z "${version}" ]]; then
    echo "failed to resolve latest release tag from ${repo}" >&2
    exit 1
  fi
fi

archive_name="grapha-${version}-${asset_suffix}.tar.gz"
download_url="https://github.com/${repo}/releases/download/${version}/${archive_name}"

tmp_dir="$(mktemp -d)"
trap 'rm -rf "${tmp_dir}"' EXIT

archive_path="${tmp_dir}/${archive_name}"
curl -fL "${download_url}" -o "${archive_path}"
tar -xzf "${archive_path}" -C "${tmp_dir}"

bundle_dir="${tmp_dir}/grapha-${version}-${asset_suffix}"
if [[ ! -d "${bundle_dir}" ]]; then
  echo "unexpected archive layout: ${bundle_dir} not found" >&2
  exit 1
fi

mkdir -p "${install_dir}"
install -m 755 "${bundle_dir}/grapha" "${install_dir}/grapha"
install -m 644 "${bundle_dir}/libGraphaSwiftBridge.dylib" "${install_dir}/libGraphaSwiftBridge.dylib"

echo "Installed Grapha ${version} to ${install_dir}"
echo "Binary: ${install_dir}/grapha"
echo "Bridge: ${install_dir}/libGraphaSwiftBridge.dylib"

case ":$PATH:" in
  *":${install_dir}:"*) ;;
  *)
    echo "Note: ${install_dir} is not currently on PATH."
    ;;
esac
