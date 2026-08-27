#!/bin/sh
# Provenance: source=docs/harness-configs/qwen-code.md last_verified=2026-08-25 sanitized fake sk-fake-
# superai wrapper instance=qwen-work id=test-qwen-1 harness=qwen-code generator=0.1.0 digest=abcd1234abcd1234
set -eu
export QWEN_HOME='/tmp/superai-test-qwen-isolated-123'
export QWEN_RUNTIME_DIR='/tmp/superai-test-qwen-isolated-123/runtime'
exec 'qwen' "$@"
