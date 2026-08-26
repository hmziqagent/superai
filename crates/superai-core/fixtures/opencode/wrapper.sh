#!/bin/sh
# Provenance: source=docs/harness-configs/opencode.md last_verified=2026-08-25 sanitized fake sk-fake-
# superai wrapper instance=opencode-work id=test-opencode-1 harness=opencode generator=0.1.0 digest=abcd1234abcd1234
# generated: do not edit manually; edits will be detected as drift
set -eu
export XDG_CONFIG_HOME='/tmp/superai-test-opencode-isolated-123'
export OPENCODE_CONFIG='/tmp/superai-test-opencode-isolated-123/opencode.json'
exec 'opencode' "$@"
