#!/bin/zsh
# Mush8 - race-bench.sh
# Boots straight into a 4-player race and prints how fast it renders, uncapped (CAPPED=1 for the real 60 fps game)
# usage: tools/race-bench.sh [NX_4K=1 NX_SPLIT60=1 ...], with PLAYERS=1, ITEMS=1, SAVE=<folder>, FROM/END=<game seconds>
set -eu
repo=${0:A:h:h}
cfg="$HOME/Library/Application Support/Mush8/config.json"
conf() { python3 -c "import json,sys; print(json.load(open(sys.argv[1])).get(sys.argv[2], ''))" "$cfg" "$1" 2>/dev/null || true; }
GAME=${GAME:-$(conf folder)}
BASE=${BASE:-$(conf base)}
UPDATE=${UPDATE:-$(conf update)}
KEYS=${KEYS:-$(conf keys)}
FROM=${FROM:-60}
END=${END:-80}
LOG=${LOG:-/tmp/mush8-race-bench.log}

PLAYERS=${PLAYERS:-4}
if [ $PLAYERS = 1 ]; then in="14:A,16:A"; else in="14:A,16:DOWN,16.5:RIGHT,17:RIGHT,17.5:A"; fi
for t in $(seq 19 0.5 40) $(seq 50 0.15 $END); do
  for p in $(seq 1 $PLAYERS); do in+=$(printf ',%s:A@%s' $t $p); done
done
if [ "${ITEMS:-}" = 1 ]; then
  for t in $(seq 60 1.5 $END); do
    for p in $(seq 1 $PLAYERS); do in+=$(printf ',%s:L@%s' $t $p); done
  done
fi

cd "$GAME"
SECONDS=0
env NX_TURBO=$([ "${CAPPED:-}" = 1 ] && echo $FROM || echo $END) NX_EXIT_AT=$END NX_EXTRA_PADS=$((PLAYERS - 1)) NX_WINDOWED=1 NX_FPS=1 NX_AUTOINPUT="$in" "$@" \
  "$repo/target/release/nxhost" --keys "$KEYS" --base "$BASE" --update "$UPDATE" ${SAVE:+--save} ${SAVE:+"$SAVE"} > "$LOG" 2>&1 || true
python3 - "$LOG" $SECONDS $FROM "${CAPPED:-}" <<'PY'
import sys, re
clock, gpu, fps1, worst, inside = {}, [], [], [], False
for l in open(sys.argv[1], errors="replace"):
    m = re.match(r"\[clock\] game (\d+) s at ([0-9.]+) s", l)
    if m:
        clock[int(m.group(1))] = float(m.group(2))
        inside = int(m.group(1)) >= int(sys.argv[3])
    elif l.startswith("[fps]") and inside and "GPU" in l:
        x = l.split()
        gpu.append(float(x[8])); fps1.append(float(x[1])); worst.append(float(x[5].rstrip(",")))
t = sorted(k for k in clock if k >= int(sys.argv[3]))
if len(t) < 2:
    sys.exit(f"the race window was not reached, see {sys.argv[1]}")
fps = 60 * (t[-1] - t[0]) / (clock[t[-1]] - clock[t[0]])
gpu = gpu[1:-1] or [0]
if sys.argv[4] == "1":
    low = sum(1 for f in fps1[1:-1] if f < 59)
    print(f"race: {fps:.1f} fps capped, {low} of {len(fps1[1:-1])} seconds under 59, slowest frame {max(worst[1:-1] or [0]):.1f} ms, GPU busy {sum(gpu) / len(gpu):.1f} ms/frame  [{sys.argv[2]} s, log {sys.argv[1]}]")
else:
    print(f"race: {fps:.1f} fps uncapped ({1000 / fps:.1f} ms/frame), GPU busy {sum(gpu) / len(gpu):.1f} ms/frame  [{sys.argv[2]} s, log {sys.argv[1]}]")
PY
