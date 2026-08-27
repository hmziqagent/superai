#!/bin/sh
# Provenance: source=docs/harness-configs/kimi-cli.md last_verified=2026-08-25 sanitized fake sk-fake-
# superai wrapper instance=kimi-work id=test-kimi-1 harness=kimi-code generator=0.1.0 digest=abcd1234abcd1234
set -eu
export KIMI_CODE_HOME='/tmp/superai-test-kimi-isolated-123'
exec 'kimi' "$@"
