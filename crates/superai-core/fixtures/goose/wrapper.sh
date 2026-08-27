#!/bin/sh
# Provenance: source=docs/harness-configs/goose.md last_verified=2026-08-25 sanitized fake sk-fake-
# superai wrapper instance=goose-work id=test-goose-1 harness=goose generator=0.1.0 digest=abcd1234abcd1234
set -eu
export GOOSE_PATH_ROOT='/tmp/superai-test-goose-isolated-123'
exec 'goose' "$@"
