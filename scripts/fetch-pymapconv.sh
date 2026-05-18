#!/usr/bin/env bash
#
# fetch-pymapconv.sh — vendor a pinned PyMapConv prebuilt release under
# tools/pymapconv/. Idempotent: re-running with the artifact already
# present and the right SHA256 is a no-op apart from a chmod sweep.
#
# Why a script (not build.rs): build.rs network access breaks offline /
# CI / packaging builds. This is a one-shot dev/install step, not a
# library dep. See docs/DECISIONS.md ADR-011.
#
# Why a frozen tag (not git clone of upstream HEAD): the upstream repo
# is maintenance-mode (last commit 2024-10-30). Tracking a moving target
# would let breakage land without us noticing.

set -euo pipefail

# ---- pinned upstream -------------------------------------------------------
VERSION='v0.6.3'
EXPECTED_SHA256='7040c68f7a7f401e8e7613b4f51df8a8147f66ac24b717a91888fbf15d980a73'

# ---- paths -----------------------------------------------------------------
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
DEST="$REPO_ROOT/tools/pymapconv"

# ---- arch / asset ----------------------------------------------------------
UNAME_S="$(uname -s)"
UNAME_M="$(uname -m)"
case "$UNAME_S-$UNAME_M" in
    Linux-x86_64)
        ASSET="pymapconv.${VERSION}.linux-amd64.tar.gz"
        ;;
    *)
        printf 'fetch-pymapconv: unsupported platform %s/%s\n' \
            "$UNAME_S" "$UNAME_M" >&2
        printf '  Stage 0 ships linux-amd64 only. Windows support is\n' >&2
        printf '  tracked in docs/DECISIONS.md ADR-011.\n' >&2
        exit 1
        ;;
esac
URL="https://github.com/Beherith/springrts_smf_compiler/releases/download/${VERSION}/${ASSET}"

# ---- helpers ---------------------------------------------------------------
have() { command -v "$1" >/dev/null 2>&1; }

sha256_of() {
    if have sha256sum; then
        sha256sum "$1" | awk '{print $1}'
    elif have shasum; then
        shasum -a 256 "$1" | awk '{print $1}'
    else
        printf 'fetch-pymapconv: need sha256sum or shasum\n' >&2
        exit 1
    fi
}

# ---- main ------------------------------------------------------------------
mkdir -p "$DEST"

# Entry point is a PyInstaller-bundled ELF (no python runtime required).
# Bundled tools (Compressonator-derived dragon-dxt1/dragon-dxt5, magick)
# ship alongside under tools/, despite what upstream README says about
# installing them yourself.
ENTRY="$DEST/pymapconv"
if [ -x "$ENTRY" ]; then
    printf 'fetch-pymapconv: %s already present, skipping download\n' "$DEST"
else
    TMPDIR="$(mktemp -d)"
    trap 'rm -rf "$TMPDIR"' EXIT
    TARBALL="$TMPDIR/$ASSET"

    printf 'fetch-pymapconv: downloading %s\n' "$URL"
    curl --fail --location --retry 3 --output "$TARBALL" "$URL"

    GOT="$(sha256_of "$TARBALL")"
    if [ "$GOT" != "$EXPECTED_SHA256" ]; then
        printf 'fetch-pymapconv: sha256 mismatch\n' >&2
        printf '  expected: %s\n' "$EXPECTED_SHA256" >&2
        printf '  got:      %s\n' "$GOT" >&2
        exit 1
    fi
    printf 'fetch-pymapconv: sha256 ok (%s)\n' "$GOT"

    tar -xzf "$TARBALL" -C "$DEST" --strip-components=1
fi

# Restore exec bits in case the tar lost them.
chmod +x "$ENTRY"
if [ -d "$DEST/tools" ]; then
    find "$DEST/tools" -maxdepth 1 -type f -exec chmod +x {} +
fi

printf '\nfetch-pymapconv: ready.\n'
printf '  Entry point: %s\n' "$ENTRY"
printf '  Note: %s --help is broken upstream (argparse format crash);\n' "$ENTRY"
printf '        run with no args to see the Qt GUI instead, or pass\n'
printf '        flags directly. See docs/DECISIONS.md ADR-011.\n'
