#!/usr/bin/env bash
#
# fetch-feature-decals.sh — populate tools/feature-decals/<family>/diffuse.tga
# from the upstream beyond-all-reason/mapfeatures repo. One diffuse per
# family; the FeatureDecalRegistry loads each into a layer of a
# texture_2d_array at app startup (Sprint 29 / ADR-046).
#
# Why a script (not build.rs): same as fetch-textures.sh + fetch-pymapconv.sh —
# build.rs network access is hostile to offline builds, CI without network,
# and reproducible packaging.
#
# Why no in-repo redistribution: upstream mapfeatures has AI_POLICY.md but
# NO LICENSE file. We do not vendor textures inside this repo or any built
# artifact; the user opts in by running this script locally. tools/feature-
# decals/ is gitignored.
#
# Idempotent: per-family files are skipped if already installed. Re-runs
# after `git pull` upstream pick up the freshest diffuse only when --refresh
# is passed.

set -euo pipefail

# ---- paths -----------------------------------------------------------------
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
DEST_ROOT="$REPO_ROOT/tools/feature-decals"

UPSTREAM_DIR="${BARME_MAPFEATURES_DIR:-$HOME/code/Beyond-All-Reason/mapfeatures}"
UPSTREAM_URL="https://github.com/beyond-all-reason/mapfeatures.git"
# Pin captured during the Sprint 29 audit (2026-05-21). If upstream
# moves, the script logs a warning but still uses whatever the clone
# resolves; the pinned SHA is informational, not enforced.
PINNED_COMMIT="3b791639210dcc4f89875fce712bde8f460d38e1"

# ---- catalog of family → upstream diffuse ----------------------------------
# Mirrors assets/mapfeatures_catalog.json::families.<key>.diffuse_texture for
# every family whose `source` is "mapfeatures" AND whose
# `diffuse_texture` is non-null. Families with `null` diffuse (kapok,
# rocks30, tombstone, xmascomwreck, geovent) fall back to the category
# glyph at runtime; they're intentionally not listed here.
#
# Format: "<family_key>:<filename_under_unittextures/>"
FAMILIES=(
    "ad0_aleppo2:ad0_aleppo2_1.tga"
    "ad0_banyan:ad0_banyan_1.tga"
    "ad0_cedar_atlas:ad0_cedar_atlas_1.tga"
    "ad0_fir:ad0_fir_1.tga"
    "ad0_senegal:ad0senegal_1.tga"
    "agorm_rock:rocks1a.tga"
    "allpinesb_ad0:allpinesb_ad0_diffuse.tga"
    "anemone:anemone1_Color.tga"
    "birchtree:birch_tree_03_1.tga"
    "cycas:fernsa.tga"
    "mushroom_orange:shroomorange.tga"
    "mushroom_purple:shroompurple.tga"
    "mushroom_tan:shroomtan.tga"
    "pdrock:pdrock_1.tga"
    "pedro:pedro-1.tga"
    "peyote:peyote-1.tga"
)

# ---- args ------------------------------------------------------------------
MODE='install'
while [ $# -gt 0 ]; do
    case "$1" in
        --check)
            MODE='check'
            ;;
        --refresh)
            MODE='refresh'
            ;;
        --help|-h)
            cat <<EOF
Usage: $0 [--check|--refresh]

Without flags:
  Idempotently copy upstream mapfeatures diffuse textures into
  $DEST_ROOT/<family>/diffuse.tga. Clones upstream into $UPSTREAM_DIR
  if not present. Skips families whose destination file already exists.

--check:
  Verify the upstream clone exists, every advertised diffuse is
  reachable inside it, and every installed file is non-empty. Does
  not download or copy. Exit non-zero on any miss.

--refresh:
  Force-overwrite every destination file from upstream (does NOT
  re-clone, only the destination is rewritten — use 'git -C
  $UPSTREAM_DIR pull' yourself if you want a fresh upstream).

env BARME_MAPFEATURES_DIR=<path>
  Override the upstream clone path. Default
  $HOME/code/Beyond-All-Reason/mapfeatures.

License note: upstream has no LICENSE file. tools/feature-decals/ is
gitignored — these textures never enter the BAR Map Editor repo or
any release artifact. The user opts in by running this script.

EOF
            exit 0
            ;;
        *)
            printf 'fetch-feature-decals: unknown flag: %s (try --help)\n' "$1" >&2
            exit 2
            ;;
    esac
    shift
