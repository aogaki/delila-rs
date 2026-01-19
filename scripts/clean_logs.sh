#!/bin/bash
# DELILA DAQ Log Cleanup Script
# Usage: ./scripts/clean_logs.sh [days_to_keep]
# Default: keep logs from last 7 days

DAYS_TO_KEEP="${1:-7}"
LOG_BASE="./logs"

echo "Cleaning logs older than $DAYS_TO_KEEP days..."

# Find and remove old log directories (but not 'latest' symlink)
find "$LOG_BASE" -maxdepth 1 -type d -name "20*" -mtime +$DAYS_TO_KEEP -exec rm -rf {} \; 2>/dev/null

# Count remaining
REMAINING=$(find "$LOG_BASE" -maxdepth 1 -type d -name "20*" 2>/dev/null | wc -l)
echo "Remaining log sessions: $REMAINING"

# Show disk usage
if [ -d "$LOG_BASE" ]; then
    echo "Total log size: $(du -sh "$LOG_BASE" 2>/dev/null | cut -f1)"
fi
