#!/bin/sh
# Provenance: source=docs/harness-configs/swe-agent.md last_verified=2026-08-25 sanitized fake sk-fake-
# superai wrapper instance=swe-work id=test-swe-1 harness=swe-agent generator=0.1.0 digest=abcd1234abcd1234
set -eu
export SWE_AGENT_TRAJECTORY_DIR='/tmp/superai-test-swe-isolated-123/trajectories'
exec 'sweagent' --config '/tmp/superai-test-swe-isolated-123/config.yaml' "$@"
