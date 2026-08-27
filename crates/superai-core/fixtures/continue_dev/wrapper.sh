#!/bin/sh
# Provenance: source=docs/harness-configs/continue-dev.md last_verified=2026-08-25 sanitized fake sk-fake-
# superai wrapper instance=continue-work id=test-continue-1 harness=continue-dev generator=0.1.0 digest=abcd1234abcd1234
set -eu
exec 'cn' --config '/tmp/superai-test-continue-isolated-123/config.yaml' "$@"
