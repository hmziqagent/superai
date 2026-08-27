#!/bin/sh
# Provenance: source=docs/harness-configs/amp.md last_verified=2026-08-25 sanitized fake sk-fake-
# superai wrapper instance=amp-work id=test-amp-1 harness=amp generator=0.1.0 digest=abcd1234abcd1234
set -eu
export AMP_SETTINGS_FILE='/tmp/superai-test-amp-isolated-123/settings.json'
exec 'amp' --settings-file '/tmp/superai-test-amp-isolated-123/settings.json' "$@"
