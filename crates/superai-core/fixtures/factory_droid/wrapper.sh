#!/bin/sh
# Provenance: source=docs/harness-configs/factory-droid.md last_verified=2026-08-25 sanitized fake sk-fake-
# superai wrapper instance=factory-work id=test-factory-1 harness=factory-droid generator=0.1.0 digest=abcd1234abcd1234
set -eu
export HOME='/tmp/superai-test-factory-isolated-123'
exec 'droid' "$@"
