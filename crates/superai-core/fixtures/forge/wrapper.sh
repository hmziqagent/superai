#!/bin/sh
# Provenance: source=docs/harness-configs/forge.md last_verified=2026-08-25 sanitized fake sk-fake-
# superai wrapper instance=forge-work id=test-forge-1 harness=forge generator=0.1.0 digest=abcd1234abcd1234
set -eu
export FORGE_CONFIG='/tmp/superai-test-forge-isolated-123'
exec 'forge' "$@"
