#!/bin/bash
# DELILA DAQ Start Script
# Usage: ./scripts/start_daq.sh [config_file] [--no-mongo]
#
# Options:
#   --no-mongo    Skip MongoDB/Docker startup

CONFIG_FILE="config.toml"
BINARY_DIR="./target/release"
SKIP_MONGO=false

# Parse arguments
for arg in "$@"; do
    case $arg in
        --no-mongo)
            SKIP_MONGO=true
            ;;
        *.toml)
            CONFIG_FILE="$arg"
            ;;
    esac
done

# Log level configuration
# For specific component: RUST_LOG=info,delila_rs::merger=debug ./scripts/start_daq.sh
# Force info level unless explicitly set before script runs
if [ -z "$RUST_LOG_SET" ]; then
    export RUST_LOG="info"
fi
export RUST_LOG_SET=1

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
CYAN='\033[0;36m'
NC='\033[0m' # No Color

echo -e "${GREEN}=== DELILA DAQ Startup ===${NC}"
echo "Config: $CONFIG_FILE"

# Kill any leftover DAQ processes from previous sessions
KILLED=false
for proc in operator monitor recorder merger reader emulator data_sink online_event_builder; do
    if pkill -f "target/release/$proc" 2>/dev/null; then
        KILLED=true
    fi
done
# root_sink is script-managed but lives outside target/release (deployed to
# ~/.local/bin / PATH), so match the exact process name — never a broad -f.
if pkill -x root_sink 2>/dev/null; then
    KILLED=true
fi
if [ "$KILLED" = true ]; then
    echo -e "${YELLOW}Killed leftover DAQ processes from previous session${NC}"
    # Wait for processes to actually exit (up to 5s), then force kill
    for i in $(seq 1 10); do
        STILL_RUNNING=false
        for proc in operator monitor recorder merger reader emulator data_sink online_event_builder; do
            if pgrep -f "target/release/$proc" &>/dev/null; then
                STILL_RUNNING=true
                break
            fi
        done
        pgrep -x root_sink &>/dev/null && STILL_RUNNING=true
        [ "$STILL_RUNNING" = false ] && break
        sleep 0.5
    done
    # Force kill any remaining stragglers
    for proc in operator monitor recorder merger reader emulator data_sink online_event_builder; do
        pkill -9 -f "target/release/$proc" 2>/dev/null
    done
    pkill -9 -x root_sink 2>/dev/null
    sleep 0.5  # brief wait for kernel to release sockets after SIGKILL
fi

# MongoDB configuration.
# Hosts with a local mongo (gant, .76 — Docker on localhost) use the defaults.
# Hosts without one (e.g. es2) override MONGODB_URI/MONGODB_DATABASE to point at
# a remote instance. Precedence: existing env var > scripts/mongodb.local.env
# (git-ignored, per-host) > the localhost default below.
if [ -f "$(dirname "$0")/mongodb.local.env" ]; then
    # shellcheck disable=SC1091
    . "$(dirname "$0")/mongodb.local.env"
fi
# Whether a URI was chosen deliberately (env var or the per-host override file).
# If it was, it is never second-guessed below; if it is just the built-in
# default, an unauthenticated localhost fallback is allowed.
MONGODB_URI_EXPLICIT="${MONGODB_URI:+yes}"
MONGODB_URI="${MONGODB_URI:-mongodb://delila:delila_pass@localhost:27017}"
MONGODB_DATABASE="${MONGODB_DATABASE:-delila}"

# CAEN FELib isolated prefix (see scripts/setup_caen_felib.sh). FELib dlopen()s
# its dig1/dig2 backends at runtime, and dlopen does NOT use the binaries' baked
# rpath for that lookup — it needs LD_LIBRARY_PATH. Interactive logins get this
# from ~/.bashrc, but non-interactive runs (cron, ssh "cmd", systemd) do not, and
# the resulting failure is obscure: CAEN -10 "DEVICE LIBRARY NOT AVAILABLE".
# Prepend it here when the prefix exists so the DAQ starts from any shell.
CAEN_PREFIX_LIB="${CAEN_PREFIX:-/opt/delila-caen}/lib"
if [ -d "$CAEN_PREFIX_LIB" ] && [[ ":${LD_LIBRARY_PATH}:" != *":${CAEN_PREFIX_LIB}:"* ]]; then
    export LD_LIBRARY_PATH="${CAEN_PREFIX_LIB}${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
