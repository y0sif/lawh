#!/usr/bin/env bash
# Renders the README demo: demo/area.demo typed into lawh with no window (the app's own frames),
# then framed with ffmpeg: a dark backdrop, a window with a title bar, and the keys being typed
# below it. Writes demo/out/lawh-demo.mp4 and assets/demo.gif. Run from anywhere; see
# demo/README.md.
#
#   SKIP_FRAMES=1 demo/render.sh   reframes the last frames without typing the script again
#   DEMO_SCRIPT=other.demo         renders another script (default demo/area.demo)
set -euo pipefail

cd "$(dirname "$0")/.."
OUT=demo/out

# ffmpeg 7 or later: older ones lack `-/filter_complex` (a filter graph read from a file) and
# drawtext's `y_align`.
if ! command -v ffmpeg > /dev/null; then
  echo "render.sh: ffmpeg not found; it needs ffmpeg 7 or later on PATH" >&2
  exit 1
fi
FFMPEG_VERSION=$(ffmpeg -version | sed -n '1s/^ffmpeg version \([^ ]*\).*/\1/p')
FFMPEG_MAJOR=$(printf '%s' "$FFMPEG_VERSION" | sed -n 's/^n\{0,1\}\([0-9][0-9]*\)\..*/\1/p')
if [ -z "$FFMPEG_MAJOR" ]; then
  echo "render.sh: can't read ffmpeg's version (\"$FFMPEG_VERSION\"); going on, but it needs 7 or later" >&2
elif [ "$FFMPEG_MAJOR" -lt 7 ]; then
  echo "render.sh: ffmpeg $FFMPEG_VERSION is too old; it needs ffmpeg 7 or later" >&2
  exit 1
fi
SCRIPT=${DEMO_SCRIPT:-demo/area.demo}
FONT=assets/fonts/Inter-Regular.ttf
FPS=30       # mp4 frame rate
GIF_FPS=15   # the GIF is decimated to keep it small

# 1. The frames, timeline.ffconcat, demo.json and keys.tsv, from the app's frame composition
#    (crates/lawh/src/app/demo.rs).
#    The last run's outputs go first, so a test that didn't run can't leave them to be used.
if [ -z "${SKIP_FRAMES:-}" ]; then
  rm -f "$OUT/demo.json" "$OUT/timeline.ffconcat" "$OUT/keys.tsv"
  LAWH_DEMO_SCRIPT="$SCRIPT" LAWH_DEMO_OUT="$OUT" nice -n 19 \
    cargo test -p lawh --bin lawh --release --quiet -- --ignored --exact app::demo::render_demo
fi
for made in demo.json timeline.ffconcat keys.tsv; do
  if [ ! -f "$OUT/$made" ]; then
    echo "render.sh: $OUT/$made is missing: the driver (app::demo::render_demo) didn't write it" >&2
    exit 1
  fi
done

secs() { printf '%d.%03d' $(($1 / 1000)) $(($1 % 1000)); }
# demo.json is written one field a line, so sed reads it without a JSON tool.
json_int() { sed -n "s/^  \"$1\": \([0-9]*\),\$/\1/p" "$OUT/demo.json"; }
CW=$(json_int logical_width)    # the window's content, in logical pixels
CH=$(json_int logical_height)
SCALE=$(json_int scale)
DURATION_MS=$(json_int duration_ms)
# The board's file name, for the title as the app makes it (`status::title`, unmodified).
BOARD=$(sed -n 's/^  "file": "\(.*\)",$/\1/p' "$OUT/demo.json")

# 2. The chrome around the frames, all in logical pixels: the backdrop, a soft shadow, and the
#    window (rounded corners, a title bar with three dots and the title).
PAD=40       # backdrop left and right of the window
TOP=36       # backdrop above it
BAR=40       # title bar
KEYS=84      # backdrop below it, where the keys show
R=12         # corner radius
W=$((CW + 2 * PAD))
H=$((TOP + BAR + CH + KEYS))
X0=$PAD; Y0=$TOP; X1=$((PAD + CW)); Y1=$((TOP + BAR + CH))
BACKDROP=(0x23 0x24 0x28); TITLEBAR=(0xE6 0xE6 0xE1); EDGE=(0xD2 0xD2 0xCC)
RED=(0xFF 0x5F 0x57); YELLOW=(0xFE 0xBC 0x2E); GREEN=(0x28 0xC8 0x40)

