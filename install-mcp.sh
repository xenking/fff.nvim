#!/usr/bin/env bash
set -eo pipefail

# FFF MCP + fffq installer
# Usage: curl -fsSL https://raw.githubusercontent.com/xenking/fff.nvim/main/install-mcp.sh | bash

REPO="${FFF_MCP_REPO:-xenking/fff.nvim}"
INSTALL_DIR="${FFF_MCP_INSTALL_DIR:-$HOME/.local/bin}"

info() { printf '\033[1;34m%s\033[0m\n' "$*"; }
success() { printf '\033[1;38;5;208m%s\033[0m\n' "$*"; }
warn() { printf '\033[1;33m%s\033[0m\n' "$*"; }
error() { printf '\033[1;31mError: %s\033[0m\n' "$*" >&2; exit 1; }

# Print JSON with syntax highlighting via jq if available, plain otherwise
print_json() {
    if command -v jq &>/dev/null; then
        echo "$1" | jq .
    else
        echo "$1"
    fi
}

detect_platform() {
    local os arch target

    os="$(uname -s)"
    arch="$(uname -m)"

    case "$os" in
        Linux)
            case "$arch" in
                x86_64)  target="x86_64-unknown-linux-gnu" ;;
                aarch64|arm64) target="aarch64-unknown-linux-gnu" ;;
                *) error "Unsupported architecture: $arch" ;;
            esac
            ;;
        Darwin)
            case "$arch" in
                x86_64)  target="x86_64-apple-darwin" ;;
                aarch64|arm64) target="aarch64-apple-darwin" ;;
                *) error "Unsupported architecture: $arch" ;;
            esac
            ;;
        MINGW*|MSYS*|CYGWIN*)
            case "$arch" in
                x86_64)  target="x86_64-pc-windows-msvc" ;;
                aarch64|arm64) target="aarch64-pc-windows-msvc" ;;
                *) error "Unsupported architecture: $arch" ;;
            esac
            ;;
        *) error "Unsupported OS: $os" ;;
    esac

    echo "$target"
}

get_latest_release_tag() {
    local target="$1"
    local releases_json
    releases_json=$(curl -fsSL "https://api.github.com/repos/${REPO}/releases") \
        || error "Failed to fetch releases from https://github.com/${REPO}/releases"

    # Find the first release that contains an fff-mcp binary for our platform.
    # fffq is downloaded from the same release.
    local tag
    tag=$(echo "$releases_json" \
        | grep -oE '"(tag_name|name)": *"[^"]*"' \
        | awk -v target="fff-mcp-${target}" '
            /"tag_name":/ { gsub(/.*": *"|"/, ""); current_tag = $0; next }
            /"name":/ && index($0, target) { print current_tag; exit }
        ')

    if [ -z "$tag" ]; then
        error "No release found containing fff-mcp binaries for ${target}. The MCP build may not have been released yet."
    fi
    echo "$tag"
}

download_binary() {
    local binary_name="$1"
    local target="$2"
    local tag="$3"
    local ext="$4"

    local filename="${binary_name}-${target}${ext}"
    local url="https://github.com/${REPO}/releases/download/${tag}/${filename}"
    local checksum_url="${url}.sha256"
    local output_path="$5"

    info "Downloading ${filename} from release ${tag}..."

    if ! curl -fsSL -o "$output_path" "$url" 2>/dev/null; then
        echo "" >&2
        printf '\033[1;31mError: Failed to download %s for your platform.\033[0m\n' "$binary_name" >&2
        echo "" >&2
        echo "  URL: ${url}" >&2
        echo "  Release: ${tag}" >&2
        echo "  Platform: ${target}" >&2
        echo "" >&2
        echo "Check available releases at: https://github.com/${REPO}/releases" >&2
        exit 1
    fi

    if curl -fsSL -o "${output_path}.sha256" "$checksum_url" 2>/dev/null; then
        info "Verifying ${filename} checksum..."
        if command -v sha256sum &>/dev/null; then
            (cd "$(dirname "$output_path")" && sha256sum -c "$(basename "${output_path}.sha256")") \
                || error "Checksum verification failed for ${filename}!"
        elif command -v shasum &>/dev/null; then
            local expected actual
            expected="$(awk '{print $1}' "${output_path}.sha256")"
            actual="$(shasum -a 256 "$output_path" | awk '{print $1}')"
            [ "$expected" = "$actual" ] || error "Checksum verification failed for ${filename}!"
        else
            warn "No checksum tool found, skipping checksum verification for ${filename}."
        fi
    else
        warn "Checksum file for ${filename} not available, skipping verification."
    fi
}

download_binaries() {
    local target="$1"
    local tag="$2"
    local ext=""

    case "$target" in
        *windows*) ext=".exe" ;;
    esac

    local tmp_dir
    tmp_dir="$(mktemp -d)"
    trap 'rm -rf "$tmp_dir"' EXIT

    download_binary "fff-mcp" "$target" "$tag" "$ext" "${tmp_dir}/fff-mcp-${target}${ext}"
    download_binary "fffq" "$target" "$tag" "$ext" "${tmp_dir}/fffq-${target}${ext}"

    mkdir -p "$INSTALL_DIR"
    mv "${tmp_dir}/fff-mcp-${target}${ext}" "${INSTALL_DIR}/fff-mcp${ext}"
    mv "${tmp_dir}/fffq-${target}${ext}" "${INSTALL_DIR}/fffq${ext}"
    chmod +x "${INSTALL_DIR}/fff-mcp${ext}" "${INSTALL_DIR}/fffq${ext}"

    if [ "$IS_UPDATE" != true ]; then
        success "Installed fff-mcp to ${INSTALL_DIR}/fff-mcp${ext}"
        success "Installed fffq to ${INSTALL_DIR}/fffq${ext}"
    fi
}

check_path() {
    case ":$PATH:" in
        *":${INSTALL_DIR}:"*) return 0 ;;
    esac

    warn "${INSTALL_DIR} is not in your PATH."
    echo ""
    echo "Add it to your shell profile:"
    echo ""

    local shell_name
    shell_name="$(basename "${SHELL:-bash}")"
    case "$shell_name" in
        zsh)
            echo "  echo 'export PATH=\"${INSTALL_DIR}:\$PATH\"' >> ~/.zshrc"
            echo "  source ~/.zshrc"
            ;;
        fish)
            echo "  fish_add_path ${INSTALL_DIR}"
            ;;
        *)
            echo "  echo 'export PATH=\"${INSTALL_DIR}:\$PATH\"' >> ~/.bashrc"
            echo "  source ~/.bashrc"
            ;;
    esac
    echo ""
}

