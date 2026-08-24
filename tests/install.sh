#!/bin/sh
set -eu

repo=$(CDPATH='' cd -- "$(dirname "$0")/.." && pwd)
root=$(mktemp -d "${TMPDIR:-/tmp}/wut-release-install-test.XXXXXX")
trap 'rm -rf "$root"' EXIT HUP INT TERM
real_home=${HOME:?}
cargo_home=${CARGO_HOME:-"$real_home/.cargo"}
rustup_home=${RUSTUP_HOME:-"$real_home/.rustup"}
WUT_DISABLE_GH=1
export WUT_DISABLE_GH

version=$(awk -F '"' '/^version = / { print $2; exit }' "$repo/Cargo.toml")
case "$(uname -s)" in
  Darwin) os="apple-darwin" ;;
  Linux) os="unknown-linux-musl" ;;
  *) exit 0 ;;
esac
case "$(uname -m)" in
  arm64 | aarch64) arch="aarch64" ;;
  x86_64 | amd64) arch="x86_64" ;;
  *) exit 0 ;;
esac
archive_name="wut-${arch}-${os}.tar.gz"

home="$root/home"
config="$root/config"
install_dir="$root/install dir"
args="$root/args"
mkdir -p "$root/package" "$root/release" "$root/bin" "$home" "$config" "$install_dir"

for invalid_version in 01.2.3 1.2.3-beta.1 1..2 v+1.0.0; do
  if WUT_INSTALL_DIR="$install_dir" sh "$repo/install.sh" "$invalid_version" \
    > "$root/invalid-stdout" 2> "$root/invalid-stderr"; then
    exit 1
  fi
  grep -F "wut: invalid version: $invalid_version" "$root/invalid-stderr" >/dev/null
done

cat > "$root/package/wut" <<EOF
#!/bin/sh
if [ "\${1:-}" = --version ] || [ "\${1:-}" = -V ]; then
  printf '%s\n' '$version'
else
  : "\${WUT_TEST_ARGS:?}"
  printf '%s\n' "\$@" > "\$WUT_TEST_ARGS"
fi
EOF
chmod 755 "$root/package/wut"
cp "$repo/README.md" "$repo/LICENSE" "$root/package/"
tar -C "$root/package" -czf "$root/release/$archive_name" wut README.md LICENSE

cat > "$install_dir/wut" <<'EOF'
#!/bin/sh
printf '0.0.0\n'
EOF
chmod 755 "$install_dir/wut"

if command -v sha256sum >/dev/null 2>&1; then
  (cd "$root/release" && sha256sum "$archive_name" > "$archive_name.sha256")
else
  (cd "$root/release" && shasum -a 256 "$archive_name" > "$archive_name.sha256")
fi

cat > "$root/bin/curl" <<'EOF'
#!/bin/sh
set -eu
[ "${WUT_TEST_CURL_MUST_NOT_RUN:-0}" != 1 ] || exit 99
destination=""
url=""
while [ "$#" -gt 0 ]; do
  case "$1" in
    -o)
      shift
      destination="$1"
      ;;
    https://*) url="$1" ;;
  esac
  shift
done
[ -n "$destination" ]
[ -n "$url" ]
case "$url" in
  "https://github.com/ethanolivertroy/wut/releases/download/v$WUT_TEST_VERSION/"*) ;;
  *) exit 1 ;;
esac
cp "$WUT_TEST_RELEASE/${url##*/}" "$destination"
EOF
chmod 755 "$root/bin/curl"

cat > "$root/bin/gh" <<'EOF'
#!/bin/sh
set -eu
[ "${1:-}" = release ]
[ "${2:-}" = download ]
shift 2
tag=""
repository=""
destination=""
pattern_count=0
pattern_one=""
pattern_two=""
while [ "$#" -gt 0 ]; do
  case "$1" in
    --repo)
      shift
      repository="$1"
      ;;
    --pattern)
      shift
      pattern_count=$((pattern_count + 1))
      if [ "$pattern_count" -eq 1 ]; then
        pattern_one="$1"
      else
        pattern_two="$1"
      fi
      ;;
    --dir)
      shift
      destination="$1"
      ;;
    v*) tag="$1" ;;
    *) exit 1 ;;
  esac
  shift
