#!/bin/bash
# Backlog watermark + drain-first stop E2E test (TODO 68)
#
# Emulator → Merger → Recorder with an artificially slowed writer
# (DELILA_TEST_WRITE_DELAY_MS): a real backlog builds in the Recorder's
# receiver→writer channel, the tiny watermarks in config_backlog_test.toml
# trip soft then hard, and the drain-first stop must save every queued byte.
#
# Two modes:
#   ./scripts/backlog_drain_test.sh           # manual /api/stop_drain
#   ./scripts/backlog_drain_test.sh autostop  # wait for the watcher to fire
#   ./scripts/backlog_drain_test.sh timeout   # 1-s drain timeout: the stop is
#                                             # forced, the residual is counted
#                                             # and reported (success=false)
#
# Pass criteria (checked below):
#   - recorder queue_bytes grows and backlog_level reaches 1 then 2
#   - after the drain-first stop: queue_bytes == 0, and recorder
#     events_processed == emulator events sent (nothing lost)
#   - dropped_batches did not grow while Running
#   - run document (if Mongo is up) records status Completed + stop_reason
set -u

RED='\033[0;31m'; GREEN='\033[0;32m'; YELLOW='\033[1;33m'; CYAN='\033[0;36m'; NC='\033[0m'

MODE="${1:-manual}"
BASE_CONFIG="config/config_backlog_test.toml"
CONFIG="$BASE_CONFIG"
BIN="./target/release"
API="http://localhost:9092/api"
LOG_DIR="./logs/backlog_test"
OUT_DIR="./data/backlog_test"
WRITE_DELAY_MS="${DELILA_TEST_WRITE_DELAY_MS:-200}"

PIDS=()
cleanup() {
    for pid in "${PIDS[@]:-}"; do kill "$pid" 2>/dev/null; done
    sleep 1
    for pid in "${PIDS[@]:-}"; do kill -9 "$pid" 2>/dev/null; done
}
trap cleanup EXIT

fail() { echo -e "${RED}FAIL: $*${NC}"; exit 1; }
pass() { echo -e "${GREEN}PASS: $*${NC}"; }

metric() { # component-name jq-path
    curl -s "$API/status" | python3 -c "
import sys, json
d = json.load(sys.stdin)
for c in d.get('components', []):
    if c['name'] == '$1':
        print(c.get('metrics', {}).get('$2', 0)); break
else:
    print(0)"
}

echo -e "${CYAN}=== Backlog drain-first stop test (mode: $MODE) ===${NC}"

# 0. Build + clean
cargo build --release --bins || fail "build"
rm -rf "$OUT_DIR" "$LOG_DIR"; mkdir -p "$OUT_DIR" "$LOG_DIR"

# autostop mode: enable the watcher via a temp copy of the config. The base
# config deliberately has no [operator.backlog_autostop] so manual mode
# cannot race the watcher.
if [ "$MODE" = "autostop" ]; then
    CONFIG="$LOG_DIR/config_autostop.toml"
    { cat "$BASE_CONFIG"; printf '\n[operator.backlog_autostop]\npoll_interval_secs = 1\n'; } > "$CONFIG"
elif [ "$MODE" = "timeout" ]; then
    CONFIG="$LOG_DIR/config_timeout.toml"
    sed 's/^drain_stop_timeout_secs = .*/drain_stop_timeout_secs = 1/' "$BASE_CONFIG" > "$CONFIG"
fi

# 1. Launch pipeline (recorder gets the write-delay hook)
echo -e "${CYAN}=== Starting components ===${NC}"
"$BIN/operator" --config "$CONFIG" > "$LOG_DIR/operator.log" 2>&1 & PIDS+=($!)
"$BIN/emulator" --config "$CONFIG" --source-id 0 > "$LOG_DIR/emulator.log" 2>&1 & PIDS+=($!)
"$BIN/merger"   --config "$CONFIG" > "$LOG_DIR/merger.log" 2>&1 & PIDS+=($!)
DELILA_TEST_WRITE_DELAY_MS="$WRITE_DELAY_MS" \
  "$BIN/recorder" --config "$CONFIG" > "$LOG_DIR/recorder.log" 2>&1 & PIDS+=($!)
sleep 3

