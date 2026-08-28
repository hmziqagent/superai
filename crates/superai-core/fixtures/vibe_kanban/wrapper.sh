#!/bin/sh
# Provenance: source=docs/harness-configs/orchestrators.md last_verified=2026-08-25 sanitized fake
# MigrationOnly: Vibe Kanban sunsetting → community maintained, no wrapper; export/backup only
# superai wrapper would be worktree-based but blocked MigrationOnly
set -eu
echo "MigrationOnly: use conductor/sculptor or direct harness, export worktrees at .vibe-kanban-workspaces/" >&2
exit 69
