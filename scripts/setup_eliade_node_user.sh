#!/usr/bin/env bash
# setup_eliade_node_user.sh — the per-user half of an ELIADE node install.
#
# Run as the account that owns ~/delila-rs (no sudo). Assumes
# scripts/provision_eliade_node.sh has already done the root-level half:
# apt packages, MongoDB, the checkout, CAEN FELib in /opt/delila-caen and the
# A3818 driver.
#
# Usage:  bash scripts/setup_eliade_node_user.sh [OPTIONS]
#   --skip-build     stop after the environment is set up (no cargo/C++ build)
#   --skip-toolbox   do not install CAEN Toolbox
#
# Idempotent: re-running refreshes rather than duplicating (the ~/.bashrc block
# is delimited and rewritten in place).
#
# Steps:
#   1. Rust toolchain (rustup, minimal profile)
#   2. ~/.bashrc environment block: CAEN prefix on LD_LIBRARY_PATH, ~/.local/bin
#      on PATH, ROOT via thisroot.sh
#   3. CAEN Toolbox -> ~/.local/caen-toolbox (user-local; the vendor installer
#      wants /opt + /usr/local and therefore root)
#   4. cargo build --release --bins
#   5. root_sink + delila2root -> ~/.local/bin
set -u

DO_BUILD=1
DO_TOOLBOX=1
while [ $# -gt 0 ]; do
  case "$1" in
    --skip-build)   DO_BUILD=0;   shift ;;
    --skip-toolbox) DO_TOOLBOX=0; shift ;;
    -h|--help) sed -n '2,25p' "$0"; exit 0 ;;
    *) echo "unknown option: $1" >&2; exit 2 ;;
  esac
done

C='\033[0;36m'; G='\033[0;32m'; Y='\033[1;33m'; R='\033[0;31m'; N='\033[0m'
log()  { printf "${C}[setup]${N} %s\n" "$*"; }
ok()   { printf "${G}[setup] OK:${N} %s\n" "$*"; }
warn() { printf "${Y}[setup] WARN:${N} %s\n" "$*"; }
die()  { printf "${R}[setup] ERROR:${N} %s\n" "$*" >&2; exit 1; }

REPO="$HOME/delila-rs"
CAEN_PREFIX="${CAEN_PREFIX:-/opt/delila-caen}"
ROOTSYS_DIR="$HOME/root"
[ -d "$REPO" ] || die "no checkout at $REPO — run scripts/provision_eliade_node.sh first"
mkdir -p "$HOME/.local/bin"

log "host=$(hostname)  repo=$REPO"

# --------------------------------------------------------------- 1. Rust
if [ -x "$HOME/.cargo/bin/cargo" ]; then
  ok "Rust present: $("$HOME/.cargo/bin/rustc" --version)"
else
  log "installing Rust (rustup, minimal profile)"
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
    | sh -s -- -y --default-toolchain stable --profile minimal >/dev/null 2>&1 \
    || die "rustup install failed"
  ok "Rust installed: $("$HOME/.cargo/bin/rustc" --version)"
fi
export PATH="$HOME/.cargo/bin:$PATH"

# ------------------------------------------------------------ 2. ~/.bashrc
# Interactive convenience only. Ubuntu's ~/.bashrc returns early for
# non-interactive shells, so nothing here reaches cron/systemd/`ssh host cmd` —
# that is why scripts/start_daq.sh prepends the CAEN prefix itself rather than
# relying on this block.
BRC="$HOME/.bashrc"
BEGIN='# >>> delila-rs environment >>>'
END='# <<< delila-rs environment <<<'
[ -f "$BRC" ] || : > "$BRC"
if grep -qF "$BEGIN" "$BRC"; then
  log "refreshing the delila-rs block in ~/.bashrc"
  awk -v b="$BEGIN" -v e="$END" '
    index($0,b){skip=1} !skip{print} index($0,e){skip=0}' "$BRC" > "$BRC.tmp" \
    && mv "$BRC.tmp" "$BRC"
