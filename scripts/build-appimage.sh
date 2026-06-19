#!/usr/bin/env bash
# Build a relocatable Linux AppImage for BAR Map Editor.
# Sprint 33 (T6 / ADR-049 / NFR-Portability).
#
# Produces ./BARMapEditor-x86_64.AppImage bundling:
#   - the release `barme-app` binary
#   - tools/pymapconv/*       (vendored sidecar — required to build maps)
#   - tools/compressonator/*  (BC1/BC3 DDS encoder)
#   - assets/                 (fixtures + mapfeatures catalog)
#   - tools/textures/         ONLY when INCLUDE_TEXTURES=1
#
# Pitfall #6 (AppImage size): the stock texture pack is ~100 MB. By
# default we EXCLUDE it to keep the image small; the app falls back to
# the "no slots found under tools/textures/" empty state and the pack is
# fetched on first use via scripts/fetch-textures.sh. Set
# INCLUDE_TEXTURES=1 to bundle it for a fully-offline image.
#
# The relocation trick: AppRun exports BARME_ROOT="$APPDIR", which the
# app's repo_root() honours (see crates/barme-app/src/main.rs), so the
# bundled tools/ + assets/ resolve at runtime regardless of mount point.
#
# Usage:
#   ./scripts/build-appimage.sh                 # lean image
#   INCLUDE_TEXTURES=1 ./scripts/build-appimage.sh
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

BIN="target/release/barme-app"
APPDIR="target/appimage/BARMapEditor.AppDir"
OUT="BARMapEditor-x86_64.AppImage"

die() { printf 'build-appimage: %s\n' "$*" >&2; exit 1; }

[ -x "$BIN" ] || die "release binary missing at $BIN (run: cargo build --release -p barme-app)"
[ -d tools/pymapconv ] || die "tools/pymapconv missing (run: ./scripts/fetch-pymapconv.sh)"
[ -d tools/compressonator ] || die "tools/compressonator missing (run: ./scripts/fetch-compressonator.sh)"

# --- fetch appimagetool if not already cached ------------------------------
TOOL="${APPIMAGETOOL:-}"
if [ -z "$TOOL" ]; then
    TOOL="target/appimage/appimagetool-x86_64.AppImage"
    if [ ! -x "$TOOL" ]; then
        mkdir -p target/appimage
        echo "fetching appimagetool…"
        wget -q -O "$TOOL" \
            "https://github.com/AppImage/appimagetool/releases/download/continuous/appimagetool-x86_64.AppImage" \
            || die "failed to download appimagetool"
        chmod +x "$TOOL"
    fi
fi

# --- assemble the AppDir ---------------------------------------------------
rm -rf "$APPDIR"
mkdir -p "$APPDIR/usr/bin" "$APPDIR/tools" "$APPDIR/assets"

cp "$BIN" "$APPDIR/usr/bin/barme-app"
cp -r tools/pymapconv "$APPDIR/tools/"
cp -r tools/compressonator "$APPDIR/tools/"
cp -r assets/. "$APPDIR/assets/"

if [ "${INCLUDE_TEXTURES:-0}" = "1" ]; then
    if [ -d tools/textures ]; then
        echo "bundling tools/textures/ (INCLUDE_TEXTURES=1)…"
        cp -r tools/textures "$APPDIR/tools/"
    else
        echo "warning: INCLUDE_TEXTURES=1 but tools/textures/ is absent — skipping"
    fi
else
    echo "skipping tools/textures/ (set INCLUDE_TEXTURES=1 to bundle the stock pack)"
fi

# --- AppRun: set BARME_ROOT so repo_root() finds the bundle ----------------
cat > "$APPDIR/AppRun" <<'APPRUN'
#!/usr/bin/env bash
HERE="$(dirname "$(readlink -f "${0}")")"
export BARME_ROOT="$HERE"
export PATH="$HERE/usr/bin:$PATH"
exec "$HERE/usr/bin/barme-app" "$@"
APPRUN
chmod +x "$APPDIR/AppRun"

# --- .desktop + icon (appimagetool requires both) --------------------------
cat > "$APPDIR/barme-app.desktop" <<'DESKTOP'
[Desktop Entry]
Type=Application
Name=BAR Map Editor
Exec=barme-app
Icon=barme-app
Categories=Graphics;Development;
Terminal=false
DESKTOP

# Minimal 1×1 PNG icon if the repo doesn't ship one yet. appimagetool
# only needs *a* valid icon named to match the .desktop Icon= key.
ICON_SRC=""
for cand in assets/barme-app.png assets/icon.png; do
    [ -f "$cand" ] && ICON_SRC="$cand" && break
done
if [ -n "$ICON_SRC" ]; then
    cp "$ICON_SRC" "$APPDIR/barme-app.png"
else
    # 1×1 transparent PNG, base64-decoded.
    base64 -d > "$APPDIR/barme-app.png" <<'PNG'
iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==
PNG
fi

# --- build ----------------------------------------------------------------
# ARCH is required by appimagetool when run headless. --no-appstream skips
# the metadata validation that needs network/appstreamcli.
echo "running appimagetool…"
ARCH=x86_64 "$TOOL" --no-appstream "$APPDIR" "$OUT"

echo "built: $OUT ($(du -h "$OUT" | cut -f1))"
