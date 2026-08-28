#!/bin/sh
# Provenance: source=docs/harness-configs/deepseek-harness.md last_verified=2026-08-25 sanitized fake
# superai wrapper instance=dsh-work id=test-deepseek-1 harness=deepseek-harness generator=0.1.0 digest=abcd1234
set -eu
export DSH_HOME='/tmp/superai-test-deepseek-isolated-123'
exec 'dsh' "$@"