else
  log "adding a delila-rs block to ~/.bashrc"
fi

{
  echo ""
  echo "$BEGIN"
  echo "# CAEN FELib lives in an isolated prefix so the system CAEN libs stay"
  echo "# untouched; FELib dlopen()s its dig1 backend, which needs the path here."
  echo "[ -d \"$CAEN_PREFIX/lib\" ] && export LD_LIBRARY_PATH=\"$CAEN_PREFIX/lib\${LD_LIBRARY_PATH:+:\$LD_LIBRARY_PATH}\""
  echo "# delila-rs tools: root_sink, delila2root, caen-toolbox, plus cargo"
  echo "export PATH=\"\$HOME/.local/bin:\$HOME/.cargo/bin:\$PATH\""
  # Only add ROOT if the user is not already sourcing it elsewhere in .bashrc.
  if grep -q 'thisroot\.sh' "$BRC" 2>/dev/null; then
    echo "# ROOT is sourced elsewhere in this file — not repeated here."
  else
    echo "[ -f \"$ROOTSYS_DIR/bin/thisroot.sh\" ] && . \"$ROOTSYS_DIR/bin/thisroot.sh\""
  fi
  echo "$END"
} >> "$BRC"
ok "~/.bashrc updated (affects new interactive logins)"

# ------------------------------------------------------- 3. CAEN Toolbox
if [ "$DO_TOOLBOX" -eq 0 ]; then
  log "skipping CAEN Toolbox (--skip-toolbox)"
elif [ -x "$HOME/.local/bin/caen-toolbox" ]; then
  ok "CAEN Toolbox present: $("$HOME/.local/bin/caen-toolbox" --version 2>/dev/null | head -1)"
else
  TB=$(ls "$REPO"/external/CAEN_Toolbox-*.tar.gz /tmp/CAEN_Toolbox-*.tar.gz 2>/dev/null | head -1)
  if [ -n "$TB" ]; then
    log "installing CAEN Toolbox from $(basename "$TB")"
    TMP=$(mktemp -d); tar xzf "$TB" -C "$TMP"
    SRC=$(find "$TMP" -maxdepth 2 -type d -name caen-toolbox | head -1)
    if [ -n "$SRC" ]; then
      rm -rf "$HOME/.local/caen-toolbox"
      cp -a "$SRC" "$HOME/.local/caen-toolbox"
      ln -sf "$HOME/.local/caen-toolbox/caen-toolbox" "$HOME/.local/bin/caen-toolbox"
      ok "CAEN Toolbox: $("$HOME/.local/bin/caen-toolbox" --version 2>/dev/null | head -1)"
    else
      warn "unexpected tarball layout — Toolbox not installed"
    fi
    rm -rf "$TMP"
  else
    warn "no CAEN_Toolbox tarball found (stage it in /tmp or external/) — skipping"
  fi
fi

[ "$DO_BUILD" -eq 1 ] || { echo; ok "environment ready (build skipped)"; exit 0; }

# ------------------------------------------------------------ 4. delila-rs
[ -e "$CAEN_PREFIX/include/CAEN_FELib.h" ] \
  || die "CAEN FELib headers missing from $CAEN_PREFIX — run the provisioning script first"

