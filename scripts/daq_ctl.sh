#!/bin/bash
# DELILA DAQ Control Script (5-state machine)
# Usage: ./scripts/daq_ctl.sh <command> [component] [options]
#
# Commands: configure, arm, start, stop, reset, status
# Components: all, emulators, merger, sink, or specific port number
#
# Examples:
#   ./scripts/daq_ctl.sh status                      # Status of all components
#   ./scripts/daq_ctl.sh configure --run 123         # Configure all with run 123
#   ./scripts/daq_ctl.sh arm                         # Arm all components
#   ./scripts/daq_ctl.sh start                       # Start all components
#   ./scripts/daq_ctl.sh stop                        # Stop all components
#   ./scripts/daq_ctl.sh reset                       # Reset all to Idle
#   ./scripts/daq_ctl.sh configure merger --run 123  # Configure merger only

BINARY_DIR="./target/release"
CONTROLLER="$BINARY_DIR/controller"

# Default ports (matching config.toml with 0-indexed sources)
# Source 0: Emulator, Source 1: Reader (digitizer)
SOURCE_PORTS="5560 5561"
MERGER_PORT="5570"
SINK_PORT="5580"

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
CYAN='\033[0;36m'
NC='\033[0m'

show_help() {
    echo "DELILA DAQ Control (5-state machine)"
    echo ""
    echo "Usage: $0 <command> [component] [options]"
    echo ""
    echo "Commands:"
    echo "  configure  - Configure for run (Idle → Configured)"
    echo "  arm        - Prepare for acquisition (Configured → Armed)"
    echo "  start      - Begin data acquisition (Armed → Running)"
    echo "  stop       - Stop acquisition (Running → Configured)"
    echo "  reset      - Reset to idle state (Any → Idle)"
    echo "  status     - Get component status"
    echo ""
    echo "Components (optional, default: all):"
    echo "  all       - All components"
    echo "  sources   - All sources (emulators + readers)"
    echo "  merger    - Merger only"
    echo "  sink      - Data sink only"
    echo "  <port>    - Specific port number"
    echo ""
    echo "Options for 'configure':"
    echo "  --run <number>   Run number (required)"
    echo ""
    echo "Examples:"
    echo "  $0 configure --run 123          # Configure all for run 123"
    echo "  $0 arm                           # Arm all"
    echo "  $0 start                         # Start all"
    echo "  $0 stop                          # Stop all"
    echo "  $0 reset                         # Reset all"
    echo "  $0 status merger                 # Check merger status"
    echo ""
    echo "Typical run sequence:"
    echo "  $0 configure --run 123"
    echo "  $0 arm"
    echo "  $0 start"
    echo "  # ... data acquisition ..."
    echo "  $0 stop"
    echo "  # For new run: $0 configure --run 124"
    echo "  # To fully reset: $0 reset"
}

send_command() {
    local cmd=$1
    local port=$2
    local name=$3
    shift 3
    local extra_args="$@"

    echo -e "${CYAN}[$name]${NC} tcp://localhost:$port"
    $CONTROLLER "$cmd" "tcp://localhost:$port" $extra_args 2>/dev/null | grep -E "(State|Success|Message|Run)" | sed 's/^/  /'
    echo ""
}

if [ $# -lt 1 ]; then
    show_help
    exit 1
fi

CMD=$1
shift

# Parse component and options
COMPONENT="all"
EXTRA_ARGS=""

while [ $# -gt 0 ]; do
    case $1 in
        all|sources|emulators|merger|sink)
            COMPONENT=$1
            shift
            ;;
        [0-9]*)
            COMPONENT=$1
            shift
            ;;
        --run)
            EXTRA_ARGS="$EXTRA_ARGS --run $2"
            shift 2
            ;;
        --comment)
            EXTRA_ARGS="$EXTRA_ARGS --comment \"$2\""
            shift 2
            ;;
        *)
            echo -e "${RED}Unknown option: $1${NC}"
            show_help
            exit 1
            ;;
    esac
done

case $CMD in
    configure|arm|start|stop|reset|status)
        ;;
    -h|--help|help)
        show_help
        exit 0
        ;;
    *)
        echo -e "${RED}Unknown command: $CMD${NC}"
        show_help
        exit 1
        ;;
esac

# Validate configure has --run
if [ "$CMD" = "configure" ] && [[ ! "$EXTRA_ARGS" =~ "--run" ]]; then
    echo -e "${RED}Error: configure requires --run <number>${NC}"
    echo "Example: $0 configure --run 123"
    exit 1
fi

echo -e "${GREEN}=== DAQ Control: $CMD ===${NC}"
echo ""

case $COMPONENT in
    all)
        for port in $SOURCE_PORTS; do
            id=$((port - 5560))
            send_command "$CMD" "$port" "Source $id" $EXTRA_ARGS
        done
        send_command "$CMD" "$MERGER_PORT" "Merger" $EXTRA_ARGS
        send_command "$CMD" "$SINK_PORT" "DataSink" $EXTRA_ARGS
        ;;
    sources|emulators)
        for port in $SOURCE_PORTS; do
            id=$((port - 5560))
            send_command "$CMD" "$port" "Source $id" $EXTRA_ARGS
        done
        ;;
    merger)
        send_command "$CMD" "$MERGER_PORT" "Merger" $EXTRA_ARGS
        ;;
    sink)
        send_command "$CMD" "$SINK_PORT" "DataSink" $EXTRA_ARGS
        ;;
    [0-9]*)
        send_command "$CMD" "$COMPONENT" "Port $COMPONENT" $EXTRA_ARGS
        ;;
    *)
        echo -e "${RED}Unknown component: $COMPONENT${NC}"
        exit 1
        ;;
esac
