#!/bin/sh
# Provenance: source=docs/harness-configs/mimo-code.md last_verified=2026-08-25 sanitized fake sk-fake-
# superai wrapper instance=mimo-work id=test-mimo-1 harness=mimo-code generator=0.1.0 digest=abcd1234abcd1234
set -eu
export MIMOCODE_HOME='/tmp/superai-test-mimo-isolated-123'
exec 'mimo' "$@"
