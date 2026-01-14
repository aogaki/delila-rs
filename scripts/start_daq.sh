#!/bin/bash
# DELILA DAQ Start Script
# Usage: ./scripts/start_daq.sh [config_file]

CONFIG_FILE="${1:-config.toml}"
BINARY_DIR="./target/release"

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

# Start emulators or readers based on config
for id in $SOURCE_IDS; do
    if [ "$(has_digitizer_url $id)" = "yes" ]; then
        echo "  Starting reader (source_id=$id) [digitizer]..."
        $BINARY_DIR/reader --config "$CONFIG_FILE" --source-id "$id" &
    else
        echo "  Starting emulator (source_id=$id)..."
        $BINARY_DIR/emulator --config "$CONFIG_FILE" --source-id "$id" &
    fi
    sleep 0.3
done

# Start merger
echo "  Starting merger..."
$BINARY_DIR/merger --config "$CONFIG_FILE" &
sleep 0.3

# Start data sink
echo "  Starting data_sink..."
$BINARY_DIR/data_sink --config "$CONFIG_FILE" &
sleep 0.3

# Start operator (Web UI)
echo "  Starting operator (Web UI)..."
$BINARY_DIR/operator --config "$CONFIG_FILE" &
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
echo "  DataSink:   tcp://localhost:5580"
echo ""
echo -e "${CYAN}=== Web UI ===${NC}"
echo -e "  Swagger UI: ${YELLOW}http://localhost:8080/swagger-ui/${NC}"
echo ""
echo -e "${YELLOW}Use ./scripts/daq_ctl.sh to control components (CLI)${NC}"
echo -e "${YELLOW}Use ./scripts/stop_daq.sh to stop all components${NC}"
