#!/usr/bin/env bash
#
# fetch-textures.sh — vendor the 16-slot CC0 starter texture pack under
# tools/textures/<NN-slot-name>/. Sources are ambientCG `_1K-PNG.zip`
# archives; the script downloads each, verifies its sha256 against a
# pinned value, extracts only the `*_Color.png` (diffuse) and
# `*_NormalGL.png` (OpenGL-convention normal) members, and writes a
# per-slot `meta.toml` descriptor.
#
# Why a script (not build.rs): same rationale as fetch-pymapconv.sh
# (ADR-011) — build.rs network access is hostile to offline builds, CI
# without network, and reproducible packaging.
#
# Why _1K-PNG (not _1K-JPG): JPG normal maps are silently wrong. The
# 4:2:0 chroma subsampling that ambientCG applies to JPGs destroys the
# X/Y vector data encoded in the red/green channels of a tangent-space
# normal map. We accept the larger network footprint (~16 MB/zip × 16
# slots ≈ 256 MB) for vector-correct normals.
#
# Why _NormalGL (not _NormalDX): Recoil's SMF fragment shader builds the
# TBN basis assuming OpenGL tangent space (+Y up); see
# RecoilEngine/cont/base/springcontent/shaders/GLSL/SMFFragProg.glsl:276-278
# and docs/research/source-audit-2026-05-18/FINDINGS.md §7.4. ambientCG
# `_NormalGL` is the matching convention; no Y-flip needed at fetch time.
#
# License: every bundled asset is CC0-1.0 per
# https://docs.ambientcg.com/license/. The `.sd7` output the editor
# eventually emits inherits no licensing obligations from this pack.
#
# Idempotent: re-running with all slots present and meta.toml in place
# skips every download. A `--check` flag does HEAD-only URL verification
# without downloading (use this for CI; ambientCG renumbers + reprocesses
# assets occasionally — without the check, a year-later run would
# silently 404 some slots).

set -euo pipefail

# ---- paths -----------------------------------------------------------------
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
DEST_ROOT="$REPO_ROOT/tools/textures"

# ---- palette ---------------------------------------------------------------
# Parallel arrays keyed by slot index 0..15. Edit here if a slot ever
# needs a different ambientCG asset; rerun the script and the meta.toml
# updates automatically.
#
# Format per slot (one column per array):
#   SLOT_NAMES    — kebab-case slot name (becomes directory suffix)
#   SLOT_LABELS   — human-readable label (goes into meta.toml `name`)
#   SLOT_BIOMES   — biome group (Earth-Temperate / Arid / Snow-Alpine /
#                    Alien-Industrial)
#   SLOT_ASSETS   — ambientCG asset ID (e.g. Grass002)
#   SLOT_SHAS     — pinned sha256 of the _1K-PNG.zip
#
# To re-pin after a known-deliberate asset bump, set SLOT_SHAS to the
# literal "BOOTSTRAP" for that slot; the script will compute + print
# the new sha and exit non-zero so you paste it back in.

SLOT_NAMES=(
    grass-meadow
    forest-floor-pine
    dirt-mud-cracked
    rocky-outcrop-grey
    desert-sand-dunes
    dry-rock-sandstone
    dusty-hardpan-clay
    arid-gravel-pebbles
    alpine-snow-powder
    jagged-ice-frozen
    cold-bare-rock
    frozen-permafrost
    dark-volcanic-lava
    rusty-metal-plates
    clean-metal-floor
    alien-organic-creep
)

SLOT_LABELS=(
    "Grass meadow"
    "Forest floor (pine)"
    "Dirt / mud (cracked)"
    "Rocky outcrop (grey)"
    "Desert sand dunes"
    "Dry rock (sandstone)"
    "Dusty hardpan clay"
    "Arid gravel pebbles"
    "Alpine snow powder"
    "Jagged ice (frozen)"
    "Cold bare rock"
    "Frozen permafrost"
    "Dark volcanic lava rock"
    "Rusty metal plates"
    "Clean metal floor"
    "Alien organic creep"
)

