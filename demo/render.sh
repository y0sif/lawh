#!/usr/bin/env bash
# Renders the README demo: demo/area.demo typed into lawh with no window (the app's own frames),
# then framed with ffmpeg: a dark backdrop, a window with a title bar, and under it a magnifier
# on the status bar's left end, where the command line, the mode and messages show. Writes
# demo/out/lawh-demo.mp4 and assets/demo.gif. Run from anywhere; see demo/README.md.
#
#   SKIP_FRAMES=1 demo/render.sh   reframes the last frames without typing the script again
#   DEMO_SCRIPT=other.demo         renders another script (default demo/area.demo)
set -euo pipefail

cd "$(dirname "$0")/.."
OUT=demo/out

# ffmpeg 7 or later: older ones lack `-/filter_complex` (a filter graph read from a file).
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

# 1. The frames, timeline.ffconcat and demo.json, from the app's frame composition
#    (crates/lawh/src/app/demo.rs).
#    The last run's outputs go first, so a test that didn't run can't leave them to be used.
if [ -z "${SKIP_FRAMES:-}" ]; then
  rm -f "$OUT/demo.json" "$OUT/timeline.ffconcat"
  LAWH_DEMO_SCRIPT="$SCRIPT" LAWH_DEMO_OUT="$OUT" nice -n 19 \
    cargo test -p lawh --bin lawh --release --quiet -- --ignored --exact app::demo::render_demo
fi
for made in demo.json timeline.ffconcat; do
  if [ ! -f "$OUT/$made" ]; then
    echo "render.sh: $OUT/$made is missing: the driver (app::demo::render_demo) didn't write it" >&2
    exit 1
  fi
done

secs() { printf '%d.%03d' $(($1 / 1000)) $(($1 % 1000)); }
# demo.json is written one field a line (a frame a line), so sed reads it without a JSON tool.
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
BELOW=84     # backdrop below it, where the magnifier shows
R=12         # corner radius
W=$((CW + 2 * PAD))
H=$((TOP + BAR + CH + BELOW))
X0=$PAD; Y0=$TOP; X1=$((PAD + CW)); Y1=$((TOP + BAR + CH))
BACKDROP=(0x23 0x24 0x28); TITLEBAR=(0xE6 0xE6 0xE1); EDGE=(0xD2 0xD2 0xCC)
RED=(0xFF 0x5F 0x57); YELLOW=(0xFE 0xBC 0x2E); GREEN=(0x28 0xC8 0x40)

# Coverage (0 to 1) of a rectangle with rounded corners, centre (cx, cy), half size (hx, hy),
# corner radius r (R unless given): from its signed distance, so edges are antialiased.
rounded() { # cx cy hx hy [r]
  local r=${5:-$R}
  local qx="(abs(X+0.5-($1))-($3-$r))" qy="(abs(Y+0.5-($2))-($4-$r))"
  echo "clip(0.5-(hypot(max($qx,0),max($qy,0))+min(max($qx,$qy),0)-$r),0,1)"
}
# A soft shadow under that rectangle: `alpha` black moved `dy` down, fading out over `spread`
# from `inset` inside its edge.
shadow() { # cx cy hx hy r dy inset spread alpha
  local qx="(abs(X+0.5-($1))-($3-$5))" qy="(abs(Y+0.5-($2+$6))-($4-$5))"
  echo "$9*pow(1-clip((hypot(max($qx,0),max($qy,0))+min(max($qx,$qy),0)-$5+$7)/$8,0,1),2)"
}
dot() { echo "clip(7-hypot(X+0.5-($1),Y+0.5-$((Y0 + BAR / 2))),0,1)"; } # radius 6.5
CX="($X0+$X1)/2"; CY="($Y0+$Y1)/2"; HX="($X1-$X0)/2"; HY="($Y1-$Y0)/2"
WINDOW=$(rounded "$CX" "$CY" "$HX" "$HY")
# The window's shadow: 45 % black moved 10 px down, fading out over 30 px from 6 px inside.
SHADOW=$(shadow "$CX" "$CY" "$HX" "$HY" "$R" 10 6 30 0.45)
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

