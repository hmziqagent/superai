#!/bin/sh
# Provenance: source=docs/harness-configs/hermes-agent.md last_verified=2026-08-25 sanitized fake sk-fake-
# superai wrapper instance=hermes-work id=test-hermes-1 harness=hermes-agent generator=0.1.0 digest=abcd1234abcd1234
set -eu
export HERMES_HOME='/tmp/superai-test-hermes-isolated-123'
exec 'hermes' "$@"
