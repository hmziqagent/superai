#!/bin/sh
# Provenance: source=docs/harness-configs/claude-code.md last_verified=2026-08-25 sanitized fake sk-fake-
# superai wrapper instance=claude-work id=test-claude-1 harness=claude-code generator=0.1.0 digest=abcd1234abcd1234
# generated: do not edit manually; edits will be detected as drift
set -eu
export CLAUDE_CONFIG_DIR='/tmp/superai-test-claude-isolated-123'
exec 'claude' "$@"
