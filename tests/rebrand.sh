#!/bin/sh
set -eu

# Some assertions intentionally search for literal shell variables in source files.

contains() {
  pattern="$1"
  shift
  grep -F "$pattern" "$@" >/dev/null
}

rejects() {
  pattern="$1"
  shift
  if grep -F "$pattern" "$@" >/dev/null; then
    printf 'unexpected legacy brand %s in %s\n' "$pattern" "$*" >&2
    exit 1
  fi
}

fail() {
  printf '%s\n' "$*" >&2
  exit 1
}

contains 'name = "wut"' Cargo.toml
contains 'repository = "https://github.com/ethanolivertroy/wut"' Cargo.toml
contains 'name = "wut"' Cargo.lock
contains 'REPOSITORY="ethanolivertroy/wut"' install.sh
contains 'WUT_INSTALL_DIR' install.sh
# shellcheck disable=SC2016
contains 'archive_name="wut-${target}.tar.gz"' install.sh
contains 'Usage:' src/cli.rs
contains '  wut [QUESTION...]' src/cli.rs
contains 'https://github.com/ethanolivertroy/wut/issues' src/error.rs
contains 'https://api.github.com/repos/ethanolivertroy/wut/releases/latest' src/upgrade.rs
contains '"wut/", env!("CARGO_PKG_VERSION")' src/upgrade.rs
contains 'join("wut/config.json")' src/config.rs
contains 'join("wut/sessions")' src/state.rs
contains 'join("wut/update.json")' src/update_check.rs
contains 'dist/wut-' .github/workflows/release.yml
# shellcheck disable=SC2016
contains 'target/$target/release/wut' .github/workflows/release.yml
contains 'for archive in wut-*.tar.gz' .github/workflows/release.yml
contains 'sh tests/rebrand.sh' .github/workflows/ci.yml
contains 'sudo apt-get install --yes shellcheck zsh' .github/workflows/ci.yml
contains "alias wut='noglob command wut'" install.sh tests/install.sh
contains '# >>> wut shell integration >>>' install.sh tests/install.sh
contains 'git clone git@github.com:ethanolivertroy/wut.git' README.md
contains './install.sh' README.md
contains '<img alt="wut"' README.md
contains '$ wut' README.md
contains 'WUT_INSTALL_DIR' README.md
contains 'Benjamin Akar' README.md
contains '3d1cd5d90603586aeba9ba47612d0c0625a04d3a' README.md
contains 'Copyright (c) 2026 Ethan Troy' LICENSE
rejects 'Copyright (c) 2026 Benjamin Akar' LICENSE
contains '      - ".github/workflows/**"' .github/workflows/ci.yml
contains 'shellcheck install.sh tests/install.sh tests/rebrand.sh' .github/workflows/ci.yml

for trigger in '      - README.md' '      - LICENSE' '      - "assets/**"'; do
    count="$(grep -Fxc -- "$trigger" .github/workflows/ci.yml || true)"
    [ "$count" -eq 2 ] || fail "CI must trigger on '$trigger' for pull requests and main pushes"
done

rejects 'https://github.com/benja/ask' Cargo.toml install.sh README.md src/error.rs src/upgrade.rs tests/install.sh
rejects 'name = "ask-cli"' Cargo.toml Cargo.lock
rejects 'name = "ask"' Cargo.toml Cargo.lock
# shellcheck disable=SC2016
rejects 'target/$target/release/ask' .github/workflows/release.yml
rejects 'dist/ask-' .github/workflows/release.yml
rejects 'ASK_INSTALL_DIR' README.md install.sh src/upgrade.rs tests/install.sh
rejects '$ ask' README.md
rejects '`ask' README.md
rejects '/ask/' README.md

python3 - <<'PY'
from hashlib import sha256
from pathlib import Path
from struct import unpack

legacy = {
    "assets/dark-banner.png": "9fc4aad7190ffa16d7b7c451dddc7163e175228b4f8b7ef435ff1ac637b261ec",
    "assets/light-banner.png": "80fd2e6490a3be7250e9ebb8b143b6242b821cb16368a129ffddbbf905019db5",
}
for name, old_digest in legacy.items():
    data = Path(name).read_bytes()
    assert data[:8] == b"\x89PNG\r\n\x1a\n", f"{name} is not a PNG"
    assert unpack(">II", data[16:24]) == (1925, 817), f"{name} dimensions changed"
    assert sha256(data).hexdigest() != old_digest, f"{name} still contains the ask banner"
PY

printf 'rebrand contract passed\n'
