#!/bin/bash
# DELILA DAQ Stop Script
# Usage: ./scripts/stop_daq.sh [--with-docker]
#
# Options:
#   --with-docker    Also stop Docker containers (MongoDB, Mongo Express)

STOP_DOCKER=false

# Parse arguments
for arg in "$@"; do
    case $arg in
        --with-docker)
            STOP_DOCKER=true
            shift
            ;;
    esac
done

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
CYAN='\033[0;36m'
NC='\033[0m'

echo -e "${GREEN}=== Stopping DELILA DAQ ===${NC}"

pkill -f "target/release/operator" 2>/dev/null && echo "  Stopped operator"
pkill -f "target/release/monitor" 2>/dev/null && echo "  Stopped monitor"
pkill -f "target/release/recorder" 2>/dev/null && echo "  Stopped recorder"
pkill -f "target/release/merger" 2>/dev/null && echo "  Stopped merger"
pkill -f "target/release/reader" 2>/dev/null && echo "  Stopped readers"
pkill -f "target/release/emulator" 2>/dev/null && echo "  Stopped emulators"
pkill -f "target/release/data_sink" 2>/dev/null && echo "  Stopped data_sink"

echo -e "${GREEN}All DAQ components stopped.${NC}"

# Stop Docker containers if requested
if [ "$STOP_DOCKER" = true ]; then
    echo ""
    echo -e "${CYAN}=== Stopping Docker containers ===${NC}"
    DOCKER_DIR="./docker"
    if [ -f "$DOCKER_DIR/docker-compose.yml" ]; then
        if docker ps --format '{{.Names}}' | grep -q "delila_"; then
            (cd "$DOCKER_DIR" && docker compose down 2>/dev/null) || \
            (cd "$DOCKER_DIR" && docker-compose down 2>/dev/null)
            echo -e "  ${GREEN}Docker containers stopped${NC}"
        else
            echo "  No DELILA Docker containers running"
        fi
    fi
else
    # Check if MongoDB is running and inform user
    if docker ps --format '{{.Names}}' 2>/dev/null | grep -q "delila_mongo"; then
        echo ""
        echo -e "${YELLOW}Note: MongoDB is still running. Use --with-docker to stop it.${NC}"
    fi
fi
