#!/bin/sh
set -eu

repo=$(CDPATH='' cd -- "$(dirname "$0")/.." && pwd)
root=$(mktemp -d "${TMPDIR:-/tmp}/wut-install-test.XXXXXX")
trap 'rm -rf "$root"' EXIT HUP INT TERM

home="$root/home"
config="$root/config"
bin="$root/bin"
args="$root/args"
mkdir -p "$home" "$config" "$bin"

cat > "$bin/wut" <<'SH'
#!/bin/sh
printf '%s\n' "$@" > "$WUT_TEST_ARGS"
SH
chmod 700 "$bin/wut"

run_installer() {
    HOME="$home" \
    XDG_CONFIG_HOME="$config" \
    PATH="$bin:$PATH" \
    SHELL=/bin/zsh \
    WUT_SKIP_CARGO_INSTALL=1 \
    sh "$repo/install.sh"
}

run_installer
run_installer

integration="$config/wut/zsh-integration.zsh"
test -f "$integration"
test -f "$home/.zshrc"

marker_count=$(awk '/^# >>> wut shell integration >>>$/{count++} END{print count+0}' "$home/.zshrc")
test "$marker_count" -eq 1

HOME="$home" \
XDG_CONFIG_HOME="$config" \
ZDOTDIR="$home" \
PATH="$bin:$PATH" \
WUT_TEST_ARGS="$args" \
zsh -ic 'wut what is kubernetes?' >/dev/null

expected="$root/expected"
printf '%s\n' what is 'kubernetes?' > "$expected"
cmp "$expected" "$args"

printf '%s\n' 'zsh install smoke passed'
