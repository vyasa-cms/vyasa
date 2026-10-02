#!/usr/bin/env bash
# Long-running soak test: hold steady traffic and watch resident memory.
#
# A leak shows as RSS that climbs and never returns; ordinary caching shows
# as RSS that rises to a plateau. The distinction needs hours, not minutes,
# which is why this is separate from load.sh.

set -euo pipefail

BASE="${1:-http://localhost:8080}"
MINUTES="${2:-60}"
PID="$(pgrep -f 'vyasa serve' | head -1 || true)"

if [ -z "$PID" ]; then
    echo "no 'vyasa serve' process found; start the server first" >&2
    exit 1
fi
if ! command -v oha >/dev/null 2>&1; then
    echo "oha is not installed: cargo install oha" >&2
    exit 1
fi

LOG="perf/soak-$(date +%Y%m%d-%H%M%S).tsv"
printf 'elapsed_s\trss_kb\n' > "$LOG"
echo "soaking $BASE for ${MINUTES}m against pid $PID; RSS samples → $LOG"

oha --no-tui -z "${MINUTES}m" -c 20 "$BASE/" > /dev/null 2>&1 &
LOAD_PID=$!

START=$(date +%s)
while kill -0 "$LOAD_PID" 2>/dev/null; do
    RSS=$(awk '/VmRSS/ {print $2}' "/proc/$PID/status" 2>/dev/null || echo 0)
    printf '%s\t%s\n' "$(( $(date +%s) - START ))" "$RSS" >> "$LOG"
    sleep 30
done

echo
echo "samples written to $LOG"
awk -F'\t' 'NR>1 {if (min=="" || $2<min) min=$2; if ($2>max) max=$2; last=$2}
     END {printf "RSS min %s kB, max %s kB, final %s kB\n", min, max, last}' "$LOG"
echo "A final value close to the max, with a rising trend throughout, suggests a leak."
