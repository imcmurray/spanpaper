#!/usr/bin/env bash
# gen-test-assets.sh — generate three calibration wallpapers for spanpaper.
#
#   test-assets/test-span-1920x2160.png   spanning image (HDMI-A-4 + DP-6)
#   test-assets/test-span-1920x2160.mp4   same image animated as video
#   test-assets/test-side-1080x1920.png   side monitor (DP-5)
#
# The span image is designed to make a "true span vs duplicated image" failure
# instantly visible:
#   - a yellow diagonal from (0,0) to (1920,2160) forms a single straight line
#     only when the two halves are correctly aligned across the seam
#   - a white circle centered exactly on y=1080 spans both monitors; if the
#     image is duplicated you'd see two half-circles, not one full one
#   - a 12-row grid + ruler whose row numbers continue across the seam

set -euo pipefail

REPO_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
OUT_DIR="$REPO_DIR/test-assets"
mkdir -p "$OUT_DIR"

# ---- tools -------------------------------------------------------------------

command -v magick >/dev/null || { echo "magick (ImageMagick 7+) required" >&2; exit 1; }
command -v ffmpeg >/dev/null || { echo "ffmpeg required"                   >&2; exit 1; }

# ImageMagick takes a font NAME (resolved through fontconfig). ffmpeg needs an
# actual TTF file path.
# Pick the font name. Done with bash builtins to avoid awk-exit SIGPIPE
# tripping `set -o pipefail`.
IM_FONT="DejaVu-Sans-Bold"
{
    set +o pipefail
    _candidate=$(magick -list font 2>/dev/null | grep -m1 'Font: DejaVu-Sans-Bold' | awk '{print $2}')
    set -o pipefail
} || true
[[ -n "${_candidate:-}" ]] && IM_FONT="$_candidate"

if command -v fc-match >/dev/null; then
    FONT_FILE=$(fc-match -f '%{file}' 'DejaVu Sans:weight=bold' 2>/dev/null || true)
fi
: "${FONT_FILE:=/usr/share/fonts/TTF/DejaVuSans-Bold.ttf}"
[[ -f "$FONT_FILE" ]] || {
    echo "could not locate a bold TTF font; install ttf-dejavu" >&2
    exit 1
}

echo "==> using ImageMagick font: $IM_FONT"
echo "==> using ffmpeg font file:  $FONT_FILE"

SPAN_PNG="$OUT_DIR/test-span-1920x2160.png"
SPAN_MP4="$OUT_DIR/test-span-1920x2160.mp4"
SIDE_PNG="$OUT_DIR/test-side-1080x1920.png"

# ---- 1. Span image -----------------------------------------------------------

echo "==> building $SPAN_PNG (1920×2160)"

# Grid lines for the full 1920×2160 canvas, skipping the seam row (y=1080)
# which is drawn separately in red.
GRID_DRAW=""
for x in 192 384 576 768 960 1152 1344 1536 1728; do
    GRID_DRAW+="line $x,0 $x,2160 "
done
for y in 180 360 540 720 900 1260 1440 1620 1800 1980; do
    GRID_DRAW+="line 0,$y 1920,$y "
done

# Row-number labels down the left edge — they must read continuously across
# the seam, e.g. ..., 8, 9, 10, 11, ...
ROW_LABELS=""
i=0
for y in 90 270 450 630 810 990 1170 1350 1530 1710 1890 2070; do
    ROW_LABELS+="text 30,$y 'row $i' "
    i=$((i+1))
done

magick \
    \( -size 1920x1080 gradient:'#ff7a1f-#a32d0a' \) \
    \( -size 1920x1080 gradient:'#3a6fff-#0a1a3a' \) \
    -append +repage \
    -fill none -stroke 'rgba(255,255,255,0.28)' -strokewidth 2 \
    -draw "$GRID_DRAW" \
    -stroke 'rgba(255,80,80,0.95)'  -strokewidth 5 \
    -draw "line 0,1080 1920,1080" \
    -stroke 'rgba(255,242,80,0.92)' -strokewidth 7 \
    -draw "line 0,0 1920,2160" \
    -stroke 'rgba(255,255,255,0.85)' -strokewidth 8 -fill none \
    -draw "circle 960,1080 960,680" \
    -strokewidth 4 \
    -draw "circle 960,1080 960,830" \
    -strokewidth 3 \
    -draw "circle 960,1080 960,930" \
    -font "$IM_FONT" -fill white -stroke none \
    -pointsize 28 -fill 'rgba(255,255,255,0.85)' -stroke none \
    -draw "$ROW_LABELS" \
    -fill white \
    -gravity North     -pointsize 150 -annotate +0+90  "HDMI-A-4" \
    -gravity North     -pointsize 56  -annotate +0+260 "★  TOP HALF  ·  1920 × 1080  ★" \
    -gravity North     -pointsize 40  -annotate +0+340 "(this should appear on the upper monitor)" \
    -gravity South     -pointsize 150 -annotate +0+280 "DP-6" \
    -gravity South     -pointsize 56  -annotate +0+450 "★  BOTTOM HALF  ·  1920 × 1080  ★" \
    -gravity South     -pointsize 40  -annotate +0+530 "(this should appear on the lower monitor)" \
    -gravity NorthWest -pointsize 28  -annotate +30+30 "(0, 0)" \
    -gravity NorthEast -pointsize 28  -annotate +30+30 "(1920, 0)" \
    -gravity SouthWest -pointsize 28  -annotate +30+30 "(0, 2160)" \
    -gravity SouthEast -pointsize 28  -annotate +30+30 "(1920, 2160)" \
    -fill 'rgba(255,200,200,1)' \
    -gravity NorthWest -pointsize 36 -annotate +280+1045 "← seam at y = 1080  ·  HDMI-A-4 ends ↑  ·  DP-6 begins ↓ →" \
    -fill 'rgba(255,242,80,1)' \
    -gravity NorthWest -pointsize 34 -annotate +900+540 "← diagonal must be ONE straight line if spanning works" \
    -gravity NorthWest -pointsize 34 -annotate +900+1440 "← diagonal continues here (same line)" \
    -fill 'rgba(255,255,255,1)' \
    -gravity NorthWest -pointsize 32 -annotate +320+760  "circle straddles the seam — only" \
    -gravity NorthWest -pointsize 32 -annotate +320+800  "renders as a full circle if spanned" \
    "$SPAN_PNG"

