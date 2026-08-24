#!/bin/sh
set -eu

repo=$(CDPATH='' cd -- "$(dirname "$0")" && pwd)

if [ "${WUT_SKIP_CARGO_INSTALL:-0}" != 1 ]; then
    if ! command -v cargo >/dev/null 2>&1; then
        printf '%s\n' 'wut: cargo is required (Rust 1.88 or newer)' >&2
        exit 1
    fi
    cargo install --path "$repo" --locked --force
fi

case "${SHELL:-}" in
    */zsh) ;;
    *)
        printf '%s\n' 'wut installed; shell integration skipped because the login shell is not zsh'
        exit 0
        ;;
esac

umask 077
config_home=${XDG_CONFIG_HOME:-"$HOME/.config"}
wut_config="$config_home/wut"
integration="$wut_config/zsh-integration.zsh"
zshrc=${ZDOTDIR:-"$HOME"}/.zshrc
marker='# >>> wut shell integration >>>'

mkdir -p "$wut_config"
temporary="$integration.tmp.$$"
trap 'rm -f "$temporary"' EXIT HUP INT TERM
printf '%s\n' \
    '# Managed by the wut installer. Re-run install.sh to refresh.' \
    "alias wut='noglob command wut'" \
    > "$temporary"
mv -f "$temporary" "$integration"
trap - EXIT HUP INT TERM

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

printf '%s\n' 'wut installed with managed zsh punctuation support'
printf '%s\n' 'Open a new terminal, or source ~/.zshrc once, before using the new shell integration.'
