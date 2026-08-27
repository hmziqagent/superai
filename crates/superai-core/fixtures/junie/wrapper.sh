#!/bin/sh
# Provenance: source=docs/harness-configs/junie-cli.md last_verified=2026-08-25 sanitized fake sk-fake-
# superai wrapper instance=junie-work id=test-junie-1 harness=junie-cli generator=0.1.0 digest=abcd1234abcd1234
set -eu
export JUNIE_HOME='/tmp/superai-test-junie-isolated-123'
exec 'junie' "$@"
