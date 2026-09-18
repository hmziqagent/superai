#!/bin/sh
# Provenance: source=docs/harness-configs/plandex.md last_verified=2026-08-25 sanitized fake sk-fake-
# superai wrapper instance=plandex-work id=test-plandex-1 harness=plandex generator=0.1.0 digest=abcd1234abcd1234
set -eu
export PLANDEX_API_HOST='http://localhost:8099'
export PLANDEX_ENV='production'
export OPENROUTER_API_KEY='sk-fake-openrouter-123456'
# plandex v2 has no relocation env; its home (~/.plandex-home-v2 with
# custom-models.json) is HOME-relative, so isolation relocates HOME
# (live cli/v2.2.1 — docs/harness-configs/plandex.md §1).
export HOME='/tmp/superai-test-plandex-isolated-123'
exec 'plandex' "$@"
