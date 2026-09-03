#!/bin/sh
set -eu

REPOSITORY="ethanolivertroy/wut"
VERSION="${1:-latest}"

error() {
  printf 'wut: %s\n' "$*" >&2
  exit 1
}

require() {
  command -v "$1" >/dev/null 2>&1 || error "$1 is required"
}

download() {
  download_url="$1"
  download_destination="$2"

  if command -v curl >/dev/null 2>&1; then
    curl --proto '=https' --tlsv1.2 -fsSL "$download_url" -o "$download_destination"
  elif command -v wget >/dev/null 2>&1; then
    wget -qO "$download_destination" "$download_url"
  else
    error "curl or wget is required"
  fi
}

download_release_assets() {
  release_tag="$1"
  release_archive_name="$2"
  release_destination="$3"
  release_url_base="$4"

  if [ "${WUT_DISABLE_GH:-0}" != 1 ] && command -v gh >/dev/null 2>&1; then
    if [ -n "$release_tag" ]; then
      gh release download "$release_tag" --repo "$REPOSITORY" \
        --pattern "$release_archive_name" --pattern "$release_archive_name.sha256" \
        --dir "$release_destination" >/dev/null 2>&1 || true
    else
      gh release download --repo "$REPOSITORY" \
        --pattern "$release_archive_name" --pattern "$release_archive_name.sha256" \
        --dir "$release_destination" >/dev/null 2>&1 || true
    fi
    if [ -f "$release_destination/$release_archive_name" ] &&
      [ -f "$release_destination/$release_archive_name.sha256" ]; then
      return
    fi
  fi

  download "$release_url_base/$release_archive_name" \
    "$release_destination/$release_archive_name"
  download "$release_url_base/$release_archive_name.sha256" \
    "$release_destination/$release_archive_name.sha256"
}

detect_target() {
  case "$(uname -s)" in
    Darwin) os="apple-darwin" ;;
    Linux) os="unknown-linux-musl" ;;
    *) error "unsupported operating system: $(uname -s)" ;;
  esac

  case "$(uname -m)" in
    arm64 | aarch64) arch="aarch64" ;;
    x86_64 | amd64) arch="x86_64" ;;
    *) error "unsupported architecture: $(uname -m)" ;;
  esac

  printf '%s-%s\n' "$arch" "$os"
}

verify_checksum() {
  archive="$1"
  checksum_name="${archive##*/}.sha256"
  archive_dir="${archive%/*}"
  IFS=' ' read -r _ checked_name < "$archive.sha256" ||
    error "invalid checksum for ${archive##*/}"
  [ "$checked_name" = "${archive##*/}" ] ||
    [ "$checked_name" = "*${archive##*/}" ] ||
    error "invalid checksum for ${archive##*/}"

  if command -v sha256sum >/dev/null 2>&1; then
    (cd "$archive_dir" && sha256sum -c "$checksum_name") >/dev/null 2>&1 ||
      error "checksum verification failed"
  elif command -v shasum >/dev/null 2>&1; then
    (cd "$archive_dir" && shasum -a 256 -c "$checksum_name") >/dev/null 2>&1 ||
      error "checksum verification failed"
  else
    error "sha256sum or shasum is required"
  fi
}

valid_component() {
  case "$1" in
    "" | *[!0-9]* | 0[0-9]*) return 1 ;;
    *) return 0 ;;
  esac
}

valid_version() {
  value="$1"
  major="${value%%.*}"
  rest="${value#*.}"
  minor="${rest%%.*}"
  patch="${rest#*.}"
  [ "$rest" != "$value" ] &&
    [ "$patch" != "$rest" ] &&
    [ "$patch" = "${patch#*.}" ] &&
    valid_component "$major" &&
    valid_component "$minor" &&
    valid_component "$patch"
}