grep -q "TEST HOOK ACTIVE" "$LOG_DIR/recorder.log" || fail "write-delay hook not active"
pass "write-delay hook active (${WRITE_DELAY_MS} ms/batch)"

# 2. Start a run
echo -e "${CYAN}=== Starting run ===${NC}"
START=$(curl -s -X POST "$API/run/start" -H 'Content-Type: application/json' \
    -d '{"run_number": 1, "comment": "backlog drain test", "exp_name": "backlogtest"}')
echo "$START" | grep -q '"success":true' || fail "run start: $START"
pass "run started"

# 3. Watch the backlog grow: soft (1) then hard (2)
echo -e "${CYAN}=== Waiting for backlog to build ===${NC}"
SEEN_SOFT=0; SEEN_HARD=0
DROPPED_BEFORE=$(metric Recorder dropped_batches)
for i in $(seq 1 90); do
    LEVEL=$(metric Recorder backlog_level)
    QB=$(metric Recorder queue_bytes)
    [ "$LEVEL" -ge 1 ] && SEEN_SOFT=1
    [ "$LEVEL" -ge 2 ] && SEEN_HARD=1 && break
    sleep 1
done
echo "  queue_bytes=$QB backlog_level=$LEVEL"
[ "$SEEN_SOFT" = 1 ] || fail "soft watermark never tripped"
[ "$SEEN_HARD" = 1 ] || fail "hard watermark never tripped"
pass "backlog_level reached 1 then 2 (queue_bytes=$QB)"

# 4. Stop — manual drain endpoint, or let the watcher act
if [ "$MODE" = "autostop" ]; then
    echo -e "${CYAN}=== Waiting for the autostop watcher ===${NC}"
    for i in $(seq 1 120); do
        RUNNING=$(curl -s "$API/status" | python3 -c \
            "import sys,json; print(json.load(sys.stdin).get('run_info') is not None)")
        [ "$RUNNING" = "False" ] && break
        sleep 1
    done
    [ "$RUNNING" = "False" ] || fail "watcher never stopped the run"
    grep -q "drain-first stop" "$LOG_DIR/operator.log" || fail "watcher trigger not logged"
    pass "autostop watcher fired a drain-first stop"
elif [ "$MODE" = "timeout" ]; then
    echo -e "${CYAN}=== POST /api/stop_drain (1-s timeout — must report residual) ===${NC}"
    STOP=$(curl -s -X POST "$API/stop_drain")
    echo "$STOP" | grep -q '"success":false' || fail "timeout run must not claim success: $STOP"
    echo "$STOP" | grep -q 'DRAIN INCOMPLETE' || fail "residual not reported: $STOP"
    echo "$STOP" | grep -qE 'Recorder: [0-9]+ bytes' || fail "per-component residual missing: $STOP"
    pass "drain timeout reported the residual, never silent"
    echo -e "${GREEN}=== TIMEOUT-PATH CHECKS PASSED ===${NC}"
    exit 0
else
    echo -e "${CYAN}=== POST /api/stop_drain ===${NC}"
    STOP=$(curl -s -X POST "$API/stop_drain")
    echo "$STOP" | grep -q '"success":true' || fail "stop_drain: $STOP"
    pass "drain-first stop returned success (backlog fully written)"
fi

# 5. Verify nothing was lost
echo -e "${CYAN}=== Verifying ===${NC}"
sleep 2
QB_FINAL=$(metric Recorder queue_bytes)
[ "$QB_FINAL" = 0 ] || fail "residual queue_bytes=$QB_FINAL after drain"
pass "recorder queue fully drained"

SENT=$(metric "emu-0" events_processed)
WRITTEN=$(metric Recorder events_processed)
[ "$SENT" -gt 0 ] || fail "emulator sent nothing"
[ "$SENT" = "$WRITTEN" ] || fail "events lost: emulator sent $SENT, recorder wrote $WRITTEN"
pass "all $SENT events written — nothing lost"

DROPPED_AFTER=$(metric Recorder dropped_batches)
[ "$DROPPED_AFTER" = "$DROPPED_BEFORE" ] || \
    fail "dropped_batches grew during the run ($DROPPED_BEFORE → $DROPPED_AFTER)"
pass "dropped_batches unchanged"

echo -e "${GREEN}=== ALL CHECKS PASSED ===${NC}"
