#!/bin/sh
# Provenance: source=docs/harness-configs/trae-agent.md last_verified=2026-08-25 sanitized fake sk-fake-
# superai wrapper instance=trae-work id=test-trae-1 harness=trae-agent generator=0.1.0 digest=abcd1234abcd1234
set -eu
export TRAE_CONFIG_FILE='/tmp/superai-test-trae-isolated-123/trae_config.yaml'
exec 'trae-cli' --config-file '/tmp/superai-test-trae-isolated-123/trae_config.yaml' "$@"
