#!/bin/sh
# Provenance: source=docs/harness-configs/codex-cli.md last_verified=2026-08-25 sanitized fake sk-fake-
# superai wrapper instance=codex-work id=test-codex-1 harness=codex-cli generator=0.1.0 digest=abcd1234abcd1234
# generated: do not edit manually; edits will be detected as drift
set -eu
export CODEX_HOME='/tmp/superai-test-codex-isolated-123'
exec 'codex' "$@"
