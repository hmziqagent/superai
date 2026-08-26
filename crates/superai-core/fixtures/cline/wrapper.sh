#!/bin/sh
# Provenance: source=docs/harness-configs/cline.md last_verified=2026-08-25 sanitized fake sk-fake-
# superai wrapper instance=cline-work id=test-cline-1 harness=cline generator=0.1.0 digest=abcd1234abcd1234
# generated: do not edit manually; edits will be detected as drift
set -eu
export CLINE_DATA_DIR='/tmp/superai-test-cline-isolated-123'
exec 'code' '--user-data-dir' '/tmp/superai-test-cline-isolated-123/vscode-data' '--extensions-dir' '/tmp/superai-test-cline-isolated-123/extensions' "$@"