fi

# Check if config exists
if [ ! -f "$CONFIG_FILE" ]; then
    echo -e "${RED}Error: Config file not found: $CONFIG_FILE${NC}"
    exit 1
fi

# Build if needed
if [ ! -f "$BINARY_DIR/emulator" ]; then
    echo -e "${YELLOW}Building release binaries...${NC}"
    cargo build --release
fi

# Check if MongoDB is available (unless --no-mongo)
MONGO_AVAILABLE=false
if [ "$SKIP_MONGO" = false ]; then
    echo ""
    echo -e "${CYAN}=== Checking MongoDB ===${NC}"

    # A natively installed mongod (systemd, e.g. eliadeSN01) or any already
    # reachable server at MONGODB_URI needs no Docker at all. Probe it FIRST,
    # otherwise a Docker-less host reports "Continuing without run history
    # persistence" while the Operator is in fact recording happily — a false
    # alarm that reads like a broken DAQ.
    mongo_ping() { mongosh --quiet "$1" --eval 'db.runCommand({ping:1}).ok' &>/dev/null; }
    if command -v mongosh &>/dev/null; then
        if mongo_ping "$MONGODB_URI"; then
            echo -e "  ${GREEN}MongoDB is running (native/remote)${NC}"
            MONGO_AVAILABLE=true
        elif [ -z "$MONGODB_URI_EXPLICIT" ] && mongo_ping "mongodb://localhost:27017"; then
            # The built-in default carries the credentials of the Docker mongo
            # used on the dev boxes. A distro mongodb-org install has
            # authentication disabled, so that default cannot connect and run
            # history would be dropped on every freshly provisioned node.
            # Prefer an unauthenticated localhost server over no history at all.
            MONGODB_URI="mongodb://localhost:27017"
            echo -e "  ${GREEN}MongoDB is running (native, authentication disabled)${NC}"
            MONGO_AVAILABLE=true
        fi
    fi
fi

# Docker/Colima fallback: only when no server answered above.
if [ "$SKIP_MONGO" = false ] && [ "$MONGO_AVAILABLE" = false ]; then

    # Check if Docker is available, if not try to start Colima (macOS)
    if ! docker info &>/dev/null; then
        if command -v colima &> /dev/null; then
            echo -e "  ${YELLOW}Docker not running, starting Colima...${NC}"
            colima start 2>/dev/null
            sleep 2
            if docker info &>/dev/null; then
                echo -e "  ${GREEN}Colima started successfully${NC}"
            else
                echo -e "  ${RED}Failed to start Colima${NC}"
            fi
        else
            echo -e "  ${YELLOW}Docker not available${NC}"
        fi
    fi

    # Check if MongoDB container is running, start if needed
    if docker info &>/dev/null; then
        if docker ps --format '{{.Names}}' 2>/dev/null | grep -q "delila_mongo"; then
            # Verify MongoDB is responding
            if docker exec delila_mongo mongosh --quiet --eval "db.runCommand('ping').ok" &>/dev/null; then
                echo -e "  ${GREEN}MongoDB is running${NC}"
                MONGO_AVAILABLE=true
            else
                echo -e "  ${YELLOW}MongoDB container exists but not responding${NC}"
            fi
        else
            # Try to start MongoDB container
            echo -e "  ${YELLOW}MongoDB not running, starting...${NC}"
            SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
            PROJECT_DIR="$(dirname "$SCRIPT_DIR")"
            if [ -f "$PROJECT_DIR/docker/docker-compose.yml" ]; then
                (cd "$PROJECT_DIR/docker" && docker compose up -d 2>/dev/null || docker-compose up -d 2>/dev/null)
                sleep 3
                if docker exec delila_mongo mongosh --quiet --eval "db.runCommand('ping').ok" &>/dev/null; then
                    echo -e "  ${GREEN}MongoDB started successfully${NC}"
                    MONGO_AVAILABLE=true
                else
                    echo -e "  ${YELLOW}MongoDB container started but not responding yet${NC}"
                fi
            else
                echo -e "  ${YELLOW}docker-compose.yml not found${NC}"
            fi
        fi
    else
        echo -e "  ${YELLOW}Docker not available${NC}"
    fi

    if [ "$MONGO_AVAILABLE" = false ]; then
        echo -e "  ${YELLOW}Continuing without run history persistence${NC}"
    fi
