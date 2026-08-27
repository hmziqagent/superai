#!/bin/sh
# Provenance: source=docs/harness-configs/copilot-cli.md last_verified=2026-08-25 sanitized fake sk-fake-
# superai wrapper instance=copilot-work id=test-copilot-1 harness=copilot-cli generator=0.1.0 digest=abcd1234abcd1234
set -eu
export COPILOT_HOME='/tmp/superai-test-copilot-isolated-123'
exec 'copilot' "$@"
