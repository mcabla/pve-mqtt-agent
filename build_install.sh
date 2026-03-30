#!/bin/sh
# build_install.sh
#
# Build + install pve-mqtt-agent (Alpine-friendly).
# - Builds a release binary with Cargo.
# - Installs to /opt/pve-mqtt-agent/bin/pve-mqtt-agent
# - Ensures config/state dirs exist and permissions are sane.
# - Restarts OpenRC service if present.
#
# Usage:
#   sh ./build_install.sh
#
# Optional env:
#   BIN_DIR=/opt/pve-mqtt-agent/bin
#   CONFIG_DIR=/etc/pve-mqtt-agent
#   STATE_DIR=/var/lib/pve-mqtt-agent
#   SERVICE_NAME=pve-mqtt-agent
#   CARGO_BIN=cargo

set -eu

BIN_DIR="${BIN_DIR:-/opt/pve-mqtt-agent/bin}"
CONFIG_DIR="${CONFIG_DIR:-/etc/pve-mqtt-agent}"
STATE_DIR="${STATE_DIR:-/var/lib/pve-mqtt-agent}"
SERVICE_NAME="${SERVICE_NAME:-pve-mqtt-agent}"
CARGO_BIN="${CARGO_BIN:-cargo}"

PROJECT_ROOT="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"
TARGET_BIN="${PROJECT_ROOT}/target/release/pve-mqtt-agent"
INSTALL_BIN="${BIN_DIR}/pve-mqtt-agent"

log() {
  printf '%s\n' "$*" >&2
}

die() {
  log "ERROR: $*"
  exit 1
}

need_cmd() {
  command -v "$1" >/dev/null 2>&1 || die "missing required command: $1"
}

is_root() {
  [ "$(id -u)" -eq 0 ]
}

ensure_user_group() {
  # Create service user/group if missing (Alpine busybox adduser/addgroup).
  if ! getent group pveagent >/dev/null 2>&1; then
    log "Creating group pveagent"
    addgroup -S pveagent >/dev/null
  fi
  if ! getent passwd pveagent >/dev/null 2>&1; then
    log "Creating user pveagent"
    adduser -S -D -H -s /sbin/nologin -G pveagent pveagent >/dev/null
  fi
}

ensure_dirs() {
  mkdir -p "${BIN_DIR}" "${CONFIG_DIR}" "${STATE_DIR}"

  # State dir should be writable by the service user.
  chown -R pveagent:pveagent "${STATE_DIR}"
  chmod 0750 "${STATE_DIR}"

  # Config dir readable by service user (files inside should be 0600/0640 depending on your preference).
  chown -R pveagent:pveagent "${CONFIG_DIR}"
  chmod 0750 "${CONFIG_DIR}"
}

build_release() {
  log "Building release (this can take a bit on Alpine the first time)..."
  (cd "${PROJECT_ROOT}" && "${CARGO_BIN}" build --release)
  [ -f "${TARGET_BIN}" ] || die "expected binary not found: ${TARGET_BIN}"
}

install_binary() {
  log "Installing binary to ${INSTALL_BIN}"
  # Use install(1) if available; fallback to cp+chmod.
  if command -v install >/dev/null 2>&1; then
    install -m 0755 "${TARGET_BIN}" "${INSTALL_BIN}"
  else
    cp -f "${TARGET_BIN}" "${INSTALL_BIN}"
    chmod 0755 "${INSTALL_BIN}"
  fi
}

restart_service_if_present() {
  # OpenRC service restart if the init script exists.
  if [ -x "/etc/init.d/${SERVICE_NAME}" ]; then
    log "Restarting OpenRC service: ${SERVICE_NAME}"
    # rc-service returns non-zero if service isn't added yet; don't hard-fail.
    rc-service "${SERVICE_NAME}" restart || rc-service "${SERVICE_NAME}" start || true
  else
    log "OpenRC init script not found at /etc/init.d/${SERVICE_NAME} (skipping restart)"
  fi
}

check_and_install_config() {
  # Check if config.toml exists in the project root.
  local src_config="${PROJECT_ROOT}/config.toml"
  if [ ! -f "${src_config}" ]; then
    die "config.toml not found in ${PROJECT_ROOT}. Please create it from config.example.toml:
    cp ${PROJECT_ROOT}/config.example.toml ${PROJECT_ROOT}/config.toml
    Then edit it with your actual credentials and run this script again."
  fi

  # Copy config to CONFIG_DIR if not already there.
  local dest_config="${CONFIG_DIR}/config.toml"
  log "Installing config to ${dest_config}"
  cp -f "${src_config}" "${dest_config}"
  # Restrict permissions: only service user can read
  chown pveagent:pveagent "${dest_config}"
  chmod 0640 "${dest_config}"
}

main() {
  need_cmd "${CARGO_BIN}"
  need_cmd id
  need_cmd mkdir
  need_cmd chmod
  need_cmd chown
  need_cmd getent
  need_cmd adduser
  need_cmd addgroup

  is_root || die "run as root (needed to install to /opt and manage service user)"

  ensure_user_group
  ensure_dirs
  check_and_install_config
  build_release
  install_binary
  restart_service_if_present

  log "Done."
  log "Binary: ${INSTALL_BIN}"
  log "Config: ${CONFIG_DIR}/config.toml"
  log "Logs (if configured in OpenRC): /var/log/${SERVICE_NAME}.log"
}

main "$@"
