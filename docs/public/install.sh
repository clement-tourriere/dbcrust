#!/bin/sh
# shellcheck shell=dash
# shellcheck disable=SC2039  # local is non-POSIX
#
# DBCrust Install Script
# 
# Install DBCrust standalone binaries on Unix systems
# Based on the pattern used by uv and other modern CLI tools

set -u

APP_NAME="dbcrust"
APP_VERSION="${DBCRUST_VERSION:-latest}"
GITHUB_REPO="clement-tourriere/dbcrust"

# Configuration
if [ -n "${DBCRUST_INSTALLER_GITHUB_BASE_URL:-}" ]; then
    INSTALLER_BASE_URL="$DBCRUST_INSTALLER_GITHUB_BASE_URL"
else
    INSTALLER_BASE_URL="https://github.com"
fi

if [ -n "${DBCRUST_DOWNLOAD_URL:-}" ]; then
    ARTIFACT_DOWNLOAD_URL="$DBCRUST_DOWNLOAD_URL"
elif [ "$APP_VERSION" = "latest" ]; then
    ARTIFACT_DOWNLOAD_URL="${INSTALLER_BASE_URL}/${GITHUB_REPO}/releases/latest/download"
else
    ARTIFACT_DOWNLOAD_URL="${INSTALLER_BASE_URL}/${GITHUB_REPO}/releases/download/${APP_VERSION}"
fi

# Logging configuration
PRINT_VERBOSE="${DBCRUST_PRINT_VERBOSE:-0}"
PRINT_QUIET="${DBCRUST_PRINT_QUIET:-0}"
NO_MODIFY_PATH="${DBCRUST_NO_MODIFY_PATH:-0}"

# Some Linux distributions don't set HOME
get_home() {
    if [ -n "${HOME:-}" ]; then
        echo "$HOME"
    elif [ -n "${USER:-}" ]; then
        getent passwd "$USER" | cut -d: -f6
    else
        getent passwd "$(id -un)" | cut -d: -f6
    fi
}

get_home_expression() {
    if [ -n "${HOME:-}" ]; then
        # shellcheck disable=SC2016
        echo '$HOME'
    elif [ -n "${USER:-}" ]; then
        getent passwd "$USER" | cut -d: -f6
    else
        getent passwd "$(id -un)" | cut -d: -f6
    fi
}

INFERRED_HOME=$(get_home)
INFERRED_HOME_EXPRESSION=$(get_home_expression)

usage() {
    cat <<EOF
dbcrust-installer.sh

The installer for DBCrust (Unix systems: macOS, Linux)

For Windows, use: https://clement-tourriere.github.io/dbcrust/install.ps1

This script detects what platform you're on and fetches an appropriate archive from
${INSTALLER_BASE_URL}/${GITHUB_REPO}/releases/
then unpacks the binaries and installs them to:

    \$DBCRUST_INSTALL_DIR (if set)
    \$XDG_BIN_HOME (if set)
    \$XDG_DATA_HOME/../bin (if set)
    \$HOME/.local/bin (default)

It will then add that dir to PATH by adding the appropriate line to your shell profiles.

USAGE:
    dbcrust-installer.sh [OPTIONS]

OPTIONS:
    -v, --verbose
            Enable verbose output

    -q, --quiet
            Disable progress output

        --no-modify-path
            Don't configure the PATH environment variable

    -h, --help
            Print help information

ENVIRONMENT VARIABLES:
    DBCRUST_VERSION
            Specify the version to install (default: latest)

    DBCRUST_INSTALL_DIR
            Override the installation directory

    DBCRUST_NO_MODIFY_PATH
            Don't modify PATH (same as --no-modify-path)
EOF
}

get_architecture() {
    local _ostype
    local _cputype
    _ostype="$(uname -s)"
    _cputype="$(uname -m)"
    local _clibtype="gnu"

    if [ "$_ostype" = Linux ]; then
        if ldd --version 2>&1 | grep -q 'musl'; then
            _clibtype="musl"
        else
            _clibtype="gnu"
        fi
    fi

    case "$_ostype" in
        Linux)
            _ostype="unknown-linux-$_clibtype"
            ;;
        Darwin)
            _ostype="apple-darwin"
            
            # Handle Rosetta on Apple Silicon
            if [ "$_cputype" = x86_64 ]; then
                if sysctl hw.optional.arm64 2> /dev/null | grep -q ': 1'; then
                    _cputype=aarch64
                fi
            elif [ "$_cputype" = arm64 ]; then
                _cputype=aarch64
            fi
            ;;
        *)
            err "unsupported OS: $_ostype"
            ;;
    esac

    case "$_cputype" in
        i386 | i486 | i686 | i786 | x86)
            _cputype=i686
            ;;
        x86_64 | x86-64 | x64 | amd64)
            _cputype=x86_64
            ;;
        aarch64 | arm64)
            _cputype=aarch64
            ;;
        *)
            err "unsupported CPU type: $_cputype"
            ;;
    esac

    echo "${_cputype}-${_ostype}"
}

