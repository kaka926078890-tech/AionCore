#!/usr/bin/env bash
# Package aioncore release tarball for findesk prepareAioncore (local or CI helper).
set -euo pipefail

TAG="${1:-v0.1.27-findesk.1}"
TARGET="${2:-aarch64-apple-darwin}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
VERSION="${TAG#v}"
OUT_DIR="${ROOT}/dist"
ARCHIVE="aioncore-${TAG}-${TARGET}.tar.gz"

mkdir -p "${OUT_DIR}"
BINARY="${ROOT}/target/${TARGET}/release/aioncore"
if [[ ! -f "${BINARY}" ]]; then
  echo "Binary not found at ${BINARY}; building..."
  cargo build --release --features findesk --target "${TARGET}" -p aionui-app
fi

tar -C "${ROOT}/target/${TARGET}/release" -czf "${OUT_DIR}/${ARCHIVE}" aioncore
echo "Created ${OUT_DIR}/${ARCHIVE}"
