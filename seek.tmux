#!/usr/bin/env bash

CURRENT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BINARY="$CURRENT_DIR/bin/seek"

# Default keybinding, user-overridable via tmux option
default_key="/"
key=$(tmux show-option -gv "@seek-key" 2>/dev/null || echo "$default_key")

tmux bind-key "$key" display-popup -E -w 100% -h 10 -y S -B "$BINARY"