print_setup_instructions() {
    local binary_path="${INSTALL_DIR}/fff-mcp"
    local fffq_path="${INSTALL_DIR}/fffq"
    local found_any=false

    echo ""
    success "FFF MCP Server and fffq installed successfully!"
    echo ""
    info "Setup with your AI coding assistant:"
    echo ""

    # Claude Code
    if command -v claude &>/dev/null; then
        found_any=true
        success "[Claude Code] detected"
        echo ""
        echo "Global (recommended):"
        echo "claude mcp add -s user fff -- ${binary_path}"
        echo ""
        echo "Or project-level .mcp.json (uses PATH):"
        echo ""
        print_json '{
  "mcpServers": {
    "fff": {
      "type": "stdio",
      "command": "fff-mcp",
      "args": []
    }
  }
}'
        echo ""
    fi

    # OpenCode
    if command -v opencode &>/dev/null; then
        found_any=true
        success "[OpenCode] detected"
        echo ""
        echo "Add to ~/.config/opencode/opencode.json:"
        echo ""
        print_json '{
  "mcp": {
    "fff": {
      "type": "local",
      "command": ["fff-mcp"],
      "enabled": true
    }
  }
}'
        echo ""
    fi

    # Codex
    if command -v codex &>/dev/null; then
        found_any=true
        success "[Codex] detected"
        echo ""
        echo "Use fffq directly for searches:"
        echo "fffq ensure"
        echo "fffq grep query"
        echo ""
    fi

    if [ "$found_any" = false ]; then
        echo "No AI coding assistants detected."
        echo ""
        echo "Binary path: ${binary_path}"
        echo ""
    fi

    echo "fff-mcp: ${binary_path}"
    echo "fffq:    ${fffq_path}"
    echo "Docs:    https://github.com/${REPO}"
    echo ""
    info "Tip: Add this to your CLAUDE.md or AGENTS.md to make AI use fffq for all searches:"
    echo "\""
    echo "Use fffq for file search operations. Run fffq ensure once per project."
    echo "\""


}

main() {
    local target
    target="$(detect_platform)"

    local existing_binary="${INSTALL_DIR}/fff-mcp"
    IS_UPDATE=false

    if [ -x "$existing_binary" ]; then
        IS_UPDATE=true
        info "Updating FFF MCP Server..."
    else
        info "Installing FFF MCP Server..."
    fi
    echo ""

    info "Detected platform: ${target}"

    local tag
    tag="$(get_latest_release_tag "$target")"

    download_binaries "$target" "$tag"

    if [ "$IS_UPDATE" = true ]; then
        echo ""
        success "FFF MCP Server updated to ${tag}!"
        echo ""
    else
        check_path
        print_setup_instructions
    fi
}

main
