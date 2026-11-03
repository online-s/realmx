#!/bin/sh
set -eu

REPO="${REALM_REPO:-online-s/realmx}"
VERSION="${REALM_VERSION:-latest}"
INSTALL_DIR="${REALM_INSTALL_DIR:-/usr/local/bin}"
SERVICE_NAME="realm"
BINARY_PATH="${INSTALL_DIR}/realm"
SERVICE_PATH="/etc/systemd/system/${SERVICE_NAME}.service"

log() {
    printf '[realm-install] %s\n' "$*"
}

die() {
    printf '[realm-install] error: %s\n' "$*" >&2
    exit 1
}

as_root() {
    if [ "$(id -u)" -eq 0 ]; then
        "$@"
    else
        command -v sudo >/dev/null 2>&1 || die "run as root or install sudo"
        sudo "$@"
    fi
}

require_command() {
    command -v "$1" >/dev/null 2>&1 || die "required command not found: $1"
}

detect_targets() {
    [ "$(uname -s)" = "Linux" ] || die "only Linux is supported"

    case "$(uname -m)" in
        x86_64|amd64)
            printf '%s\n' 'x86_64-unknown-linux-musl' 'x86_64-unknown-linux-gnu-glibc2.28'
            ;;
        aarch64|arm64)
            printf '%s\n' 'aarch64-unknown-linux-musl' 'aarch64-unknown-linux-gnu-glibc2.28'
            ;;
        armv7l|armv7)
            printf '%s\n' 'armv7-unknown-linux-musleabihf' 'armv7-unknown-linux-gnueabihf-glibc2.28'
            ;;
        armv6l|armv6|arm)
            printf '%s\n' 'arm-unknown-linux-musleabihf' 'arm-unknown-linux-gnueabihf-glibc2.28'
            ;;
        mips)
            printf '%s\n' 'mips-unknown-linux-musl' 'mips-unknown-linux-gnu-glibc2.28'
            ;;
        mipsel)
            printf '%s\n' 'mipsel-unknown-linux-musl' 'mipsel-unknown-linux-gnu-glibc2.28'
            ;;
        mips64)
            printf '%s\n' 'mips64-unknown-linux-muslabi64' 'mips64-unknown-linux-gnuabi64-glibc2.28'
            ;;
        mips64el)
            printf '%s\n' 'mips64el-unknown-linux-muslabi64' 'mips64el-unknown-linux-gnuabi64-glibc2.28'
            ;;
        *) die "unsupported architecture: $(uname -m)" ;;
    esac
}

uninstall_realm() {
    require_command systemctl
    log "stopping and disabling ${SERVICE_NAME}.service"
    as_root systemctl disable --now "${SERVICE_NAME}.service" >/dev/null 2>&1 || true
    as_root rm -f "$SERVICE_PATH" "$BINARY_PATH"
    as_root systemctl daemon-reload
    log "uninstalled"
}

install_realm() {
    require_command curl
    require_command tar
    require_command systemctl
    require_command mktemp
    require_command install

    case "$INSTALL_DIR" in
        *' '*) die "REALM_INSTALL_DIR must not contain spaces" ;;
    esac

    temp_dir="$(mktemp -d)"
    trap 'rm -rf "$temp_dir"' EXIT HUP INT TERM

    archive=''
    for target in $(detect_targets); do
        candidate="realm-${target}.tar.gz"
        if [ "$VERSION" = "latest" ]; then
            download_url="https://github.com/${REPO}/releases/latest/download/${candidate}"
        else
            download_url="https://github.com/${REPO}/releases/download/${VERSION}/${candidate}"
        fi

        log "downloading ${download_url}"
        if curl -fsSL --retry 3 --connect-timeout 15 -o "${temp_dir}/${candidate}" "$download_url"; then
            archive="$candidate"
            break
        fi
        rm -f "${temp_dir}/${candidate}"
        log "asset unavailable, trying compatible fallback"
    done

    [ -n "$archive" ] || die "no compatible release asset is available"
    tar -xzf "${temp_dir}/${archive}" -C "$temp_dir" realm
    [ -f "${temp_dir}/realm" ] || die "release archive does not contain realm"

    as_root install -d -m 0755 "$INSTALL_DIR"
    as_root install -m 0755 "${temp_dir}/realm" "${BINARY_PATH}.new"
    as_root mv -f "${BINARY_PATH}.new" "$BINARY_PATH"

    cat >"${temp_dir}/${SERVICE_NAME}.service" <<EOF
[Unit]
Description=Realm built-in relay service
Wants=network-online.target
After=network-online.target

[Service]
Type=simple
ExecStart=${BINARY_PATH}
DynamicUser=yes
NoNewPrivileges=yes
PrivateTmp=yes
ProtectHome=yes
ProtectSystem=strict
Restart=on-failure
RestartSec=3s
LimitNOFILE=1048576

[Install]
WantedBy=multi-user.target
EOF

    as_root install -m 0644 "${temp_dir}/${SERVICE_NAME}.service" "$SERVICE_PATH"
    as_root systemctl daemon-reload
    as_root systemctl enable "${SERVICE_NAME}.service" >/dev/null
    as_root systemctl restart "${SERVICE_NAME}.service"

    if ! as_root systemctl is-active --quiet "${SERVICE_NAME}.service"; then
        as_root systemctl status "${SERVICE_NAME}.service" --no-pager || true
        die "service failed to start"
    fi

    log "installed ${BINARY_PATH} and started ${SERVICE_NAME}.service"
    log "status: systemctl status ${SERVICE_NAME} --no-pager"
    log "logs:   journalctl -u ${SERVICE_NAME} -f"
}

case "${1:-install}" in
    install) install_realm ;;
    uninstall) uninstall_realm ;;
    *) die "usage: $0 [install|uninstall]" ;;
esac
