#!/bin/sh
# Provenance: source=docs/harness-configs/gptme.md last_verified=2026-08-25 sanitized fake sk-fake-
# superai wrapper instance=gptme-work id=test-gptme-1 harness=gptme generator=0.1.0 digest=abcd1234abcd1234
set -eu
export GPTME_WORKSPACE='/tmp/superai-test-gptme-isolated-123'
exec 'gptme' --workspace '/tmp/superai-test-gptme-isolated-123' "$@"