# ---- 2. Side image -----------------------------------------------------------

echo "==> building $SIDE_PNG (1080×1920)"

SIDE_GRID=""
for x in 180 360 540 720 900; do
    SIDE_GRID+="line $x,0 $x,1920 "
done
for y in 160 320 480 640 800 960 1120 1280 1440 1600 1760; do
    SIDE_GRID+="line 0,$y 1080,$y "
done

magick \
    -size 1080x1920 gradient:'#7a1fff-#1a0a55' \
    -fill none -stroke 'rgba(255,255,255,0.25)' -strokewidth 2 \
    -draw "$SIDE_GRID" \
    -stroke 'rgba(255,255,255,0.55)' -strokewidth 6 -fill none \
    -draw "circle 540,960 540,610" \
    -font "$IM_FONT" -fill white -stroke none \
    -gravity North  -pointsize 150 -annotate +0+200 "DP-5" \
    -gravity Center -pointsize 56  -annotate +0-200 "STATIC IMAGE  ·  swaybg" \
    -gravity Center -pointsize 60  -annotate +0-60  "1080 × 1920" \
    -gravity Center -pointsize 48  -annotate +0+40  "PORTRAIT (rotated)" \
    -gravity Center -pointsize 36  -annotate +0+160 "(side / left monitor)" \
    -gravity South  -pointsize 38  -annotate +0+200 "this image is INDEPENDENT of" \
    -gravity South  -pointsize 38  -annotate +0+140 "the spanning video on HDMI-A-4 / DP-6" \
    -gravity NorthWest -pointsize 26 -annotate +20+20 "(0,0)" \
    -gravity SouthEast -pointsize 26 -annotate +20+20 "(1080,1920)" \
    "$SIDE_PNG"

# ---- 3. Span video -----------------------------------------------------------

echo "==> building $SPAN_MP4 (8s loop, h264 yuv420p, no audio)"

# Filter chain:
#   - red horizontal bar scrolls linearly from y=-30 to y=2190 over 8 seconds
#     (one bar moving continuously across BOTH monitors — proof of span sync)
#   - two HH:MM:SS.mmm timestamps straddling the seam; they should always show
#     the same time, demonstrating both mpvpaper instances are in sync

VF="drawbox=x=0:y='(t/8)*2220-30':w=iw:h=30:color=red@0.85:t=fill"
VF+=",drawtext=fontfile=${FONT_FILE}:text='%{pts\\:hms}':x=(w-tw)/2:y=960:fontsize=80:fontcolor=white:box=1:boxcolor=black@0.65:boxborderw=18"
VF+=",drawtext=fontfile=${FONT_FILE}:text='%{pts\\:hms}':x=(w-tw)/2:y=1100:fontsize=80:fontcolor=white:box=1:boxcolor=black@0.65:boxborderw=18"

ffmpeg -y -loglevel warning -loop 1 -t 8 -i "$SPAN_PNG" \
    -vf "$VF" \
    -c:v libx264 -preset slow -crf 22 -pix_fmt yuv420p \
    -movflags +faststart -an \
    "$SPAN_MP4"

# ---- done --------------------------------------------------------------------

echo
echo "Generated:"
for f in "$SPAN_PNG" "$SPAN_MP4" "$SIDE_PNG"; do
    printf "  %s  (%s)\n" "$f" "$(du -h "$f" | cut -f1)"
done
echo
echo "Apply with:"
echo "  spanpaper set --video $SPAN_MP4 --left-image $SIDE_PNG"
echo "  spanpaper start --background"