install_zsh_integration() {
  case "${SHELL:-}" in
    */zsh) ;;
    *) return 0 ;;
  esac

  [ -n "${HOME:-}" ] || error "HOME is not set; could not configure zsh integration"

  umask 077
  config_home="${XDG_CONFIG_HOME:-"$HOME/.config"}"
  wut_config="$config_home/wut"
  integration="$wut_config/zsh-integration.zsh"
  zsh_dir="${ZDOTDIR:-"$HOME"}"
  zshrc="$zsh_dir/.zshrc"
  marker='# >>> wut shell integration >>>'

  mkdir -p "$wut_config" "$zsh_dir"
  integration_staged="$integration.tmp.$$"
  printf '%s\n' \
    '# Managed by the wut installer. Re-run install.sh to refresh.' \
    "alias wut='noglob command wut'" \
    > "$integration_staged"
  mv -f "$integration_staged" "$integration"
  integration_staged=""

  if [ ! -f "$zshrc" ]; then
    : > "$zshrc"
  elif [ ! -f "$zshrc.wut-backup" ]; then
    cp -p "$zshrc" "$zshrc.wut-backup"
  fi

  if ! grep -Fqs "$marker" "$zshrc"; then
    {
      printf '\n%s\n' "$marker"
      printf '%s\n' "if [[ -r \"\${XDG_CONFIG_HOME:-\$HOME/.config}/wut/zsh-integration.zsh\" ]]; then"
      printf '%s\n' "  source \"\${XDG_CONFIG_HOME:-\$HOME/.config}/wut/zsh-integration.zsh\""
      printf '%s\n' 'fi'
      printf '%s\n' '# <<< wut shell integration <<<'
    } >> "$zshrc"
  fi

  printf 'configured managed zsh punctuation support in %s\n' "$zshrc" >&2
}

install_cerebras_key() {
  [ -z "${CEREBRAS_API_KEY:-}" ] || return 0
  [ -n "${HOME:-}" ] || return 0

  config_home="${XDG_CONFIG_HOME:-"$HOME/.config"}"
  credentials_dir="$config_home/wut"
  credentials="$credentials_dir/credentials"
  [ ! -s "$credentials" ] || return 0

  if [ ! -t 2 ] || [ ! -r /dev/tty ] || [ ! -w /dev/tty ]; then
    printf '\nRerun install.sh from a terminal to securely save your Cerebras API key.\n' >&2
    return
  fi

  printf '\nSave a Cerebras API key for wut now? [y/N] ' > /dev/tty
  IFS= read -r answer < /dev/tty || answer=''
  case "$answer" in
    y | Y | yes | YES | Yes) ;;
    *)
      printf 'You can set CEREBRAS_API_KEY or rerun the installer later.\n' >&2
      return
      ;;
  esac

  printf 'Cerebras API key (input hidden): ' > /dev/tty
  require stty
  tty_state="$(stty -g < /dev/tty)" || error "could not disable terminal echo"
  stty -echo < /dev/tty
  key=''
  IFS= read -r key < /dev/tty || true
  stty "$tty_state" < /dev/tty
  tty_state=''
  printf '\n' > /dev/tty
  [ -n "$key" ] || error "API key was empty"

  umask 077
  mkdir -p "$credentials_dir"
  chmod 700 "$credentials_dir"
  credentials_staged="$credentials.tmp.$$"
  printf '%s\n' "$key" > "$credentials_staged"
  chmod 600 "$credentials_staged"
  mv -f "$credentials_staged" "$credentials"
  credentials_staged=''
  key=''
  printf 'saved Cerebras API key to %s\n' "$credentials" >&2
}

