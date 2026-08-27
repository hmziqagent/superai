#!/bin/sh
# Provenance: source=docs/harness-configs/grok-build.md last_verified=2026-08-25 sanitized fake sk-fake-
# superai wrapper instance=grok-work id=test-grok-1 harness=grok-build generator=0.1.0 digest=abcd1234abcd1234
set -eu
export GROK_HOME='/tmp/superai-test-grok-isolated-123'
exec 'grok' "$@"
