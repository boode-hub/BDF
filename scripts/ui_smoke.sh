#!/usr/bin/env bash
# End-to-end UI test on Linux: drives the real app with xdotool on a virtual
# display, then checks the saved PDF. Needs Xvfb, xdotool and ImageMagick.
# Usage: scripts/ui_smoke.sh path/to/bdf [screenshot-dir]
set -euo pipefail
BIN="$(realpath "${1:?path to bdf}")"
SHOTS="${2:-}"
work="$(mktemp -d)"
trap 'kill $APP $XVFB 2>/dev/null || true; rm -rf "$work"' EXIT
"$BIN" --generate 5 "$work/test.pdf"
cp "$work/test.pdf" "$work/original.pdf"
export DISPLAY=:93
Xvfb $DISPLAY -screen 0 1440x900x24 >/dev/null 2>&1 & XVFB=$!
sleep 1
BDF_DATA_DIR="$work/data" "$BIN" "$work/test.pdf" >"$work/app.log" 2>&1 & APP=$!
for _ in $(seq 1 50); do xdotool search --name "BDF" >/dev/null 2>&1 && break; sleep 0.2; done
sleep 2
x() { xdotool "$@"; sleep 0.4; }
shot() { [ -n "$SHOTS" ] && import -window root "$SHOTS/$1.png" || true; }

# Toolbar positions for the default 1280x860 window (fonts are embedded, so stable).
HIGHLIGHT="283 50"; PEN="414 50"; RECT="447 50"; SELECT="174 50"

# Mouse only: synthetic keystrokes are unreliable on a bare X server.
x mousemove $HIGHLIGHT click 1                       # highlight the first words
x mousemove 366 246 mousedown 1; x mousemove 500 247; x mousemove 650 247; x mouseup 1
x mousemove $PEN click 1                             # a pen stroke
x mousemove 400 600 mousedown 1; x mousemove 480 640; x mousemove 560 600; x mousemove 640 650; x mouseup 1
x mousemove $RECT click 1                            # a rectangle, then Edit > Undo
x mousemove 700 500 mousedown 1; x mousemove 900 560; x mouseup 1
sleep 0.5
x mousemove 54 15 click 1; x mousemove 80 47 click 1
x mousemove $SELECT click 1; x mousemove 700 400
shot ui-annotated
x mousemove 20 15 click 1; x mousemove 51 124 click 1   # File > Save
sleep 3
shot ui-saved

fail=0
annots="$(BDF_PDFIUM="${BDF_PDFIUM:-$(dirname "$BIN")}" "$BIN" --list-annotations "$work/test.pdf")"
echo "$annots"
check() { if grep -q "$1" <<<"$annots"; then echo "ok: $2"; else echo "FAIL: $2"; fail=1; fi; }
check $'page 1\tHighlight' "highlight saved"
check $'page 1\tInk' "pen stroke saved"
if grep -q "Rectangle" <<<"$annots"; then echo "FAIL: undone rectangle was saved"; fail=1; else echo "ok: undo removed the rectangle"; fi
if cmp -s -n "$(stat -c%s "$work/original.pdf")" "$work/original.pdf" "$work/test.pdf"; then
  echo "ok: original bytes preserved (incremental save)"
else
  echo "FAIL: original bytes changed"; fail=1
fi
if ! kill -0 $APP 2>/dev/null; then echo "FAIL: app exited"; cat "$work/app.log"; fail=1; fi
kill $APP 2>/dev/null || true

# Forms: tick a checkbox, save from the menu, check the saved value.
"$BIN" --generate-form "$work/form.pdf"
BDF_DATA_DIR="$work/data2" "$BIN" "$work/form.pdf" >"$work/app2.log" 2>&1 & APP=$!
for _ in $(seq 1 50); do xdotool search --name "form.pdf" >/dev/null 2>&1 && break; sleep 0.2; done
sleep 2
x mousemove 475 352 click 1                          # the "I agree" checkbox
sleep 1
shot ui-form
x mousemove 20 15 click 1; x mousemove 51 124 click 1   # File > Save
sleep 3
if grep -aqE "/AS ?/Yes" "$work/form.pdf"; then echo "ok: checkbox value saved"; else echo "FAIL: checkbox value not saved"; fail=1; fi
exit $fail