# 3. The magnifier: the left end of the status bar, cut from the full-resolution frames and
#    shown MAG times as large as the window shows it, under the window's left edge, while it
#    shows something (the command line, the mode or a message).
STATUS_BAR=24  # the status bar's height (STATUS_BAR_PX, crates/lawh-render/src/overlay.rs)
STATUS_EDGE=1  # the line along its top, left out (STATUS_EDGE_PX, lawh-render's renderer/ui.rs)
# How much of the bar it shows, from its left end: area.demo's longest left text, `:sym
# arrow.r.dashed` and its caret, runs from 8.5 to 134.5 (measured on its frames; the text starts
# STATUS_PAD_PX = 8 in), so 144 leaves as much room after it as before it.
LENS_SRC=144
MAG=2          # at SCALE 2, one frame pixel a GIF pixel: as sharp as the frames are
LW=$((LENS_SRC * MAG)); LH=$(((STATUS_BAR - STATUS_EDGE) * MAG))
LR=10          # its corner radius
LM=24          # room around it in its image, for its shadow
LX=$X0; LY=$((Y1 + (BELOW - LH) / 2))
LENS_EDGE=(0xC4 0xC4 0xBE)
FADE_MS=150    # it fades in and out over this long
HOLD_MS=500    # and stays up through a gap shorter than this, rather than blink
LCX="$LM+$LW/2"; LCY="$LM+$LH/2"
# Under the lens: its shadow (as the window's, smaller) and its 1 px edge, straight alpha.
LENS_VARS="st(0,$(rounded "$LCX" "$LCY" "$LW/2+1" "$LH/2+1" $((LR + 1))));st(1,$(shadow "$LCX" "$LCY" "$LW/2" "$LH/2" "$LR" 3 3 14 0.4));st(2,ld(0)+ld(1)*(1-ld(0)))"
lens_channel() { echo "$LENS_VARS;if(ld(2),${LENS_EDGE[$1]}*ld(0)/ld(2),0)"; }
ffmpeg -loglevel error -y -f lavfi -i "color=c=black:s=$((LW + 2 * LM))x$((LH + 2 * LM))" -frames:v 1 -vf \
  "format=rgba,geq=r='$(lens_channel 0)':g='$(lens_channel 1)':b='$(lens_channel 2)':a='$LENS_VARS;255*ld(2)'" \
  "$OUT/lens.png"
# Its rounded corners, as alpha for the crop.
ffmpeg -loglevel error -y -f lavfi -i "color=c=black:s=${LW}x${LH}" -frames:v 1 -vf \
  "format=gray,geq=lum='255*$(rounded "$LW/2" "$LH/2" "$LW/2" "$LH/2" "$LR")'" \
  "$OUT/lens-mask.png"

# When it shows: the frames whose `status_left` (demo.json) isn't empty, joined across gaps
# shorter than HOLD_MS. Its opacity is 1 inside each span, rising over FADE_MS from the span's
# start and falling over FADE_MS to its end.
OPACITY=0
span() { # start end, in ms
  OPACITY+="+clip((T-$(secs "$1"))/$(secs "$FADE_MS"),0,1)*clip(($(secs "$2")-T)/$(secs "$FADE_MS"),0,1)"
}
from=; to=
while read -r start duration shown; do
  if [ "$shown" = 0 ]; then
    continue
  elif [ -n "$from" ] && [ $((start - to)) -lt "$HOLD_MS" ]; then
    to=$((start + duration))
  else
    if [ -n "$from" ]; then span "$from" "$to"; fi
    from=$start; to=$((start + duration))
  fi
done < <(sed -n \
  -e 's/^    {"file": "[^"]*", "start_ms": \([0-9]*\), "duration_ms": \([0-9]*\), "status_left": ""}.*$/\1 \2 0/p' \
  -e 't' \
  -e 's/^    {"file": "[^"]*", "start_ms": \([0-9]*\), "duration_ms": \([0-9]*\), "status_left": ".*$/\1 \2 1/p' \
  "$OUT/demo.json")
if [ -n "$from" ]; then span "$from" "$to"; fi

FILTERS="$OUT/filters.txt"
{
  # Frames come in at their own times; fps makes them a constant rate before anything is drawn
  # on them, and trim ends the video at the script's end (the timeline names the last frame
  # twice). A frame of SCALE physical pixels a logical one is averaged down to logical pixels;
  # the magnifier's crop is taken before that, at full resolution.
  printf '[0:v]fps=%d,trim=end=%s,split[full][bar];\n' "$FPS" "$(secs "$DURATION_MS")"
  printf '[full]scale=%d:%d:flags=area,setsar=1[f];\n' "$CW" "$CH"
  printf '[bar]crop=%d:%d:0:%d,scale=%d:%d:flags=area,setsar=1,format=rgba[crop];\n' \
    $((LENS_SRC * SCALE)) $(((STATUS_BAR - STATUS_EDGE) * SCALE)) \
    $(((CH - STATUS_BAR + STATUS_EDGE) * SCALE)) "$LW" "$LH"
  printf '[crop][4:v]alphamerge[lens];\n'
  printf '[3:v][lens]overlay=%d:%d:format=auto:shortest=1,format=rgba,' "$LM" "$LM"
  printf "geq=r='r(X,Y)':g='g(X,Y)':b='b(X,Y)':a='alpha(X,Y)*min(%s,1)'[mag];\n" "$OPACITY"
  printf '[1:v][f]overlay=%d:%d:shortest=1[a];\n' "$X0" $((Y0 + BAR))
  printf '[a][2:v]overlay=0:0:shortest=1[b];\n'
  printf '[b][mag]overlay=%d:%d:shortest=1[out]\n' $((LX - LM)) $((LY - LM))
} > "$FILTERS"
INPUTS=(-f concat -safe 0 -i "$OUT/timeline.ffconcat"
        -loop 1 -framerate "$FPS" -i "$OUT/canvas.png"
        -loop 1 -framerate "$FPS" -i "$OUT/corners.png"
        -loop 1 -framerate "$FPS" -i "$OUT/lens.png"
        -loop 1 -framerate "$FPS" -i "$OUT/lens-mask.png")

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
