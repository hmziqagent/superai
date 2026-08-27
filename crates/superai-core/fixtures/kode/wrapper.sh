#!/bin/sh
# Provenance: source=docs/harness-configs/kode.md last_verified=2026-08-25 sanitized fake sk-fake-
# superai wrapper instance=kode-work id=test-kode-1 harness=kode generator=0.1.0 digest=abcd1234abcd1234
set -eu
export KODE_CONFIG_DIR='/tmp/superai-test-kode-isolated-123'
export CLAUDE_CONFIG_DIR='/tmp/superai-test-kode-isolated-123'
exec 'kode' "$@"
