#!/bin/sh
# Provenance: source=docs/harness-configs/warp.md last_verified=2026-08-25 sanitized fake sk-fake-
# superai wrapper instance=warp-work id=test-warp-1 harness=warp generator=0.1.0 digest=abcd1234abcd1234
set -eu
export XDG_CONFIG_HOME='/tmp/superai-test-warp-isolated-123/config'
export XDG_DATA_HOME='/tmp/superai-test-warp-isolated-123/data'
export WARP_API_KEY='sk-fake-warp-123456'
exec 'warp' "$@"