download_and_install() {
    downloader --check
    need_cmd uname
    need_cmd mktemp
    need_cmd chmod
    need_cmd mkdir
    need_cmd rm
    need_cmd tar

    # Parse arguments
    for arg in "$@"; do
        case "$arg" in
            --help)
                usage
                exit 0
                ;;
            --quiet)
                PRINT_QUIET=1
                ;;
            --verbose)
                PRINT_VERBOSE=1
                ;;
            --no-modify-path)
                NO_MODIFY_PATH=1
                ;;
            *)
                OPTIND=1
                if [ "${arg%%--*}" = "" ]; then
                    err "unknown option $arg"
                fi
                while getopts :hvq sub_arg "$arg"; do
                    case "$sub_arg" in
                        h)
                            usage
                            exit 0
                            ;;
                        v)
                            PRINT_VERBOSE=1
                            ;;
                        q)
                            PRINT_QUIET=1
                            ;;
                        *)
                            err "unknown option -$OPTARG"
                            ;;
                    esac
                done
                ;;
        esac
    done

    local _arch
    _arch="$(get_architecture)" || return 1
    
    say "detected platform: $_arch"
    
    local _archive_name="dbcrust-${_arch}.tar.gz"
    local _url="$ARTIFACT_DOWNLOAD_URL/$_archive_name"
    
    local _tmp_dir
    _tmp_dir="$(ensure mktemp -d)" || return 1
    local _archive_file="$_tmp_dir/$_archive_name"

    say "downloading DBCrust $_arch" 1>&2
    say_verbose "  from $_url" 1>&2
    say_verbose "  to $_archive_file" 1>&2

    ensure mkdir -p "$_tmp_dir"

    if ! downloader "$_url" "$_archive_file"; then
        say "failed to download $_url"
        say "this may be a network error, or the release may not have binaries for your platform"
        say "please check ${INSTALLER_BASE_URL}/${GITHUB_REPO}/releases for available downloads"
        exit 1
    fi

    # Extract archive
    say_verbose "extracting archive to $_tmp_dir"
    ensure tar xzf "$_archive_file" -C "$_tmp_dir"
    
    # Install binaries
    install_binaries "$_tmp_dir" "dbcrust dbc"
    local _retval=$?
    
    # Cleanup
    ignore rm -rf "$_tmp_dir"
    
    return "$_retval"
}

install_binaries() {
    local _src_dir="$1"
    local _bins="$2"
    local _install_dir
    local _env_script_path
    local _env_script_path_expr
    local _install_dir_expr

    # Determine install directory
    if [ -n "${DBCRUST_INSTALL_DIR:-}" ]; then
        _install_dir="$DBCRUST_INSTALL_DIR"
        _install_dir_expr="$(replace_home "$_install_dir")"
    elif [ -n "${XDG_BIN_HOME:-}" ]; then
        _install_dir="$XDG_BIN_HOME"
        _install_dir_expr="$(replace_home "$_install_dir")"
    elif [ -n "${XDG_DATA_HOME:-}" ]; then
        _install_dir="$XDG_DATA_HOME/../bin"
        _install_dir_expr="$(replace_home "$_install_dir")"
    elif [ -n "${INFERRED_HOME:-}" ]; then
        _install_dir="$INFERRED_HOME/.local/bin"
        _install_dir_expr="$INFERRED_HOME_EXPRESSION/.local/bin"
    else
        err "could not determine installation directory"
    fi

    _env_script_path="$_install_dir/env"
    _env_script_path_expr="$(replace_home "$_env_script_path")"
    
    say "installing to $_install_dir_expr"
    ensure mkdir -p "$_install_dir"

    # Copy binaries
    for _bin_name in $_bins; do
        local _bin_path="$_src_dir/$_bin_name"
        if [ -f "$_bin_path" ]; then
            ensure cp "$_bin_path" "$_install_dir/"
            ensure chmod +x "$_install_dir/$_bin_name"
            say "  $_bin_name"
        else
            say_verbose "  $_bin_name not found in archive (skipping)"
        fi
    done

    say "installation complete!"

    # Check if install dir is already in PATH
    case ":$PATH:" in
        *:"$_install_dir":*) 
            NO_MODIFY_PATH=1 
            say "$_install_dir_expr is already in PATH"
            ;;
        *) ;;
    esac

    # Configure PATH
    if [ "$NO_MODIFY_PATH" = "0" ]; then
        add_to_path "$_install_dir_expr" "$_env_script_path" "$_env_script_path_expr"
    fi
}

