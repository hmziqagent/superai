#!/bin/sh
# Provenance: source=docs/harness-configs/aider.md last_verified=2026-08-25 sanitized fake sk-fake-
# superai wrapper instance=aider-work id=test-aider-1 harness=aider generator=0.1.0 digest=abcd1234abcd1234
# generated: do not edit manually; edits will be detected as drift
set -eu
export HOME='/tmp/superai-test-aider-isolated-123'
exec 'aider' '--config' '/tmp/superai-test-aider-isolated-123/.aider.conf.yml' '--env-file' '/tmp/superai-test-aider-isolated-123/.env' "$@"