# Coverage (0 to 1) of a rectangle with rounded corners, centre (cx, cy), half size (hx, hy):
# from its signed distance, so edges are antialiased.
rounded() { # cx cy hx hy
  local qx="(abs(X+0.5-($1))-($3-$R))" qy="(abs(Y+0.5-($2))-($4-$R))"
  echo "clip(0.5-(hypot(max($qx,0),max($qy,0))+min(max($qx,$qy),0)-$R),0,1)"
}
dot() { echo "clip(7-hypot(X+0.5-($1),Y+0.5-$((Y0 + BAR / 2))),0,1)"; } # radius 6.5
CX="($X0+$X1)/2"; CY="($Y0+$Y1)/2"; HX="($X1-$X0)/2"; HY="($Y1-$Y0)/2"
WINDOW=$(rounded "$CX" "$CY" "$HX" "$HY")
# The shadow: 45 % black under the window moved 10 px down, fading out over 30 px from 6 px
# inside its edge.
QX="(abs(X+0.5-($CX))-($HX-$R))"; QY="(abs(Y+0.5-($CY+10))-($HY-$R))"
SHADOW="0.45*pow(1-clip((hypot(max($QX,0),max($QY,0))+min(max($QX,$QY),0)-$R+6)/30,0,1),2)"
VARS="st(0,$WINDOW);st(1,$SHADOW);st(2,$(dot $((X0 + 22))));st(3,$(dot $((X0 + 44))));st(4,$(dot $((X0 + 66))))"
channel() { # index: the backdrop under its shadow, then the window over it
  local i=$1 bar="if(eq(Y,$((Y0 + BAR - 1))),${EDGE[$1]},${TITLEBAR[$1]})"
  local win="($bar*(1-ld(2)-ld(3)-ld(4))+${RED[$i]}*ld(2)+${YELLOW[$i]}*ld(3)+${GREEN[$i]}*ld(4))"
  echo "$VARS;${BACKDROP[$i]}*(1-ld(1))*(1-ld(0))+$win*ld(0)"
}
backdrop() { echo "$VARS;${BACKDROP[$1]}*(1-ld(1))"; }
mkdir -p "$OUT"
TITLE="$BOARD - lawh"
ffmpeg -loglevel error -y -f lavfi -i "color=c=black:s=${W}x${H}" -frames:v 1 -vf \
  "format=rgb24,geq=r='$(channel 0)':g='$(channel 1)':b='$(channel 2)',drawtext=fontfile=$FONT:text='$TITLE':fontsize=14:fontcolor=0x6B6B66:x=(w-text_w)/2:y=$((Y0 + BAR / 2))-th/2" \
  "$OUT/canvas.png"
# What goes over the frames: the backdrop where the window's lower corners are cut away.
ffmpeg -loglevel error -y -f lavfi -i "color=c=black:s=${W}x${H}" -frames:v 1 -vf \
  "format=rgba,geq=r='$(backdrop 0)':g='$(backdrop 1)':b='$(backdrop 2)':a='$VARS;if(gte(Y,$((Y0 + BAR))),255*(1-ld(0)),0)'" \
  "$OUT/corners.png"

# 3. The keys typed, from keys.tsv: one drawtext a state of the key display, each its own text
#    file so no character needs escaping.
rm -rf "$OUT/keys"; mkdir -p "$OUT/keys"
KEYS_Y=$((Y1 + KEYS / 2))
FILTERS="$OUT/filters.txt"
{
  # Frames come in at their own times; fps makes them a constant rate before anything is drawn
  # on them, so the keys change on the frame they are typed, and trim ends the video at the
  # script's end (the timeline names the last frame twice). A frame of SCALE physical pixels a
  # logical one is averaged down to logical pixels.
  printf '[0:v]fps=%d,trim=end=%s,scale=%d:%d:flags=area,setsar=1[f];\n' \
    "$FPS" "$(secs "$DURATION_MS")" "$CW" "$CH"
  printf '[1:v][f]overlay=%d:%d:shortest=1[a];\n' "$X0" $((Y0 + BAR))
  printf '[a][2:v]overlay=0:0:shortest=1'
  n=0
  while IFS=$'\t' read -r start end text; do
    n=$((n + 1))
    file=$(printf '%s/keys/%04d.txt' "$OUT" "$n")
    printf '%s' "$text" > "$file"
    printf ",\ndrawtext=fontfile=%s:textfile=%s:expansion=none:fontsize=24:fontcolor=0xF2F2EE" "$FONT" "$file"
    printf ":x=(w-text_w)/2:y=%d-(font_a-font_d)/2:y_align=font" "$KEYS_Y"
    printf ":enable='gte(t,%s)*lt(t,%s)'" "$(secs "$start")" "$(secs "$end")"
  done < "$OUT/keys.tsv"
  printf '[out]\n'
} > "$FILTERS"
INPUTS=(-f concat -safe 0 -i "$OUT/timeline.ffconcat"
        -loop 1 -framerate "$FPS" -i "$OUT/canvas.png"
        -loop 1 -framerate "$FPS" -i "$OUT/corners.png")

# 4. The mp4 and the GIF (keybr-tui's settings: 128 colours, no dither, only changed rectangles).
ffmpeg -loglevel error -y "${INPUTS[@]}" -/filter_complex "$FILTERS" -map '[out]' \
  -c:v libx264 -crf 18 -preset slow -pix_fmt yuv420p -movflags +faststart "$OUT/lawh-demo.mp4"
GIF_FILTERS="$OUT/filters-gif.txt"
sed 's/\[out\]$/,fps='"$GIF_FPS"',split[g1][g2];[g1]palettegen=max_colors=128:stats_mode=diff[p];[g2][p]paletteuse=dither=none:diff_mode=rectangle[out]/' \
  "$FILTERS" > "$GIF_FILTERS"
# Encoded in demo/out first: the tracked assets/demo.gif changes only once the GIF is whole.
ffmpeg -loglevel error -y "${INPUTS[@]}" -/filter_complex "$GIF_FILTERS" -map '[out]' "$OUT/demo.gif"
mkdir -p assets
mv "$OUT/demo.gif" assets/demo.gif

echo "frames at scale $SCALE; wrote:"
ls -la "$OUT/lawh-demo.mp4" assets/demo.gif
