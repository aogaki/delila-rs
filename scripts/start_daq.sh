#!/bin/bash
# DELILA DAQ Start Script
# Usage: ./scripts/start_daq.sh [config_file]

CONFIG_FILE="${1:-config.toml}"
BINARY_DIR="./target/release"

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

# Function to check if source has digitizer_url
has_digitizer_url() {
    local src_id=$1
    # Use awk to check if this source ID has a digitizer_url
    awk -v target_id="$src_id" '
        /^\[\[network\.sources\]\]/ { in_block=1; in_target=0; next }
        in_block && /^\[/ { in_block=0; in_target=0 }
        in_block && /^id *=/ {
            gsub(/[^0-9]/, "", $3)
            if ($3 == target_id) in_target=1
            else in_target=0
        }
        in_block && in_target && /^digitizer_url *=/ { print "yes"; exit }
    ' "$CONFIG_FILE"
}

# Extract source IDs from config
SOURCE_IDS=$(grep -E "^id = " "$CONFIG_FILE" | head -n $(grep -c "\[\[network.sources\]\]" "$CONFIG_FILE") | awk '{print $3}')

echo ""
echo -e "${GREEN}Starting components...${NC}"

# Create log directory with timestamp
TIMESTAMP=$(date +%Y%m%d_%H%M%S)
LOG_DIR="./logs/${TIMESTAMP}"
mkdir -p "$LOG_DIR"

# Create symlink to latest logs
rm -f ./logs/latest
ln -sf "${TIMESTAMP}" ./logs/latest

# Start emulators or readers based on config
for id in $SOURCE_IDS; do
    if [ "$(has_digitizer_url $id)" = "yes" ]; then
        echo "  Starting reader (source_id=$id) [digitizer]..."
        $BINARY_DIR/reader --config "$CONFIG_FILE" --source-id "$id" > "$LOG_DIR/reader_$id.log" 2>&1 &
    else
        echo "  Starting emulator (source_id=$id)..."
        $BINARY_DIR/emulator --config "$CONFIG_FILE" --source-id "$id" > "$LOG_DIR/emulator_$id.log" 2>&1 &
    fi
    sleep 0.3
done

# Start merger
echo "  Starting merger..."
$BINARY_DIR/merger --config "$CONFIG_FILE" > "$LOG_DIR/merger.log" 2>&1 &
sleep 0.3

# Start recorder
echo "  Starting recorder..."
$BINARY_DIR/recorder --config "$CONFIG_FILE" > "$LOG_DIR/recorder.log" 2>&1 &
sleep 0.3

# Start monitor
echo "  Starting monitor..."
$BINARY_DIR/monitor --config "$CONFIG_FILE" > "$LOG_DIR/monitor.log" 2>&1 &
sleep 0.3

# Start operator (Web UI)
echo "  Starting operator (Web UI)..."
$BINARY_DIR/operator --config "$CONFIG_FILE" > "$LOG_DIR/operator.log" 2>&1 &
sleep 0.5

echo ""
echo -e "${GREEN}All components started.${NC}"
echo ""
echo "Command ports:"
for id in $SOURCE_IDS; do
    port=$((5560 + id))
    if [ "$(has_digitizer_url $id)" = "yes" ]; then
        echo "  Reader $id:   tcp://localhost:$port (digitizer)"
    else
        echo "  Emulator $id: tcp://localhost:$port"
    fi
done
echo "  Merger:     tcp://localhost:5570"
echo "  Recorder:   tcp://localhost:5580"
echo "  Monitor:    tcp://localhost:5590"
echo ""
echo -e "${CYAN}=== Web UI ===${NC}"
echo -e "  Swagger UI: ${YELLOW}http://localhost:8080/swagger-ui/${NC}"
echo ""
echo -e "${CYAN}=== Logs ===${NC}"
echo -e "  Log directory: ${YELLOW}$LOG_DIR/${NC}"
echo -e "  Latest link:   ${YELLOW}./logs/latest/${NC}"
echo -e "  View logs:     ${YELLOW}tail -f ./logs/latest/*.log${NC}"
echo ""
echo -e "${YELLOW}Use ./scripts/daq_ctl.sh to control components (CLI)${NC}"
echo -e "${YELLOW}Use ./scripts/stop_daq.sh to stop all components${NC}"
