default:
    @just --list

[private]
check-version version:
    @[[ "{{version}}" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo "version must be X.Y.Z, got '{{version}}'" >&2; exit 1; }

bump version: (check-version version)
    sed -i 's/^version = "[^"]*"/version = "{{version}}"/' Cargo.toml herdr-plugin.toml
    cargo check --quiet

release version: (check-version version)
    #!/usr/bin/env bash
    set -euo pipefail
    fail() { echo "release: $1" >&2; exit 1; }
    [ "$(git branch --show-current)" = main ] || fail "switch to main first"
    [ -z "$(git status --porcelain)" ] || fail "commit or stash your changes first"
    git fetch --quiet origin main
    git merge-base --is-ancestor origin/main HEAD || fail "main is behind origin/main, pull first"
    ! git rev-parse -q --verify "refs/tags/v{{version}}" >/dev/null || fail "tag v{{version}} already exists"
    just bump {{version}}
    cargo test --quiet
    git add Cargo.toml Cargo.lock herdr-plugin.toml
    git commit -m "Release {{version}}"
    git push origin main
    git tag -a "v{{version}}" -m "herdr-groups {{version}}"
    git push origin "v{{version}}"