# bindgen drives libclang directly, which does not pick up GCC's own header
# directory, so the CAEN headers' #include <stdarg.h> fails without this.
GCC_INC=$(ls -d /usr/lib/gcc/x86_64-linux-gnu/*/include 2>/dev/null | sort -V | tail -1)
[ -n "$GCC_INC" ] && export BINDGEN_EXTRA_CLANG_ARGS="-isystem $GCC_INC"
LIBCLANG_DIR=$(dirname "$(ls /usr/lib/x86_64-linux-gnu/libclang-*.so.1 /lib/x86_64-linux-gnu/libclang-*.so.1 2>/dev/null | head -1)" 2>/dev/null)
[ -n "$LIBCLANG_DIR" ] && export LIBCLANG_PATH="$LIBCLANG_DIR"
export CAEN_PREFIX

log "cargo build --release --bins  (first build takes several minutes)"
if (cd "$REPO" && cargo build --release --bins) >/tmp/delila_build.log 2>&1; then
  ok "delila-rs built: $(ls "$REPO"/target/release/ | grep -cxE 'operator|reader|merger|recorder|monitor|controller|node_agent|emulator') core binaries"
else
  tail -20 /tmp/delila_build.log
  die "cargo build failed — full log at /tmp/delila_build.log"
fi

# ------------------------------------------- 5. C++ tools (root_sink, delila2root)
if [ ! -x "$ROOTSYS_DIR/bin/root-config" ]; then
  warn "no ROOT at $ROOTSYS_DIR — skipping root_sink and delila2root"
else
  # shellcheck disable=SC1091
  . "$ROOTSYS_DIR/bin/thisroot.sh"
  RC="$ROOTSYS_DIR/bin/root-config"
  # ROOT 6.28 binaries are built as C++14 and root-config says so; these tools
  # need C++17, and the flag must come AFTER root-config's or it loses.
  # The rpath means the binaries run without thisroot.sh being sourced, which
  # is what start_daq.sh needs.
  COMMON="-O2 $($RC --cflags) -std=c++17"
  LINK="$($RC --libs) -Wl,-rpath,$ROOTSYS_DIR/lib"

  log "building root_sink"
  # shellcheck disable=SC2086
  if (cd "$REPO/tools/root_sink" &&
      g++ $COMMON root_sink.cxx $LINK -lRHTTP -lzmq -o root_sink) 2>/tmp/root_sink_build.log; then
    install -m 755 "$REPO/tools/root_sink/root_sink" "$HOME/.local/bin/root_sink"
    ok "root_sink -> ~/.local/bin"
  else
    tail -10 /tmp/root_sink_build.log; warn "root_sink build failed (see /tmp/root_sink_build.log)"
  fi

  log "building delila2root"
  # shellcheck disable=SC2086
  if (cd "$REPO/tools/delila2root" &&
      g++ $COMMON delila2root.C $LINK -o delila2root) 2>/tmp/delila2root_build.log; then
    install -m 755 "$REPO/tools/delila2root/delila2root" "$HOME/.local/bin/delila2root"
    ok "delila2root -> ~/.local/bin"
  else
    tail -10 /tmp/delila2root_build.log; warn "delila2root build failed (see /tmp/delila2root_build.log)"
  fi
fi

# ----------------------------------------------------------------- summary
echo
log "=== summary for $(hostname) ==="
printf '  %-16s %s\n' "rust"        "$(rustc --version 2>/dev/null || echo MISSING)"
printf '  %-16s %s\n' "delila-rs"   "$(git -C "$REPO" log --oneline -1 2>/dev/null)"
printf '  %-16s %s\n' "binaries"    "$(ls "$REPO"/target/release/operator 2>/dev/null || echo MISSING)"
printf '  %-16s %s\n' "ROOT"        "$("$ROOTSYS_DIR/bin/root-config" --version 2>/dev/null || echo MISSING)"
printf '  %-16s %s\n' "root_sink"   "$([ -x "$HOME/.local/bin/root_sink" ] && echo ok || echo MISSING)"
printf '  %-16s %s\n' "delila2root" "$([ -x "$HOME/.local/bin/delila2root" ] && echo ok || echo MISSING)"
printf '  %-16s %s\n' "caen-toolbox" "$([ -x "$HOME/.local/bin/caen-toolbox" ] && echo ok || echo MISSING)"
printf '  %-16s %s\n' "FELib"       "$([ -e "$CAEN_PREFIX/lib/libCAEN_FELib.so" ] && echo "$CAEN_PREFIX" || echo MISSING)"
echo
ok "node ready. Open a new shell (or 'source ~/.bashrc') to pick up the environment."
