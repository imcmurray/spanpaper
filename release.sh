#!/usr/bin/env bash
# release.sh — cut a new spanpaper release end-to-end.
#
# Usage:
#   ./release.sh 0.3.0           # cut version 0.3.0 (pkgrel=1)
#   ./release.sh 0.3.0 2         # cut 0.3.0-2 (repackage of same source)
#   ./release.sh 0.3.0 -y        # skip the confirmation prompt
#
# Pipeline:
#   1. Sanity: clean tree, on main, up to date with origin.
#   2. Bump version in Cargo.toml and both contrib/PKGBUILD*.
#   3. cargo build --release  (smoke test before tagging).
#   4. Commit the bump, tag vX.Y.Z, push main + tag.
#   5. Stage the prebuilt binary tarball into dist/.
#   6. Update PKGBUILD sha256sums against the live tarballs.
#   7. makepkg against PKGBUILD-bin → spanpaper-bin-X.Y.Z-N-x86_64.pkg.tar.zst.
#   8. gh release create — uploads both assets.
#   9. Commit the sha256 updates, push.
#
# After this: on a fresh Arch box,
#   curl -LO <release>/spanpaper-bin-X.Y.Z-N-x86_64.pkg.tar.zst
#   sudo pacman -U spanpaper-bin-X.Y.Z-N-x86_64.pkg.tar.zst

set -euo pipefail

# ---- pretty printing ---------------------------------------------------------

if [[ -t 1 ]]; then
    C_OK=$'\033[1;32m'; C_WARN=$'\033[1;33m'; C_ERR=$'\033[1;31m'
    C_DIM=$'\033[2m';   C_BOLD=$'\033[1m';    C_RST=$'\033[0m'
else
    C_OK=""; C_WARN=""; C_ERR=""; C_DIM=""; C_BOLD=""; C_RST=""
fi
step() { printf "%s==>%s %s\n" "$C_BOLD$C_OK" "$C_RST$C_BOLD" "$*$C_RST"; }
warn() { printf "%s!!%s %s\n"   "$C_WARN"             "$C_RST" "$*" >&2; }
die()  { printf "%sxx%s %s\n"   "$C_ERR"              "$C_RST" "$*" >&2; exit 1; }

# ---- args --------------------------------------------------------------------

VERSION=""
PKGREL=1
SKIP_CONFIRM=0

for arg in "$@"; do
    case "$arg" in
        -y|--yes)   SKIP_CONFIRM=1 ;;
        -h|--help)  sed -n '2,/^$/p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
        -*)         die "unknown flag: $arg" ;;
        *)
            if [[ -z "$VERSION" ]]; then
                VERSION="$arg"
            else
                PKGREL="$arg"
            fi
            ;;
    esac
done

