#!/bin/sh
# Provenance: source=docs/harness-configs/iflow-cli.md last_verified=2026-08-25 sanitized fake
# MigrationOnly: iFlow CLI shut down 2026-04-17, successor gemini-cli
# superai wrapper would be IFLOW_* env vars but blocked MigrationOnly
set -eu
export IFLOW_CLI_SYSTEM_SETTINGS_PATH='/tmp/superai-test-iflow-isolated-123/settings.json'
exec 'iflow' "$@"
