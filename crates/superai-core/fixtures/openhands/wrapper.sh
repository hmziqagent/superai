#!/bin/sh
# Provenance: source=docs/harness-configs/openhands.md last_verified=2026-08-25 sanitized fake sk-fake-
# superai wrapper instance=openhands-work id=test-openhands-1 harness=openhands generator=0.1.0 digest=abcd1234abcd1234
set -eu
export OH_PERSISTENCE_DIR='/tmp/superai-test-openhands-isolated-123'
export LLM_MODEL='openai/gpt-4o'
export LLM_API_KEY='sk-fake-openhands-123456'
export LLM_BASE_URL='https://api.openai.com/v1'
export RUNTIME='docker'
exec 'openhands' --override-with-envs "$@"
