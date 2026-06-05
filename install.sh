#!/usr/bin/env bash
# Install code-ui from GitHub Releases.
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/motionharvest/code-ui/main/install.sh | bash
#
# Options (environment variables):
#   CODE_UI_VERSION   Pin a release tag, e.g. v0.1.0 (default: latest)
#   INSTALL_DIR       Where to put the binary (default: ~/.local/bin)
#   CODE_UI_REPO      GitHub repo slug (default: motionharvest/code-ui)
set -euo pipefail

REPO="${CODE_UI_REPO:-motionharvest/code-ui}"
INSTALL_DIR="${INSTALL_DIR:-${HOME}/.local/bin}"
VERSION="${CODE_UI_VERSION:-}"

err() {
  echo "install.sh: $*" >&2
  exit 1
}

need_cmd() {
  command -v "$1" >/dev/null 2>&1 || err "missing required command: $1"
}

detect_target() {
  local os arch
  os="$(uname -s)"
  arch="$(uname -m)"

  case "${os}:${arch}" in
    Linux:x86_64) echo "x86_64-unknown-linux-gnu" ;;
    Linux:aarch64 | Linux:arm64) echo "aarch64-unknown-linux-gnu" ;;
    Darwin:x86_64) echo "x86_64-apple-darwin" ;;
    Darwin:arm64 | Darwin:aarch64) echo "aarch64-apple-darwin" ;;
    *) err "unsupported platform: ${os} ${arch}" ;;
  esac
}

asset_name() {
  local version="$1"
  local target="$2"
  echo "code-ui-${version}-${target}.tar.gz"
}

fetch_latest_version() {
  need_cmd curl
  curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" \
    | sed -n 's/.*"tag_name": "\([^"]*\)".*/\1/p' \
    | head -n1
}

download_release() {
  local version="$1"
  local target="$2"
  local asset
  asset="$(asset_name "${version}" "${target}")"
  local url="https://github.com/${REPO}/releases/download/${version}/${asset}"

  need_cmd curl
  need_cmd tar

  local tmpdir
  tmpdir="$(mktemp -d)"
  trap 'rm -rf "${tmpdir}"' EXIT

  echo "Downloading ${url} ..."
  curl -fsSL "${url}" -o "${tmpdir}/${asset}"
  tar -xzf "${tmpdir}/${asset}" -C "${tmpdir}"

  if [[ ! -f "${tmpdir}/code-ui" ]]; then
    err "archive did not contain a code-ui binary"
  fi

  mkdir -p "${INSTALL_DIR}"
  install -m 0755 "${tmpdir}/code-ui" "${INSTALL_DIR}/code-ui"
}

main() {
  need_cmd uname

  if [[ -z "${VERSION}" ]]; then
    VERSION="$(fetch_latest_version)"
    [[ -n "${VERSION}" ]] || err "could not determine latest release version"
  fi

  local target
  target="$(detect_target)"
  echo "Installing code-ui ${VERSION} for ${target} into ${INSTALL_DIR} ..."

  download_release "${VERSION}" "${target}"

  if [[ ":${PATH}:" != *":${INSTALL_DIR}:"* ]]; then
    echo
    echo "Installed ${INSTALL_DIR}/code-ui"
    echo "Add this to your shell profile if needed:"
    echo "  export PATH=\"${INSTALL_DIR}:\$PATH\""
  else
    echo "Installed ${INSTALL_DIR}/code-ui"
  fi

  echo
  echo "Requirements: tmux and git must be installed."
  echo "Run from a project directory:  cd your-repo && code-ui"
}

main "$@"
