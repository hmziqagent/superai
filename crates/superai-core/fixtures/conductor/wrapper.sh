#!/bin/sh
# Provenance: source=docs/harness-configs/orchestrators.md last_verified=2026-08-25 sanitized fake sk-fake-
# superai wrapper instance=conductor-work id=test-conductor-1 harness=conductor generator=0.1.0 digest=abcd1234abcd1234
set -eu
export CONDUCTOR_WORKSPACE_PATH='/tmp/superai-test-conductor-isolated-123'
export CONDUCTOR_ROOT_PATH='/tmp/superai-test-conductor-isolated-123/..'
export CONDUCTOR_PORT='4000'
export CONDUCTOR_IS_LOCAL='1'
exec 'conductor' "$@"