fi

if [ "$SKIP_MONGO" = true ]; then
    echo ""
    echo -e "${YELLOW}Skipping MongoDB check (--no-mongo)${NC}"
fi

# Function to get source type (emulator, psd1, psd2, pha1, zle)
get_source_type() {
    local src_id=$1
    # Use awk to get the type field for this source ID
    awk -v target_id="$src_id" '
        /^\[\[network\.sources\]\]/ {
            # When entering a new block, if we were in target, print and exit
            if (in_target) { print src_type; printed=1; exit }
            in_block=1; in_target=0; src_type="emulator"; next
        }
        in_block && /^\[/ {
            if (in_target) { print src_type; printed=1; exit }
            in_block=0; in_target=0
        }
        in_block && /^id *=/ {
            gsub(/[^0-9]/, "", $3)
            if ($3 == target_id) in_target=1
            else in_target=0
        }
        in_block && in_target && /^type *=/ {
            gsub(/.*= *"/, "", $0)
            gsub(/".*/, "", $0)
            src_type=$0
        }
        END { if (in_target && !printed) print src_type }
    ' "$CONFIG_FILE"
}

# Function to check if source is emulator
is_emulator() {
    local src_type=$(get_source_type $1)
    [ "$src_type" = "emulator" ] || [ -z "$src_type" ]
}

# Function to get source host (defaults to "localhost")
get_source_host() {
    local src_id=$1
    local host
    host=$(awk -v target_id="$src_id" '
        /^\[\[network\.sources\]\]/ {
            if (in_target) { print src_host; printed=1; exit }
            in_block=1; in_target=0; src_host="localhost"; next
        }
        in_block && /^\[/ {
            if (in_target) { print src_host; printed=1; exit }
            in_block=0; in_target=0
        }
        in_block && /^id *=/ {
            gsub(/[^0-9]/, "", $3)
            if ($3 == target_id) in_target=1
            else in_target=0
        }
        in_block && in_target && /^host *=/ {
            gsub(/.*= *"/, "", $0)
            gsub(/".*/, "", $0)
            src_host=$0
        }
        END { if (in_target && !printed) print src_host }
    ' "$CONFIG_FILE")
    echo "${host:-localhost}"
}

# Function to check if source is remote (host != localhost)
is_remote() {
    local host=$(get_source_host $1)
    [ "$host" != "localhost" ] && [ "$host" != "127.0.0.1" ]
}

# Function to get one key's value from the [network.root_sink] section.
# Scalars only (string/int/float); arrays/tables are not supported. Empty output
# means the key is absent.
get_root_sink_key() {
    local key=$1
    awk -v target_key="$key" '
        /^\[network\.root_sink\]/ { in_section=1; next }
        in_section && /^\[/ { exit }
        in_section && $0 ~ "^" target_key " *=" {
            sub(/^[^=]*= */, "", $0)   # drop "key ="
            sub(/ +#.*$/, "", $0)      # drop trailing " # comment"
            gsub(/^"|"$/, "", $0)      # strip surrounding double quotes
            print
            exit
        }
    ' "$CONFIG_FILE"
}

# Append "<flag> <value>" to RS_ARGS only if the TOML key is present.
# RS_ARGS is a global array populated by the root_sink launch block below.
rs_add_arg() {
    local val
    val=$(get_root_sink_key "$1")
    [ -n "$val" ] && RS_ARGS+=("$2" "$val")
}

# Extract source IDs from config
SOURCE_IDS=$(grep -E "^id = " "$CONFIG_FILE" | head -n $(grep -c "\[\[network.sources\]\]" "$CONFIG_FILE") | awk '{print $3}')

# Extract operator port from config (default: 9090)
OPERATOR_PORT=$(awk '/^\[operator\]/{in_op=1} in_op && /^port *=/{print $3; exit}' "$CONFIG_FILE")
OPERATOR_PORT=${OPERATOR_PORT:-9090}

echo ""
echo -e "${GREEN}Starting components...${NC}"

# Create log directory with timestamp
TIMESTAMP=$(date +%Y%m%d_%H%M%S)
LOG_DIR="./logs/${TIMESTAMP}"
mkdir -p "$LOG_DIR"

# Create symlink to latest logs
rm -f ./logs/latest
ln -sf "${TIMESTAMP}" ./logs/latest

# Track component names and PIDs for summary
declare -a COMP_NAMES=()
declare -a COMP_PIDS=()

# Start emulators or readers based on source type
REMOTE_SOURCES=()
for id in $SOURCE_IDS; do
    src_type=$(get_source_type $id)
    if is_remote $id; then
        host=$(get_source_host $id)
        echo -e "  ${YELLOW}Skipping source $id ($src_type) — remote at $host${NC}"
        REMOTE_SOURCES+=("$id")
    elif is_emulator $id; then
        echo "  Starting emulator (source_id=$id)..."
        $BINARY_DIR/emulator --config "$CONFIG_FILE" --source-id "$id" > "$LOG_DIR/emulator_$id.log" 2>&1 &
        COMP_NAMES+=("Emulator $id")
        COMP_PIDS+=($!)
    else
        echo "  Starting reader (source_id=$id) [type=$src_type]..."
        $BINARY_DIR/reader --config "$CONFIG_FILE" --source-id "$id" > "$LOG_DIR/reader_$id.log" 2>&1 &
        COMP_NAMES+=("Reader $id ($src_type)")
        COMP_PIDS+=($!)
    fi
done

# Start merger
echo "  Starting merger..."
$BINARY_DIR/merger --config "$CONFIG_FILE" > "$LOG_DIR/merger.log" 2>&1 &
COMP_NAMES+=("Merger")
COMP_PIDS+=($!)

# Start recorder
echo "  Starting recorder..."
$BINARY_DIR/recorder --config "$CONFIG_FILE" > "$LOG_DIR/recorder.log" 2>&1 &
COMP_NAMES+=("Recorder")
COMP_PIDS+=($!)

# Start monitor
echo "  Starting monitor..."
$BINARY_DIR/monitor --config "$CONFIG_FILE" > "$LOG_DIR/monitor.log" 2>&1 &
COMP_NAMES+=("Monitor")
COMP_PIDS+=($!)

# Start online event builder (if configured and binary exists)
if grep -q "^\[network\.event_builder\]" "$CONFIG_FILE" 2>/dev/null; then
    if [ -f "$BINARY_DIR/online_event_builder" ]; then
        echo "  Starting online event builder..."
        $BINARY_DIR/online_event_builder --config "$CONFIG_FILE" > "$LOG_DIR/event_builder.log" 2>&1 &
        COMP_NAMES+=("EventBuilder")
        COMP_PIDS+=($!)
    else
        echo -e "  ${YELLOW}Event Builder configured but binary not found (build with --features root)${NC}"
    fi
fi

# Start root_sink (parallel C++ ROOT sink, if configured and binary found).
# It is a standalone binary deployed outside target/release; a missing binary is
# a warning, not fatal (same spirit as the event builder branch above).
ROOT_SINK_LAUNCHED=false
ROOT_SINK_HTTP_PORT=""
if grep -q "^\[network\.root_sink\]" "$CONFIG_FILE" 2>/dev/null; then
    # Binary search order: ROOT_SINK_BIN env override > PATH > ~/.local/bin >
    # tools/root_sink. An explicit env override that is not executable is caught
    # below rather than silently falling through.
    if [ -z "$ROOT_SINK_BIN" ]; then
        if command -v root_sink &>/dev/null; then
            ROOT_SINK_BIN=$(command -v root_sink)
        elif [ -x "$HOME/.local/bin/root_sink" ]; then
            ROOT_SINK_BIN="$HOME/.local/bin/root_sink"
        elif [ -x "./tools/root_sink/root_sink" ]; then
            ROOT_SINK_BIN="./tools/root_sink/root_sink"
        fi
    fi

    if [ -z "$ROOT_SINK_BIN" ] || [ ! -x "$ROOT_SINK_BIN" ]; then
        echo -e "  ${YELLOW}root_sink configured but binary not found — skipping (build: see tools/root_sink/README.md)${NC}"
    else
        # Map TOML key -> CLI flag; only present keys become flags (root_sink has
        # sane defaults for the rest).
        RS_ARGS=()
        rs_add_arg subscribe    --zmq
        rs_add_arg output_dir   --out-dir
        rs_add_arg tree         --tree
        rs_add_arg exp_name     --exp-name
        rs_add_arg hists        --hists
        rs_add_arg gamma_ch     --gamma-ch
        rs_add_arg thgem1_ch    --thgem1-ch
        rs_add_arg thgem2_ch    --thgem2-ch
        rs_add_arg window_ns    --window-ns
        rs_add_arg margin_ns    --margin-ns
        rs_add_arg http_port    --http-port
        rs_add_arg dt_bins      --dt-bins
        rs_add_arg dt_min       --dt-min
        rs_add_arg dt_max       --dt-max
        rs_add_arg autosave_sec --autosave-sec
        # Always derive --operator from [operator] port; an explicit exp_name key
        # still wins inside root_sink, so this default is always safe and yields
        # Recorder-matching filenames.
        RS_ARGS+=(--operator "http://localhost:${OPERATOR_PORT}")

        echo "  Starting root_sink..."
        "$ROOT_SINK_BIN" "${RS_ARGS[@]}" > "$LOG_DIR/root_sink.log" 2>&1 &
        COMP_NAMES+=("RootSink")
        COMP_PIDS+=($!)
        ROOT_SINK_LAUNCHED=true
        ROOT_SINK_HTTP_PORT=$(get_root_sink_key http_port)
    fi
fi

# Start operator (Web UI)
echo "  Starting operator (Web UI)..."
if [ "$MONGO_AVAILABLE" = true ]; then
    $BINARY_DIR/operator --config "$CONFIG_FILE" \
        --mongodb-uri "$MONGODB_URI" \
        --mongodb-database "$MONGODB_DATABASE" \
        > "$LOG_DIR/operator.log" 2>&1 &
    echo "    (with MongoDB for run history)"
else
    $BINARY_DIR/operator --config "$CONFIG_FILE" > "$LOG_DIR/operator.log" 2>&1 &
    echo "    (without MongoDB)"
fi
COMP_NAMES+=("Operator")
COMP_PIDS+=($!)

echo ""

# --- Health check: wait for Operator API ---
echo -e "${CYAN}=== Health Check ===${NC}"
echo -n "  Waiting for Operator API (port $OPERATOR_PORT)..."
MAX_RETRIES=15
HEALTH_OK=false
STATUS_JSON=""
for i in $(seq 1 $MAX_RETRIES); do
    STATUS_JSON=$(curl -s --max-time 2 "http://localhost:${OPERATOR_PORT}/api/status" 2>/dev/null)
    if [ $? -eq 0 ] && [ -n "$STATUS_JSON" ]; then
        HEALTH_OK=true
        echo -e " ${GREEN}ready${NC} (${i}s)"
        break
    fi
    echo -n "."
    sleep 1
done

if [ "$HEALTH_OK" = false ]; then
    echo -e " ${RED}timeout${NC}"
    echo -e "  ${YELLOW}Operator did not respond within ${MAX_RETRIES}s. Check $LOG_DIR/operator.log${NC}"
fi

# --- Startup summary table ---
echo ""
echo -e "${GREEN}=== Startup Summary ===${NC}"
printf "  %-24s  %7s  %s\n" "Component" "PID" "Status"
printf "  %-24s  %7s  %s\n" "------------------------" "-------" "----------"

for idx in "${!COMP_NAMES[@]}"; do
    name="${COMP_NAMES[$idx]}"
    pid="${COMP_PIDS[$idx]}"
    if kill -0 "$pid" 2>/dev/null; then
        printf "  %-24s  %7s  ${GREEN}%s${NC}\n" "$name" "$pid" "running"
    else
        printf "  %-24s  %7s  ${RED}%s${NC}\n" "$name" "$pid" "DEAD"
    fi
done

# Show component states from Operator API if available
if [ "$HEALTH_OK" = true ] && command -v python3 &>/dev/null; then
    echo ""
    echo -e "${CYAN}=== Component States (from Operator) ===${NC}"
    echo "$STATUS_JSON" | python3 -c "
import sys, json
try:
    data = json.load(sys.stdin)
    comps = data.get('components', [])
    print(f'  System state: {data.get(\"system_state\", \"unknown\")}')
    print(f'  Components:   {len(comps)}')
    for c in comps:
        state = c.get('state', 'Unknown')
        name = c.get('name', '?')
        print(f'    {name:24s}  {state}')
except:
    pass
" 2>/dev/null
fi

# Print remote Reader instructions if any
if [ ${#REMOTE_SOURCES[@]} -gt 0 ]; then
    echo ""
    echo -e "${CYAN}=== Remote Readers ===${NC}"
    echo -e "Start these Readers on the remote machines:"
    echo ""
    for id in "${REMOTE_SOURCES[@]}"; do
        host=$(get_source_host $id)
        src_type=$(get_source_type $id)
        echo -e "  ${YELLOW}[$host] source_id=$id ($src_type):${NC}"
        echo "    ./reader --config $CONFIG_FILE --source-id $id"
        echo ""
    done
fi

echo ""
echo -e "${CYAN}=== Web UI ===${NC}"
echo -e "  Swagger UI:    ${YELLOW}http://localhost:${OPERATOR_PORT}/swagger-ui/${NC}"
echo -e "  Monitor:       ${YELLOW}http://localhost:8081/${NC}"
if [ "$MONGO_AVAILABLE" = true ]; then
    echo -e "  Mongo Express: ${YELLOW}http://localhost:8082/${NC}"
fi
# root_sink THttpServer (default port 8090; skipped when http_port = 0).
if [ "$ROOT_SINK_LAUNCHED" = true ]; then
    RS_HTTP_PORT="${ROOT_SINK_HTTP_PORT:-8090}"
    if [ "$RS_HTTP_PORT" != "0" ]; then
        echo -e "  RootSink:      ${YELLOW}http://localhost:${RS_HTTP_PORT}/${NC}"
    fi
fi
echo ""
echo -e "${CYAN}=== Logs ===${NC}"
echo -e "  Log directory: ${YELLOW}$LOG_DIR/${NC}"
echo -e "  Latest link:   ${YELLOW}./logs/latest/${NC}"
echo -e "  View logs:     ${YELLOW}tail -f ./logs/latest/*.log${NC}"
echo ""
echo -e "${YELLOW}Use ./scripts/daq_ctl.sh to control components (CLI)${NC}"
echo -e "${YELLOW}Use ./scripts/stop_daq.sh to stop all components${NC}"