done
[ "$repository" = ethanolivertroy/wut ]
[ "$tag" = "v$WUT_TEST_VERSION" ]
[ "$pattern_count" -eq 2 ]
[ "$pattern_one" = "$WUT_TEST_ARCHIVE" ]
[ "$pattern_two" = "$WUT_TEST_ARCHIVE.sha256" ]
[ -n "$destination" ]
cp "$WUT_TEST_RELEASE/$pattern_one" "$destination/$pattern_one"
cp "$WUT_TEST_RELEASE/$pattern_two" "$destination/$pattern_two"
printf '%s\n' "$repository $tag" > "$WUT_TEST_GH_LOG"
EOF
chmod 755 "$root/bin/gh"

cat > "$root/bin/cursor-agent" <<'EOF'
#!/bin/sh
exit 0
EOF
chmod 755 "$root/bin/cursor-agent"

printf '%s\n' 'export WUT_EXISTING_ZSHRC=1' > "$home/.zshrc"
cp "$home/.zshrc" "$root/original-zshrc"

run_installer() {
  HOME="$home" \
  XDG_CONFIG_HOME="$config" \
  ZDOTDIR="$home" \
  PATH="$root/bin:/usr/bin:/bin" \
  SHELL=/bin/zsh \
  WUT_TEST_RELEASE="$root/release" \
  WUT_TEST_VERSION="$version" \
  WUT_INSTALL_DIR="$install_dir" \
  sh "$repo/install.sh" "$version" \
  > "$root/stdout" 2> "$root/stderr"
}

run_installer

test ! -s "$root/stdout"
test -x "$install_dir/wut"
test "$("$install_dir/wut" --version)" = "$version"
grep -F "installed wut $version to $install_dir/wut" "$root/stderr" >/dev/null
if grep -F 'No supported coding agent was found on PATH.' "$root/stderr" >/dev/null; then
  exit 1
fi

mkdir "$root/gh-install"
WUT_DISABLE_GH=0 \
HOME="$home" \
PATH="$root/bin:$PATH" \
SHELL=/bin/sh \
WUT_TEST_ARCHIVE="$archive_name" \
WUT_TEST_CURL_MUST_NOT_RUN=1 \
WUT_TEST_GH_LOG="$root/gh-log" \
WUT_TEST_RELEASE="$root/release" \
WUT_TEST_VERSION="$version" \
WUT_INSTALL_DIR="$root/gh-install" \
sh "$repo/install.sh" "$version" \
  > "$root/gh-stdout" 2> "$root/gh-stderr"
test -x "$root/gh-install/wut"
test "$("$root/gh-install/wut" --version)" = "$version"
grep -Fx "ethanolivertroy/wut v$version" "$root/gh-log" >/dev/null

integration="$config/wut/zsh-integration.zsh"
backup="$home/.zshrc.wut-backup"
test -f "$integration"
test -f "$backup"
cmp "$root/original-zshrc" "$backup"
grep -Fx "alias wut='noglob command wut'" "$integration" >/dev/null
if grep -E '(^|[[:space:]])(setopt|unsetopt)[[:space:]]' "$integration" >/dev/null; then
  printf '%s\n' 'managed integration must not change global zsh options' >&2
  exit 1
fi

printf '%s\n' 'preserve this first backup' >> "$backup"
cp "$backup" "$root/first-backup"
run_installer
cmp "$root/first-backup" "$backup"

marker_count=$(awk '/^# >>> wut shell integration >>>$/{count++} END{print count+0}' "$home/.zshrc")
test "$marker_count" -eq 1