SLOT_BIOMES=(
    Earth-Temperate
    Earth-Temperate
    Earth-Temperate
    Earth-Temperate
    Arid
    Arid
    Arid
    Arid
    Snow-Alpine
    Snow-Alpine
    Snow-Alpine
    Snow-Alpine
    Alien-Industrial
    Alien-Industrial
    Alien-Industrial
    Alien-Industrial
)

SLOT_ASSETS=(
    Grass002
    Ground037
    Ground042
    Rock030
    Ground027
    Rock023
    Ground033
    Gravel018
    Snow004
    Snow006
    Rock029
    Ground035
    Rock035
    Metal009
    Metal003
    Moss001
)

SLOT_SHAS=(
    3a51690e1fd2fd6672f8964737091eb52444c1ed90f58f16bf79a50d2e5aa517
    cbd75f0660870b3299a68c4fe7fd54efb3951cf992d4295961209619eb284c47
    2a2e34f68981519f81a6b8cc982a68b51669185971cefa038d8055a02d7c7443
    d3e0dc55fc46b093631f4d0009c934003c601e69df9aa4ba41a43db3807056ee
    03e41b00d17ed28c235cccaa6aba74015b49961e2fe657c75c59b55ccf8fd050
    4d6a7d7a36bf6dfbe4fe456cc748bd875c2bb95c3135aff0785f86928ea3b0d2
    b8dbd0105b204863b9b1b6d9e2656fa4f7f77398eebfdef644349140b6da3a72
    aceb088008927d82085629a7d765abb4c2d704fdbbf5d185669757c4bdfd9616
    ed08bcfdcc0a57e815dba6fc64429d7498773be41381fdf35efc5b771e286472
    f993019a7e2a59bfdf3ddeb9b4e692bc2a734a5fb71f17581234f24065dbdac5
    b8d3517cc73bf317a32ad1c3ca8bb4e4c7b8aed0eab30ee24a00c623374a8764
    ed88469c201a41f82776d8651d947a0ea00a9412fca7a2261aa79dc162ffb257
    e745b558d754962ac44162ccee8805d7dba84ecdc428a543c3e552bcb28f8b85
    ec44086a3bee042418ac2b38a74c8cedfa8313d942bf08c1c91be4ef63c8a97f
    b664c3a54bb5e5fc879bb0f69f0f51e2bfd7925c014ca076c779912a72ef2e50
    e3745c52f895acf88ce3f28fa83aebc0b7371b68378022f813dcc16ffb0aa8c8
)

SLOT_COUNT="${#SLOT_NAMES[@]}"

# Defaults baked into every slot's meta.toml. The splat tool UI (D5)
# may override per-slot at the project level; these are the "fresh
# install" values. See ADR-025 + the Comet Catcher Remake reference
# (texScales = {0.004, 0.007, 0.003, 0.0018}) for why 0.02 is a
# placeholder rather than a target — real BAR maps run much smaller.
DEFAULT_TEX_SCALE='0.02'
DEFAULT_TEX_MULT='1.0'

# ---- args ------------------------------------------------------------------
MODE='install'
while [ $# -gt 0 ]; do
    case "$1" in
        --check)
            MODE='check'
            ;;
        --help|-h)
            cat <<EOF
Usage: $0 [--check]

Without flags:
  Downloads, sha256-verifies, and extracts the 16-slot starter texture
  pack into $DEST_ROOT/. Idempotent — slots already present are skipped.

With --check:
  HEAD-checks each URL and verifies on-disk slots have the expected
  files. Does not download anything. Exits 0 if all URLs return 200 and
  every present slot has {diffuse.{png,jpg}, normal.png, meta.toml};
  non-zero otherwise. Use this in CI to detect ambientCG asset rot.

EOF
            exit 0
            ;;
        *)
            printf 'fetch-textures: unknown flag: %s (try --help)\n' "$1" >&2
            exit 2
            ;;
    esac
    shift
done

# ---- helpers ---------------------------------------------------------------
have() { command -v "$1" >/dev/null 2>&1; }

sha256_of() {
    if have sha256sum; then
        sha256sum "$1" | awk '{print $1}'
    elif have shasum; then
        shasum -a 256 "$1" | awk '{print $1}'
    else
        printf 'fetch-textures: need sha256sum or shasum\n' >&2
        exit 1
    fi
}