main() {
  if [ -n "${WUT_INSTALL_DIR:-}" ]; then
    install_dir="$WUT_INSTALL_DIR"
  else
    [ -n "${HOME:-}" ] || error "HOME is not set"
    install_dir="$HOME/.local/bin"
  fi
  [ -n "$install_dir" ] || error "WUT_INSTALL_DIR must not be empty"

  require mktemp

  case "$VERSION" in
    latest) tag="" ;;
    v*) version="${VERSION#v}" ;;
    *) version="$VERSION" ;;
  esac
  if [ "$VERSION" != latest ]; then
    valid_version "$version" || error "invalid version: $VERSION"
    tag="v$version"
  fi

  temp_dir="$(mktemp -d)" || error "could not create a temporary directory"
  staged=""
  integration_staged=""
  credentials_staged=""
  tty_state=""
  cleanup() {
    [ -z "$tty_state" ] || stty "$tty_state" < /dev/tty 2>/dev/null || true
    [ -z "$staged" ] || rm -f "$staged"
    [ -z "$integration_staged" ] || rm -f "$integration_staged"
    [ -z "$credentials_staged" ] || rm -f "$credentials_staged"
    rm -rf "$temp_dir"
  }
  trap cleanup 0
  trap 'exit 1' HUP INT TERM

  script_dir=''
  script_parent=$(dirname "$0")
  if script_dir=$(CDPATH='' cd -- "$script_parent" 2>/dev/null && pwd); then
    :
  fi
  if [ "$VERSION" = latest ] &&
    [ -f "$script_dir/Cargo.toml" ] &&
    [ -f "$script_dir/src/main.rs" ]; then
    cargo_bin="${CARGO:-}"
    if [ -z "$cargo_bin" ] && [ -n "${HOME:-}" ] &&
      [ -x "${CARGO_HOME:-"$HOME/.cargo"}/bin/cargo" ]; then
      cargo_bin="${CARGO_HOME:-"$HOME/.cargo"}/bin/cargo"
    fi
    if [ -z "$cargo_bin" ]; then
      require cargo
      cargo_bin="cargo"
    fi
    printf 'building wut from %s...\n' "$script_dir" >&2
    CARGO_TARGET_DIR="$temp_dir/target" "$cargo_bin" build \
      --manifest-path "$script_dir/Cargo.toml" --locked --release
    source_binary="$temp_dir/target/release/wut"
    [ -f "$source_binary" ] || error "local build did not produce wut"
  else
    require uname
    require tar
    target="$(detect_target)"
    archive_name="wut-${target}.tar.gz"
    if [ -z "$tag" ]; then
      release_url="https://github.com/${REPOSITORY}/releases/latest/download"
    else
      release_url="https://github.com/${REPOSITORY}/releases/download/${tag}"
    fi

    archive="$temp_dir/$archive_name"
    unpacked="$temp_dir/unpacked"

    printf 'installing wut...\n' >&2
    download_release_assets "$tag" "$archive_name" "$temp_dir" "$release_url"
    verify_checksum "$archive"

    mkdir -p "$unpacked"
    tar -xzf "$archive" -C "$unpacked"
    [ -f "$unpacked/wut" ] || error "release archive does not contain wut"
    source_binary="$unpacked/wut"
  fi

  mkdir -p "$install_dir"
  [ ! -d "$install_dir/wut" ] || error "$install_dir/wut is a directory"
  staged="$install_dir/.wut-install.$$"
  cp "$source_binary" "$staged"
  chmod 755 "$staged"

  installed_version="$("$staged" --version 2>/dev/null)" ||
    error "installed binary could not run on this system"
  valid_version "$installed_version" ||
    error "downloaded binary returned an unexpected version"
  if [ -n "$tag" ] && [ "$installed_version" != "${tag#v}" ]; then
    error "downloaded wut $installed_version, expected wut ${tag#v}"
  fi

  mv -f "$staged" "$install_dir/wut"
  staged=""
  printf 'installed wut %s to %s\n' "$installed_version" "$install_dir/wut" >&2

  install_zsh_integration
  install_cerebras_key

  case ":${PATH:-}:" in
    *":$install_dir:"*) ;;
    *)
      printf '\nAdd wut to your PATH:\n\n' >&2
      printf '  export PATH="%s:%s"\n\n' "$install_dir" "\$PATH" >&2
      printf 'Then add that line to your shell configuration.\n' >&2
      ;;
  esac

}

main "$@"
