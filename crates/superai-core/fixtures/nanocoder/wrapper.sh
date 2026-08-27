#!/bin/sh
# Provenance: source=docs/harness-configs/nanocoder.md last_verified=2026-08-25 sanitized fake sk-fake-
# superai wrapper instance=nanocoder-work id=test-nanocoder-1 harness=nanocoder generator=0.1.0 digest=abcd1234abcd1234
set -eu
export NANOCODER_CONFIG_DIR='/tmp/superai-test-nanocoder-isolated-123'
exec 'nanocoder' "$@"
