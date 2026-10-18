#!/usr/bin/env sh
set -eu

REPO="dwaddle/orbit-cli"
INSTALL_DIR="${ORBIT_INSTALL_DIR:-$HOME/.local/bin}"

OS="$(uname -s)"
ARCH="$(uname -m)"

case "${OS}" in
  Linux)
    case "${ARCH}" in
      x86_64|amd64) TARGET="x86_64-unknown-linux-gnu" ;;
      *) echo "Unsupported Linux architecture: ${ARCH} (use 'cargo install --git https://github.com/${REPO}')" >&2; exit 1 ;;
    esac
    ;;
  Darwin)
    case "${ARCH}" in
      arm64|aarch64) TARGET="aarch64-apple-darwin" ;;
      x86_64) TARGET="x86_64-apple-darwin" ;;
      *) echo "Unsupported macOS architecture: ${ARCH}" >&2; exit 1 ;;
    esac
    ;;
  *)
    echo "Unsupported OS: ${OS} (use 'cargo install --git https://github.com/${REPO}')" >&2
    exit 1
    ;;
esac

TAG="$(curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" | grep '"tag_name":' | sed -E 's/.*"([^"]+)".*/\1/')"
if [ -z "${TAG}" ]; then
  echo "Failed to determine latest release tag from GitHub." >&2
  exit 1
fi

ARCHIVE="orbit-cli-${TAG}-${TARGET}.tar.gz"
URL="https://github.com/${REPO}/releases/download/${TAG}/${ARCHIVE}"

TMP_DIR="$(mktemp -d)"
trap 'rm -rf "${TMP_DIR}"' EXIT

echo "Downloading Orbit CLI ${TAG} (${TARGET})..."
curl -fsSL "${URL}" -o "${TMP_DIR}/${ARCHIVE}"
tar -xzf "${TMP_DIR}/${ARCHIVE}" -C "${TMP_DIR}"

mkdir -p "${INSTALL_DIR}"
install -m 755 "${TMP_DIR}/orbit-cli" "${INSTALL_DIR}/orbit-cli"
ln -sf "${INSTALL_DIR}/orbit-cli" "${INSTALL_DIR}/orbit"

echo "Installed orbit-cli (${TAG}) to ${INSTALL_DIR}/orbit-cli (alias: ${INSTALL_DIR}/orbit)"