add_to_path() {
    local _install_dir_expr="$1"
    local _env_script_path="$2"
    local _env_script_path_expr="$3"

    # Create env script if it doesn't exist
    if [ ! -f "$_env_script_path" ]; then
        say_verbose "creating $_env_script_path_expr"
        write_env_script "$_install_dir_expr" "$_env_script_path"
    fi

    # Add to shell profiles
    local _updated=0
    for _profile in ".profile" ".bashrc" ".bash_profile" ".bash_login" ".zshrc" ".zshenv"; do
        local _profile_path="$INFERRED_HOME/$_profile"
        if [ -f "$_profile_path" ]; then
            if ! grep -F ". \"$_env_script_path_expr\"" "$_profile_path" > /dev/null 2>&1; then
                say_verbose "adding DBCrust to $_profile"
                echo "" >> "$_profile_path"
                echo ". \"$_env_script_path_expr\"" >> "$_profile_path"
                _updated=1
            fi
        fi
    done

    # Fish shell support
    local _fish_dir="$INFERRED_HOME/.config/fish/conf.d"
    if [ -d "$INFERRED_HOME/.config/fish" ]; then
        ensure mkdir -p "$_fish_dir"
        local _fish_file="$_fish_dir/dbcrust.fish"
        if [ ! -f "$_fish_file" ]; then
            say_verbose "adding DBCrust to fish shell"
            write_fish_env_script "$_install_dir_expr" "$_fish_file"
            _updated=1
        fi
    fi

    if [ "$_updated" = "1" ]; then
        say ""
        say "To add DBCrust to your PATH, either restart your shell or run:"
        say ""
        say "    source $_env_script_path_expr"
    fi
}

write_env_script() {
    local _install_dir_expr="$1"
    local _env_script_path="$2"
    cat > "$_env_script_path" <<EOF
#!/bin/sh
# DBCrust environment setup
case ":\${PATH}:" in
    *:"$_install_dir_expr":*)
        ;;
    *)
        export PATH="$_install_dir_expr:\$PATH"
        ;;
esac
EOF
}

write_fish_env_script() {
    local _install_dir_expr="$1"
    local _fish_file="$2"
    cat > "$_fish_file" <<EOF
# DBCrust environment setup for fish shell
if not contains "$_install_dir_expr" \$PATH
    set -x PATH "$_install_dir_expr" \$PATH
end
EOF
}

replace_home() {
    local _str="$1"
    if [ -n "${HOME:-}" ]; then
        echo "$_str" | sed "s,$HOME,\$HOME,"
    else
        echo "$_str"
    fi
}

# Utility functions
say() {
    if [ "$PRINT_QUIET" = "0" ]; then
        echo "$1"
    fi
}

say_verbose() {
    if [ "$PRINT_VERBOSE" = "1" ]; then
        echo "$1"
    fi
}

err() {
    if [ "$PRINT_QUIET" = "0" ]; then
        echo "ERROR: $1" >&2
    fi
    exit 1
}

need_cmd() {
    if ! check_cmd "$1"; then
        err "need '$1' (command not found)"
    fi
}

check_cmd() {
    command -v "$1" > /dev/null 2>&1
}

ensure() {
    if ! "$@"; then
        err "command failed: $*"
    fi
}

ignore() {
    "$@"
}

downloader() {
    local _dld
    if [ "$1" = --check ]; then
        need_cmd "curl"
        return
    fi
    
    if check_cmd curl; then
        curl -fsSL "$1" -o "$2"
    else
        err "curl is required to download DBCrust"
    fi
}

# Main execution
download_and_install "$@" || exit 1