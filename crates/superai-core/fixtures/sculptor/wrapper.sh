#!/bin/sh
# Provenance: source=docs/harness-configs/orchestrators.md last_verified=2026-08-25 sanitized fake sk-fake-
# superai wrapper instance=sculptor-work id=test-sculptor-1 harness=sculptor generator=0.1.0 digest=abcd1234abcd1234
set -eu
export SCULPTOR_WORKSPACE_PATH='/tmp/superai-test-sculptor-isolated-123/code'
export SCULPTOR_ENV_FILE='/tmp/superai-test-sculptor-isolated-123/.env'
export SCULPTOR_GLOBAL_ENV='/tmp/superai-test-sculptor-isolated-123'
exec 'sculptor' "$@"
