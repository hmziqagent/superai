#!/bin/sh
# Provenance: source=docs/harness-configs/mistral-vibe.md last_verified=2026-08-25 sanitized fake sk-fake-
# superai wrapper instance=vibe-work id=test-vibe-1 harness=mistral-vibe generator=0.1.0 digest=abcd1234abcd1234
set -eu
export VIBE_HOME='/tmp/superai-test-vibe-isolated-123'
exec 'vibe' "$@"