[[ -n "$VERSION" ]] || die "usage: $0 <version> [pkgrel] [-y]"
[[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] \
    || die "version must be MAJOR.MINOR.PATCH (got: $VERSION)"
[[ "$PKGREL" =~ ^[1-9][0-9]*$ ]] \
    || die "pkgrel must be a positive integer (got: $PKGREL)"

TAG="v$VERSION"
REPO_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$REPO_DIR"

# ---- preflight ---------------------------------------------------------------

step "preflight"

for bin in cargo makepkg fakeroot curl sha256sum gh git tar sed awk; do
    command -v "$bin" >/dev/null || die "$bin not on PATH"
done

[[ "$(git rev-parse --abbrev-ref HEAD)" == "main" ]] \
    || die "not on main (run from main; you're on $(git rev-parse --abbrev-ref HEAD))"

[[ -z "$(git status --porcelain)" ]] \
    || die "working tree not clean — commit or stash first"

git fetch --quiet origin
LOCAL="$(git rev-parse @)"
REMOTE="$(git rev-parse @{u})"
[[ "$LOCAL" == "$REMOTE" ]] \
    || die "local main is not in sync with origin (pull/push first)"

if git rev-parse --verify --quiet "refs/tags/$TAG" >/dev/null; then
    die "tag $TAG already exists locally"
fi

if gh release view "$TAG" >/dev/null 2>&1; then
    die "release $TAG already exists on GitHub"
fi

REPO_SLUG="$(gh repo view --json nameWithOwner -q .nameWithOwner)"

cat <<EOF

  ${C_BOLD}About to release:${C_RST}
    version:    $VERSION
    pkgrel:     $PKGREL
    tag:        $TAG
    repo:       $REPO_SLUG
    from HEAD:  $(git log -1 --oneline)

EOF

if [[ "$SKIP_CONFIRM" -ne 1 ]]; then
    read -r -p "Proceed? [y/N] " yn
    [[ "$yn" =~ ^[Yy]$ ]] || die "aborted"
fi

# ---- step 2: bump versions ---------------------------------------------------

step "bumping Cargo.toml and PKGBUILDs to $VERSION-$PKGREL"

# Cargo.toml: only the [package] version line, not dep versions.
awk -v v="$VERSION" '
    /^\[/        { in_pkg = ($0 == "[package]") }
    in_pkg && /^version = / { print "version = \"" v "\""; next }
                 { print }
' Cargo.toml > Cargo.toml.tmp && mv Cargo.toml.tmp Cargo.toml

for f in contrib/PKGBUILD contrib/PKGBUILD-bin; do
    sed -i \
        -e "s/^pkgver=.*/pkgver=$VERSION/" \
        -e "s/^pkgrel=.*/pkgrel=$PKGREL/" \
        "$f"
done

# ---- step 3: smoke build -----------------------------------------------------

step "cargo build --release (smoke test)"
cargo build --release --quiet

# Refresh Cargo.lock against the new version so the commit + tag includes it.
git add Cargo.toml Cargo.lock contrib/PKGBUILD contrib/PKGBUILD-bin

# ---- step 4: commit, tag, push -----------------------------------------------

step "committing version bump and tagging $TAG"

git commit -m "Release $TAG"

git tag -a "$TAG" -m "spanpaper $TAG"

step "pushing main + tag"
git push origin main
git push origin "$TAG"

# ---- step 5: stage binary tarball -------------------------------------------

step "packaging dist/spanpaper-$VERSION-x86_64.tar.gz"
mkdir -p dist
STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT
PKGDIR="$STAGE/spanpaper-$VERSION-x86_64"
mkdir -p "$PKGDIR/contrib"
cp target/release/spanpaper            "$PKGDIR/spanpaper"
cp contrib/spanpaper.service           "$PKGDIR/contrib/"
cp contrib/spanpaper.desktop           "$PKGDIR/contrib/"
cp contrib/spanpaper-set-span.desktop  "$PKGDIR/contrib/"
cp contrib/spanpaper-set-side.desktop  "$PKGDIR/contrib/"
cp README.md LICENSE                   "$PKGDIR/"
tar -C "$STAGE" --owner=0 --group=0 \
    -czf "dist/spanpaper-$VERSION-x86_64.tar.gz" \
    "spanpaper-$VERSION-x86_64"

BIN_TARBALL="dist/spanpaper-$VERSION-x86_64.tar.gz"
BIN_SHA="$(sha256sum "$BIN_TARBALL" | awk '{print $1}')"

# ---- step 6: pull source tarball, compute sha, write into PKGBUILDs ---------

step "computing sha256 of GitHub source tarball at $TAG"
SRC_URL="https://github.com/$REPO_SLUG/archive/refs/tags/$TAG.tar.gz"
# Retry briefly — GitHub sometimes takes a moment to expose the tag tarball.
SRC_SHA=""
for attempt in 1 2 3 4 5; do
    if SRC_SHA="$(curl -fsSL "$SRC_URL" | sha256sum | awk '{print $1}')" \
       && [[ -n "$SRC_SHA" ]]; then
        break
    fi
    warn "source tarball not ready yet (attempt $attempt), retrying in 2s"
    sleep 2
done
[[ -n "$SRC_SHA" ]] || die "could not fetch $SRC_URL after retries"

sed -i "s/^sha256sums=.*/sha256sums=('$SRC_SHA')/" contrib/PKGBUILD
sed -i "s/^sha256sums=.*/sha256sums=('$BIN_SHA')/" contrib/PKGBUILD-bin

# ---- step 7: build .pkg.tar.zst via makepkg ---------------------------------

step "building .pkg.tar.zst via makepkg"
MAKEPKG_DIR="$STAGE/makepkg"
mkdir -p "$MAKEPKG_DIR"
cp contrib/PKGBUILD-bin "$MAKEPKG_DIR/PKGBUILD"
(
    cd "$MAKEPKG_DIR"
    makepkg --noconfirm --clean
)
PKG_FILE="$(find "$MAKEPKG_DIR" -maxdepth 1 -name '*.pkg.tar.zst' -print -quit)"
[[ -n "$PKG_FILE" ]] || die "makepkg produced no .pkg.tar.zst"
cp "$PKG_FILE" dist/
PKG_BASENAME="$(basename "$PKG_FILE")"

# ---- step 8: create GitHub release ------------------------------------------

step "creating GitHub release $TAG"
NOTES_FILE="$STAGE/notes.md"
cat > "$NOTES_FILE" <<EOF
## Install on Arch (no clone, no build)

\`\`\`bash
curl -LO https://github.com/$REPO_SLUG/releases/download/$TAG/$PKG_BASENAME
sudo pacman -U $PKG_BASENAME
yay -S mpvpaper swaybg   # runtime deps, if not already installed
\`\`\`

After install:

\`\`\`bash
spanpaper set --span ~/Wallpapers/clip.mp4 --side ~/Wallpapers/portrait.jpg
systemctl --user start spanpaper                  # run now
systemctl --user enable spanpaper                 # autostart on login (systemd sessions)
cp /usr/share/spanpaper/autostart/spanpaper.desktop ~/.config/autostart/   # autostart on XDG sessions
\`\`\`

## Release artifacts
- **\`$PKG_BASENAME\`** — pacman package, the simple install path.
- \`$(basename "$BIN_TARBALL")\` — raw binary tarball (for non-Arch hand-install or rebuilds).
EOF

gh release create "$TAG" \
    "$BIN_TARBALL" \
    "dist/$PKG_BASENAME" \
    --title "spanpaper $TAG" \
    --notes-file "$NOTES_FILE"

# ---- step 9: commit sha256 updates ------------------------------------------

step "committing sha256 updates"
git add contrib/PKGBUILD contrib/PKGBUILD-bin
if git diff --cached --quiet; then
    warn "no sha256 changes to commit (already in sync?)"
else
    git commit -m "PKGBUILDs: sha256 for $TAG"
    git push origin main
fi

# ---- done -------------------------------------------------------------------

cat <<EOF

${C_OK}${C_BOLD}released $TAG${C_RST}

  Release page: https://github.com/$REPO_SLUG/releases/tag/$TAG
  Install:      curl -LO https://github.com/$REPO_SLUG/releases/download/$TAG/$PKG_BASENAME
                sudo pacman -U $PKG_BASENAME

EOF