HOME="$home" \
XDG_CONFIG_HOME="$config" \
ZDOTDIR="$home" \
PATH="$install_dir:$root/bin:$PATH" \
WUT_TEST_ARGS="$args" \
zsh -ic '[[ -o nomatch ]] && wut what is kubernetes? && [[ -o nomatch ]]' >/dev/null

printf '%s\n' what is 'kubernetes?' > "$root/expected-args"
cmp "$root/expected-args" "$args"

if HOME="$home" XDG_CONFIG_HOME="$config" ZDOTDIR="$home" \
  zsh -ic 'printf "%s\n" wut-definitely-unmatched?' \
  > "$root/nomatch-stdout" 2> "$root/nomatch-stderr"; then
  printf '%s\n' 'zsh nomatch was unexpectedly disabled globally' >&2
  exit 1
fi
grep -F 'no matches found: wut-definitely-unmatched?' "$root/nomatch-stderr" >/dev/null

mkdir "$root/corrupt-release" "$root/corrupt-install"
cp "$root/release/$archive_name"* "$root/corrupt-release/"
printf 'corrupt' >> "$root/corrupt-release/$archive_name"
cp "$root/package/wut" "$root/corrupt-install/wut"
if PATH="$root/bin:$PATH" \
  WUT_TEST_RELEASE="$root/corrupt-release" \
  WUT_TEST_VERSION="$version" \
  WUT_INSTALL_DIR="$root/corrupt-install" \
  sh "$repo/install.sh" "$version" \
  > "$root/corrupt-stdout" 2> "$root/corrupt-stderr"; then
  exit 1
fi
test "$("$root/corrupt-install/wut" --version)" = "$version"
grep -F "wut: checksum verification failed" "$root/corrupt-stderr" >/dev/null

mkdir "$root/mismatch-package" "$root/mismatch-release" "$root/mismatch-install"
cat > "$root/mismatch-package/wut" <<'EOF'
#!/bin/sh
printf '9.9.9\n'
EOF
chmod 755 "$root/mismatch-package/wut"
cp "$repo/README.md" "$repo/LICENSE" "$root/mismatch-package/"
tar -C "$root/mismatch-package" -czf "$root/mismatch-release/$archive_name" wut README.md LICENSE
if command -v sha256sum >/dev/null 2>&1; then
  (cd "$root/mismatch-release" && sha256sum "$archive_name" > "$archive_name.sha256")
else
  (cd "$root/mismatch-release" && shasum -a 256 "$archive_name" > "$archive_name.sha256")
fi
cp "$root/package/wut" "$root/mismatch-install/wut"

if PATH="$root/bin:$PATH" \
  WUT_TEST_RELEASE="$root/mismatch-release" \
  WUT_TEST_VERSION="$version" \
  WUT_INSTALL_DIR="$root/mismatch-install" \
  sh "$repo/install.sh" "$version" \
  > "$root/mismatch-stdout" 2> "$root/mismatch-stderr"; then
  exit 1
fi
test "$("$root/mismatch-install/wut" --version)" = "$version"
grep -F "wut: downloaded wut 9.9.9, expected wut $version" "$root/mismatch-stderr" >/dev/null

mkdir "$root/local-install"
HOME="$home" \
SHELL=/bin/sh \
CARGO_HOME="$cargo_home" \
RUSTUP_HOME="$rustup_home" \
WUT_INSTALL_DIR="$root/local-install" \
sh "$repo/install.sh" > "$root/local-stdout" 2> "$root/local-stderr"
test -x "$root/local-install/wut"
test "$("$root/local-install/wut" --version)" = "$version"
grep -F "building wut from $repo" "$root/local-stderr" >/dev/null
grep -F "installed wut $version to $root/local-install/wut" "$root/local-stderr" >/dev/null

printf '%s\n' 'release and local installer tests passed'
