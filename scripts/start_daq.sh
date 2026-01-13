#!/bin/bash
# DELILA DAQ Start Script
# Usage: ./scripts/start_daq.sh [config_file]

CONFIG_FILE="${1:-config.toml}"
BINARY_DIR="./target/release"

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
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

# Extract source IDs from config
SOURCE_IDS=$(grep -E "^id = " "$CONFIG_FILE" | awk '{print $3}')

echo ""
echo -e "${GREEN}Starting components...${NC}"

# Start emulators
for id in $SOURCE_IDS; do
    echo "  Starting emulator (source_id=$id)..."
    $BINARY_DIR/emulator --config "$CONFIG_FILE" --source-id "$id" &
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

echo ""
echo -e "${GREEN}All components started.${NC}"
echo ""
echo "Command ports:"
for id in $SOURCE_IDS; do
    port=$((5560 + id))
    echo "  Emulator $id: tcp://localhost:$port"
done
echo "  Merger:     tcp://localhost:5570"
echo "  DataSink:   tcp://localhost:5580"
echo ""
echo -e "${YELLOW}Use ./scripts/daq_ctl.sh to control components${NC}"
echo -e "${YELLOW}Use ./scripts/stop_daq.sh to stop all components${NC}"
