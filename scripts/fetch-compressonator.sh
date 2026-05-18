#!/usr/bin/env bash
#
# fetch-compressonator.sh — vendor AMD's CompressonatorCLI under
# tools/compressonator/. Required by PyMapConv on Linux (the --linux flag
# shells out to `CompressonatorCLI` by name via os.system; see ADR-014
# and upstream src/pymapconv.py lines 828, 1032).
#
# Idempotent: re-running with the artifact present and matching SHA256 is
# a no-op apart from re-asserting the camelcase symlink the Rust driver
# prepends to PATH.
#
# Why a script (not build.rs): same rationale as fetch-pymapconv.sh
# (ADR-011) — build.rs network access is hostile to offline builds, CI
# without network, and reproducible packaging.

set -euo pipefail

# ---- pinned upstream -------------------------------------------------------
VERSION='V4.5.52'
EXPECTED_SHA256='70c9cdb27a19875df03766f349864951a749a44c0f5c001c33903944465f6b97'

# ---- paths -----------------------------------------------------------------
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
DEST="$REPO_ROOT/tools/compressonator"

# ---- arch / asset ----------------------------------------------------------
UNAME_S="$(uname -s)"
UNAME_M="$(uname -m)"
case "$UNAME_S-$UNAME_M" in
    Linux-x86_64)
        ASSET="compressonatorcli-4.5.52-Linux.tar.gz"
        ;;
    *)
        printf 'fetch-compressonator: unsupported platform %s/%s\n' \
            "$UNAME_S" "$UNAME_M" >&2
        printf '  Stage 0 ships linux-amd64 only. Windows support is\n' >&2
        printf '  tracked in docs/DECISIONS.md ADR-014.\n' >&2
        exit 1
        ;;
esac
URL="https://github.com/GPUOpen-Tools/compressonator/releases/download/${VERSION}/${ASSET}"

# ---- helpers ---------------------------------------------------------------
have() { command -v "$1" >/dev/null 2>&1; }

sha256_of() {
    if have sha256sum; then
        sha256sum "$1" | awk '{print $1}'
    elif have shasum; then
        shasum -a 256 "$1" | awk '{print $1}'
    else
        printf 'fetch-compressonator: need sha256sum or shasum\n' >&2
        exit 1
    fi
}

# ---- main ------------------------------------------------------------------
mkdir -p "$DEST"

# Entry point is the launcher script. It sets LD_LIBRARY_PATH and execs the
# bundled compressonatorcli-bin ELF.
ENTRY="$DEST/compressonatorcli"
if [ -x "$ENTRY" ]; then
    printf 'fetch-compressonator: %s already present, skipping download\n' "$DEST"
else
    TMPDIR="$(mktemp -d)"
    trap 'rm -rf "$TMPDIR"' EXIT
    TARBALL="$TMPDIR/$ASSET"

    printf 'fetch-compressonator: downloading %s\n' "$URL"
    curl --fail --location --retry 3 --output "$TARBALL" "$URL"

    GOT="$(sha256_of "$TARBALL")"
    if [ "$GOT" != "$EXPECTED_SHA256" ]; then
        printf 'fetch-compressonator: sha256 mismatch\n' >&2
        printf '  expected: %s\n' "$EXPECTED_SHA256" >&2
        printf '  got:      %s\n' "$GOT" >&2
        exit 1
    fi
    printf 'fetch-compressonator: sha256 ok (%s)\n' "$GOT"

    tar -xzf "$TARBALL" -C "$DEST" --strip-components=1
fi

# Restore exec bits and assert the camelcase alias PyMapConv looks for.
# Upstream ships `compressonatorcli` (lowercase); PyMapConv invokes
# `CompressonatorCLI` (camelcase) literally. The Rust driver prepends
# tools/compressonator/ to PATH, so a sibling symlink is the simplest fix.
chmod +x "$ENTRY"
if [ -x "$DEST/compressonatorcli-bin" ]; then
    chmod +x "$DEST/compressonatorcli-bin"
fi
ln -sf compressonatorcli "$DEST/CompressonatorCLI"

printf '\nfetch-compressonator: ready.\n'
printf '  Entry point: %s\n' "$ENTRY"
printf '  CamelCase alias (for PyMapConv): %s/CompressonatorCLI\n' "$DEST"
printf '  Rust driver prepends %s to PATH when invoking pymapconv.\n' "$DEST"