done

# ---- ensure upstream clone -------------------------------------------------
ensure_upstream() {
    if [ -d "$UPSTREAM_DIR/.git" ]; then
        return 0
    fi
    if [ "$MODE" = 'check' ]; then
        printf 'fetch-feature-decals: upstream clone missing at %s\n' \
            "$UPSTREAM_DIR" >&2
        printf '  run "%s" without --check to clone it.\n' "$0" >&2
        exit 1
    fi
    printf 'fetch-feature-decals: cloning %s -> %s (shallow)\n' \
        "$UPSTREAM_URL" "$UPSTREAM_DIR"
    mkdir -p "$(dirname "$UPSTREAM_DIR")"
    git clone --depth 1 "$UPSTREAM_URL" "$UPSTREAM_DIR"
}

warn_on_pin_drift() {
    local head
    head="$(git -C "$UPSTREAM_DIR" rev-parse HEAD 2>/dev/null || echo unknown)"
    if [ "$head" != "$PINNED_COMMIT" ]; then
        printf '  note: upstream HEAD is %s (pin was %s); mapping may have drifted.\n' \
            "${head:0:8}" "${PINNED_COMMIT:0:8}"
    fi
}

ensure_upstream
warn_on_pin_drift

# ---- check mode ------------------------------------------------------------
if [ "$MODE" = 'check' ]; then
    fail=0
    for entry in "${FAMILIES[@]}"; do
        family="${entry%%:*}"
        diffuse="${entry#*:}"
        src="$UPSTREAM_DIR/unittextures/$diffuse"
        dst="$DEST_ROOT/$family/diffuse.tga"
        if [ ! -f "$src" ]; then
            printf '  [%s] UPSTREAM MISSING: %s\n' "$family" "$src" >&2
            fail=1
            continue
        fi
        if [ -f "$dst" ] && [ ! -s "$dst" ]; then
            printf '  [%s] LOCAL EMPTY: %s (re-run without --check)\n' \
                "$family" "$dst" >&2
            fail=1
            continue
        fi
        printf '  [%s] %s -> %s (upstream %s)\n' "$family" "$diffuse" \
            "$([ -f "$dst" ] && echo present || echo absent)" \
            "$([ -f "$src" ] && echo present || echo absent)"
    done
    if [ "$fail" -ne 0 ]; then
        printf 'fetch-feature-decals: --check failed\n' >&2
        exit 1
    fi
    printf 'fetch-feature-decals: --check ok\n'
    exit 0
fi

# ---- install / refresh -----------------------------------------------------
mkdir -p "$DEST_ROOT"

installed=0
skipped=0
missing=0

for entry in "${FAMILIES[@]}"; do
    family="${entry%%:*}"
    diffuse="${entry#*:}"
    src="$UPSTREAM_DIR/unittextures/$diffuse"
    dst_dir="$DEST_ROOT/$family"
    dst="$dst_dir/diffuse.tga"

    if [ ! -f "$src" ]; then
        printf '  [%s] UPSTREAM MISSING %s\n' "$family" "$src" >&2
        missing=$((missing + 1))
        continue
    fi

    if [ -f "$dst" ] && [ "$MODE" != 'refresh' ]; then
        printf '  [%s] already installed, skipping\n' "$family"
        skipped=$((skipped + 1))
        continue
    fi

    mkdir -p "$dst_dir"
    cp "$src" "$dst"
    printf '  [%s] installed from %s (%d bytes)\n' \
        "$family" "$diffuse" "$(stat -c%s "$dst" 2>/dev/null || stat -f%z "$dst")"
    installed=$((installed + 1))
done

printf '\nfetch-feature-decals: %d installed, %d skipped, %d missing.\n' \
    "$installed" "$skipped" "$missing"

if [ "$missing" -gt 0 ]; then
    printf '  %d families lack an upstream diffuse — runtime falls back\n' \
        "$missing" >&2
    printf '  to the category glyph for those.\n' >&2
fi
