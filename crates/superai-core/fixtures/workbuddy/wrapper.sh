#!/bin/sh
# Provenance: source=docs/harness-configs/workbuddy.md last_verified=2026-09-08 sanitized fake sk-fake-
# superai wrapper instance=workbuddy-test id=test-workbuddy-1 harness=workbuddy generator=0.1.0 digest=abcd1234abcd1234
set -eu
export CODEBUDDY_CONFIG_DIR='/tmp/superai-test-workbuddy-isolated-123'
export CODEBUDDY_API_KEY='sk-test-fake-123'
export DISABLE_AUTOUPDATER=1
exec 'cbc' "$@"
