#!/bin/bash
# DELILA DAQ Stop Script
# Usage: ./scripts/stop_daq.sh

RED='\033[0;31m'
GREEN='\033[0;32m'
NC='\033[0m'

echo -e "${GREEN}=== Stopping DELILA DAQ ===${NC}"

pkill -f "target/release/operator" 2>/dev/null && echo "  Stopped operator"
pkill -f "target/release/monitor" 2>/dev/null && echo "  Stopped monitor"
pkill -f "target/release/recorder" 2>/dev/null && echo "  Stopped recorder"
pkill -f "target/release/merger" 2>/dev/null && echo "  Stopped merger"
pkill -f "target/release/reader" 2>/dev/null && echo "  Stopped readers"
pkill -f "target/release/emulator" 2>/dev/null && echo "  Stopped emulators"
pkill -f "target/release/data_sink" 2>/dev/null && echo "  Stopped data_sink"

echo -e "${GREEN}All components stopped.${NC}"
