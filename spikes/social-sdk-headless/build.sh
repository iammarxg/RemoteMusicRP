#!/usr/bin/env bash
# Build the Social SDK headless spike on Linux (e.g. WSL Ubuntu).
# Expects the SDK files (NOT committed; each user downloads them) in:
#   sdk/include/discordpp.h, sdk/include/cdiscord.h, sdk/lib/libdiscord_partner_sdk.so
set -euo pipefail
cd "$(dirname "$0")"
g++ -std=c++20 -O1 -Wall \
    -I sdk/include main.cpp \
    -L sdk/lib -ldiscord_partner_sdk -pthread \
    -Wl,-rpath,'$ORIGIN/sdk/lib' \
    -o headless-test
echo "built: $(pwd)/headless-test"
