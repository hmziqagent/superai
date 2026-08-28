#!/bin/sh
# Provenance: source=docs/harness-configs/crush.md last_verified=2026-08-25 sanitized fake
# superai wrapper instance=crush-work id=test-crush-1 harness=crush generator=0.1.0 digest=abcd1234
set -eu
export CRUSH_GLOBAL_CONFIG='/tmp/superai-test-crush-isolated-123'
exec 'crush' "$@"