url_for() {
    local asset="$1"
    printf 'https://ambientcg.com/get?file=%s_1K-PNG.zip' "$asset"
}

slot_dir_for() {
    local idx="$1"
    printf '%s/%02d-%s' "$DEST_ROOT" "$idx" "${SLOT_NAMES[$idx]}"
}

slot_installed() {
    # A slot counts as installed iff its meta.toml exists, its
    # normal.png exists, and at least one diffuse.{png,jpg} exists.
    local dir="$1"
    [ -f "$dir/meta.toml" ] || return 1
    [ -f "$dir/normal.png" ] || return 1
    [ -f "$dir/diffuse.png" ] || [ -f "$dir/diffuse.jpg" ] || return 1
    return 0
}

write_meta() {
    local dir="$1" idx="$2"
    cat > "$dir/meta.toml" <<EOF
# Auto-generated by scripts/fetch-textures.sh. Hand-edits will be
# overwritten on the next run. See ADR-025 + ADR-027.
slot = $idx
name = "${SLOT_LABELS[$idx]}"
biome = "${SLOT_BIOMES[$idx]}"
source = "https://ambientcg.com/view?id=${SLOT_ASSETS[$idx]}"
license = "CC0-1.0"
default_tex_scale = $DEFAULT_TEX_SCALE
default_tex_mult = $DEFAULT_TEX_MULT
EOF
}

# ---- check mode ------------------------------------------------------------
if [ "$MODE" = 'check' ]; then
    have curl || { printf 'fetch-textures: --check needs curl\n' >&2; exit 1; }
    fail=0
    printf 'fetch-textures: --check — HEAD probe of %d ambientCG URLs\n' \
        "$SLOT_COUNT"
    for (( i = 0; i < SLOT_COUNT; i++ )); do
        url="$(url_for "${SLOT_ASSETS[$i]}")"
        code="$(curl --silent --head --location --max-time 30 \
            -o /dev/null -w '%{http_code}' "$url" || echo '???')"
        if [ "$code" = '200' ]; then
            printf '  [%02d %s] %s -> 200 OK\n' \
                "$i" "${SLOT_ASSETS[$i]}" "${SLOT_NAMES[$i]}"
        else
            printf '  [%02d %s] %s -> %s FAIL\n' \
                "$i" "${SLOT_ASSETS[$i]}" "${SLOT_NAMES[$i]}" "$code" >&2
            fail=1
        fi
        # On-disk check: if the slot is "installed", verify its files
        # are still present. Don't fail if the slot has never been
        # installed — only fail on partial installs.
        dir="$(slot_dir_for "$i")"
        if [ -e "$dir/meta.toml" ] && ! slot_installed "$dir"; then
            printf '  [%02d %s] meta.toml present but artifacts incomplete\n' \
                "$i" "${SLOT_ASSETS[$i]}" >&2
            fail=1
        fi
    done
    if [ "$fail" -ne 0 ]; then
        printf 'fetch-textures: --check failed\n' >&2
        exit 1
    fi
    printf 'fetch-textures: --check ok\n'
    exit 0
fi

# ---- install mode ----------------------------------------------------------
have curl || { printf 'fetch-textures: need curl\n' >&2; exit 1; }
have unzip || { printf 'fetch-textures: need unzip\n' >&2; exit 1; }

mkdir -p "$DEST_ROOT"

bootstrap_seen=0
installed=0
skipped=0

