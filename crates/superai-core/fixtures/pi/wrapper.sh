#!/bin/sh
# Provenance: source=docs/harness-configs/pi.md last_verified=2026-08-25 sanitized fake sk-fake-
# superai wrapper instance=pi-work id=test-pi-1 harness=pi generator=0.1.0 digest=abcd1234abcd1234
set -eu
export PI_CODING_AGENT_DIR='/tmp/superai-test-pi-isolated-123'
exec 'pi' "$@"
