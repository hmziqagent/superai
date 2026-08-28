#!/bin/sh
# Provenance: source=docs/harness-configs/letta-code.md last_verified=2026-08-25 sanitized fake
# superai wrapper instance=letta-work id=test-letta-1 harness=letta-code generator=0.1.0 digest=abcd1234
set -eu
export LETTA_LOCAL_BACKEND_DIR='/tmp/superai-test-letta-isolated-123'
export LETTA_BASE_URL='http://localhost:8283'
exec 'letta' --backend local "$@"