for (( i = 0; i < SLOT_COUNT; i++ )); do
    asset="${SLOT_ASSETS[$i]}"
    expected_sha="${SLOT_SHAS[$i]}"
    url="$(url_for "$asset")"
    dir="$(slot_dir_for "$i")"
    label="[$(printf '%02d' "$i") $asset]"

    if slot_installed "$dir"; then
        printf '%s %s already installed at %s, skipping\n' \
            "$label" "${SLOT_NAMES[$i]}" "$dir"
        skipped=$((skipped + 1))
        continue
    fi

    mkdir -p "$dir"

    tmpdir="$(mktemp -d -t fetch-textures.XXXXXX)"
    trap 'rm -rf "$tmpdir"' EXIT
    zip="$tmpdir/$asset.zip"

    printf '%s downloading %s\n' "$label" "$url"
    curl --fail --silent --show-error --location --retry 3 --max-time 300 \
        --output "$zip" "$url"

    got_sha="$(sha256_of "$zip")"

    if [ "$expected_sha" = 'BOOTSTRAP' ]; then
        printf '%s BOOTSTRAP sha256 = %s\n' "$label" "$got_sha"
        bootstrap_seen=1
        rm -rf "$tmpdir"
        trap - EXIT
        continue
    fi

    if [ "$got_sha" != "$expected_sha" ]; then
        printf '%s sha256 mismatch\n' "$label" >&2
        printf '  expected: %s\n' "$expected_sha" >&2
        printf '  got:      %s\n' "$got_sha" >&2
        printf '  url:      %s\n' "$url" >&2
        printf '  hint:     ambientCG may have re-processed this asset.\n' >&2
        printf '            Inspect the new ZIP, then update SLOT_SHAS[%d]\n' \
            "$i" >&2
        printf '            to %s in scripts/fetch-textures.sh.\n' "$got_sha" >&2
        rm -rf "$tmpdir"
        trap - EXIT
        exit 1
    fi
    printf '%s sha256 ok (%s)\n' "$label" "$got_sha"

    # Extract the two members we care about and discard everything else
    # (the ZIP also contains AmbientOcclusion, Roughness, Displacement,
    # NormalDX, USDC, blend, mtlx, tres, preview PNG — all out of scope
    # for the splat pipeline).
    extract_dir="$tmpdir/extract"
    mkdir -p "$extract_dir"
    unzip -q -d "$extract_dir" "$zip"

    # Color (diffuse). PNG is the only variant we ever see in
    # _1K-PNG.zip, but glob loosely in case ambientCG changes format.
    color_src="$(find "$extract_dir" -maxdepth 2 -type f \
        \( -iname '*_Color.png' -o -iname '*_Color.jpg' \) | head -n 1)"
    if [ -z "$color_src" ]; then
        printf '%s no *_Color.{png,jpg} in ZIP\n' "$label" >&2
        rm -rf "$tmpdir"
        trap - EXIT
        exit 1
    fi
    color_ext="${color_src##*.}"
    cp "$color_src" "$dir/diffuse.${color_ext,,}"

    # NormalGL (OpenGL tangent space). PNG mandatory — reject if
    # someone ever ships a JPG normal here, even though that should
    # not happen for _1K-PNG.zip.
    normal_src="$(find "$extract_dir" -maxdepth 2 -type f \
        -iname '*_NormalGL.png' | head -n 1)"
    if [ -z "$normal_src" ]; then
        printf '%s no *_NormalGL.png in ZIP (required, no JPG fallback)\n' \
            "$label" >&2
        rm -rf "$tmpdir"
        trap - EXIT
        exit 1
    fi
    cp "$normal_src" "$dir/normal.png"

    # Ensure any pre-existing alt-extension diffuse from a previous
    # asset bump is cleared.
    if [ "${color_ext,,}" = 'png' ] && [ -f "$dir/diffuse.jpg" ]; then
        rm -f "$dir/diffuse.jpg"
    fi
    if [ "${color_ext,,}" = 'jpg' ] && [ -f "$dir/diffuse.png" ]; then
        rm -f "$dir/diffuse.png"
    fi

    write_meta "$dir" "$i"

    rm -rf "$tmpdir"
    trap - EXIT

    printf '%s installed at %s\n' "$label" "$dir"
    installed=$((installed + 1))
done

printf '\nfetch-textures: %d installed, %d already present.\n' \
    "$installed" "$skipped"

if [ "$bootstrap_seen" -ne 0 ]; then
    printf 'fetch-textures: BOOTSTRAP slot(s) detected — paste the printed\n' >&2
    printf '  sha256 values back into SLOT_SHAS in this script and re-run.\n' >&2
    exit 1
fi
