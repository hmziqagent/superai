#!/bin/sh
# Provenance: source=docs/harness-configs/copilot-cli.md last_verified=2026-08-25 sanitized fake
# Unsupported: Copilot Coding Agent is cloud-owned, no local wrapper
# superai reports UnsupportedOperation for plan_wrapper/validate_instance
set -eu
echo "Unsupported: cloud-owned repo/org settings at github.com → Copilot → Coding agent; manage via .github/copilot-instructions.md and copilot-setup-steps.yml; for local use copilot-cli" >&2
exit 69
