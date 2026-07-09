#!/bin/sh
# Thin wrapper: install only the ucode LuCI app via the unified installer.
#
# Usage:
#   sh install-luci.sh
#   ROUTER=root@192.168.20.254 sh install-luci.sh

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
exec sh "$ROOT/install.sh" --luci-only "$@"
