# Subplan 03 — version-aware harness adapters

Parent: [master plan](master-plan.md)  
Task prefix: HAD  
Estimate: 50–70 reviewable change sets

## Outcome

Every product surface documented under harness-configs has a registered support record and a
fixture-backed adapter. Adapters resolve paths, versions, config precedence, writable fields,
isolation, discovery, and extension mechanisms without guessing.

One adapter is normally one independently reviewable implementation plus fixtures. Closely
related version variants may share pure mapping code, but keep distinct compatibility fixtures.

## Adapter contract

### HAD-01 — identity and metadata

Each adapter declares:

- HarnessId, display name, product family, executable names, desktop bundle IDs where relevant.
- Research document link and last verified date.
- Product status: active, preview, EAP, retired, archived, sunset, acquired, or unknown.
- Supported OS/architectures.
- Adapter revision.
- Known harness version ranges and schema variants.
- Default support state and reason.

No adapter is selected by fuzzy display-name matching.

### HAD-02 — detection and version resolution

Read-only probes:

- PATH lookup and configured absolute binary.
- Standard install locations and package-manager metadata.
- Version command with timeout and accepted output parser.
- Desktop application/bundle/extension detection.
- Default and candidate config roots.
- Schema/version markers inside config or path.

Detection returns evidence and confidence. It never creates first-run config as a side effect.
If a version command launches onboarding or mutates state, use package metadata or mark version
unknown.

### HAD-03 — config surfaces and precedence

Each surface declares:

- Path resolver per OS, instance, project, and harness version.
- Kind: JSON, JSONC, TOML, YAML, env, text fragment, executable, SQLite, keychain, or opaque.
- Scope: system/managed, user, instance, project/workspace, session/inline, internal.
- Ownership: user-editable, harness-managed, external secret store, superai-created.
- Precedence and merge behavior.
- Root shape and semantic validator.
- Owned selectors and edit rules.
- Backup requirement and restart/reload behavior.

Auto-managed DBs, token caches, keychains, OAuth stores, and opaque internal state are detectable
but not writable.

### HAD-04 — supported operations

Adapter provides plans, not direct writes:

- Inspect effective provider/model/key sink.
- Set/remove provider endpoint, auth reference/key, model list/defaults.
- Inspect/manage skill, plugin, and MCP locations.
- Resolve default install as an instance target.
- Plan mirror exclusions and relocation.
- Plan wrapper invocation/environment/args.
- Scan candidate unmanaged roots and ownership markers.
- Validate an instance after mutation.
- Describe external auth/restart/manual steps.

Every absent operation returns a typed support state and reason.

### HAD-05 — version selection and migrations

Resolution:

1. Detect installed harness version.
2. Resolve exact adapter schema range.
3. Detect config-era markers if product version alone is insufficient.
4. Parse using that schema.
5. Refuse writes on conflicting or unknown era.

Harness migration support is explicit:

- Read legacy and current forms.
- Preview harness-owned migration where documented.
- Preserve deprecated fields unless migration owns them.
- Never auto-migrate just because a file was inspected.

### HAD-06 — conformance suite

Every writable adapter passes:

- Missing/default/populated/malformed config fixtures.
- Unknown-key and lexical-preservation fixtures.
- Version boundary fixtures.
- Provider/model mutation plus removal.
- Skill/plugin/MCP coexistence where supported.
- Default and isolated path resolution on supported OSes.
- Wrapper plan and command quoting.
- External-edit conflict and backup/restore.
- Secret redaction.
- Unsupported-operation behavior.

Every read-only/research-blocked adapter passes detection, explanation, and no-write tests.

## Provisional support ledger

State below is implementation entry gate, not a claim that code already supports it. Research
gaps from source docs keep a surface read-only or research-blocked until closed.

| Product surface | Source | Main writable formats/surfaces | Isolation class | Entry gate |
|---|---|---|---|---|
| Aider | [aider.md](../harness-configs/aider.md) | YAML, env, JSON metadata, explicit CLI paths | explicit-config / HOME | Full candidate |
| Amazon Q Developer CLI | [amazon-q-cli.md](../harness-configs/amazon-q-cli.md) | JSON settings/agents/MCP; shared SSO cache | project/account constrained | MigrationOnly |
| Amp | [amp.md](../harness-configs/amp.md) | JSON/JSONC settings; explicit settings file | explicit-config, account constrained | Constrained |
| Antigravity CLI | [antigravity-cli.md](../harness-configs/antigravity-cli.md) | settings plus harness-owned auth; incomplete paths | HOME workaround | ResearchBlocked |
| Auggie | [auggie.md](../harness-configs/auggie.md) | JSON settings, .augment rules/commands | account/workspace constrained | Constrained |
| Claude Code | [claude-code.md](../harness-configs/claude-code.md) | JSON/JSONC settings, env, MCP, skills/plugins | relocated-root | Full candidate |
| Cline | [cline.md](../harness-configs/cline.md) | JSON settings/MCP/rules plus VS Code storage | IDE user-data | Full candidate after OS verification |
| Codex CLI | [codex-cli.md](../harness-configs/codex-cli.md) | TOML, rules, skills, MCP, profile files | relocated-root/profile | Full candidate |
| Continue | [continue-dev.md](../harness-configs/continue-dev.md) | YAML, env, rules/prompts/MCP; legacy JSON | project/explicit CLI | Constrained; hosted features excluded |
| GitHub Copilot CLI | [copilot-cli.md](../harness-configs/copilot-cli.md) | JSONC, MCP/LSP JSON, skills/plugins | relocated-root | Full candidate |
| Copilot Coding Agent | [copilot-cli.md](../harness-configs/copilot-cli.md) | repository/org cloud settings | cloud-owned | Unsupported for mutation; document only |
| Crush | [crush.md](../harness-configs/crush.md) | executable crushrc; deprecated JSON | project/XDG | ResearchBlocked for writes |
| Cursor IDE and Agent CLI | [cursor.md](../harness-configs/cursor.md) | IDE JSON/storage, CLI config, MCP/rules | CLI root plus IDE user-data | Constrained |
| DeepSeek Harness | [deepseek-harness.md](../harness-configs/deepseek-harness.md) | provider catalog; plugin config incomplete | relocated-root | ResearchBlocked, developer preview |
| Factory Droid | [factory-droid.md](../harness-configs/factory-droid.md) | layered JSON, MCP/skills/agents | project/HOME constrained | Constrained |
| Forge | [forge.md](../harness-configs/forge.md) | config directory, provider profiles | relocated config | Full candidate |
| Gemini CLI | [gemini-cli.md](../harness-configs/gemini-cli.md) | JSON settings, env, extensions | relocated-root | MigrationOnly; retired consumer tiers |
| Goose | [goose.md](../harness-configs/goose.md) | YAML config/recipes/extensions | relocated-root | Full candidate after unverified keys closed |
| gptme | [gptme.md](../harness-configs/gptme.md) | TOML/YAML/env/workspaces/log roots | workspace plus explicit state | Constrained |
| Grok Build | [grok-build.md](../harness-configs/grok-build.md) | TOML, JSON overlay, skills/plugins/MCP | relocated-root | Full candidate |
| Hermes Agent | [hermes-agent.md](../harness-configs/hermes-agent.md) | YAML/env, profiles, skills/MCP | relocated-root/profile | Full candidate |
| iFlow CLI | [iflow-cli.md](../harness-configs/iflow-cli.md) | JSON/env/MCP/agents | env/explicit system file | MigrationOnly; shutdown |
| Junie CLI | [junie-cli.md](../harness-configs/junie-cli.md) | JSON, models/MCP, skills/agents | relocated-root | Full candidate; EAP gated |
| Kilo Code extension and CLI | [kilo-code.md](../harness-configs/kilo-code.md) | layered JSONC, agents/MCP | inline/HOME plus IDE user-data | Constrained until full root verified |
| Kimi Code CLI | [kimi-cli.md](../harness-configs/kimi-cli.md) | TOML plus MCP JSON | relocated-root | Full candidate |
| Legacy Kimi CLI | [kimi-cli.md](../harness-configs/kimi-cli.md) | TOML/MCP legacy root | legacy root | MigrationOnly |
| Kiro CLI/IDE | [kiro.md](../harness-configs/kiro.md) | settings/rules/MCP under KIRO_HOME | relocated-root | ReadOnly until research gaps close |
| Kode CLI | [kode.md](../harness-configs/kode.md) | JSON, MCP, agents/skills | relocated-root | Full candidate |
| Letta Code | [letta-code.md](../harness-configs/letta-code.md) | client config plus server/provider state | separate server/state | Constrained |
| MiMo Code | [mimo-code.md](../harness-configs/mimo-code.md) | JSON/JSONC, inline config, plugins/MCP | relocated-root | Full candidate |
| Mistral Vibe | [mistral-vibe.md](../harness-configs/mistral-vibe.md) | TOML, skills/tools/MCP | relocated-root | Full candidate |
| Nanocoder | [nanocoder.md](../harness-configs/nanocoder.md) | JSON provider/MCP/preferences | relocated/explicit files | Full candidate |
| OpenClaw | [openclaw.md](../harness-configs/openclaw.md) | config/provider JSON plus daemon state | relocated-root/daemon | ResearchBlocked until gateway/schema complete |
| OpenCode | [opencode.md](../harness-configs/opencode.md) | layered JSONC, agents/plugins/MCP | relocated/inline config | Full candidate |
| OpenHands | [openhands.md](../harness-configs/openhands.md) | V1 JSON/env; V0 TOML; Docker persistence | persistence root/container | Constrained, version split required |
| Pi | [pi.md](../harness-configs/pi.md) | JSON settings/auth/models, extensions | relocated-root | Full candidate; MCP absent by design |
| Plandex | [plandex.md](../harness-configs/plandex.md) | env and server/model-pack config | provider/server scoped | Constrained |
| Qwen Code | [qwen-code.md](../harness-configs/qwen-code.md) | layered JSON/env/MCP | relocated settings/runtime | Full candidate |
| Roo Code | [roo-code.md](../harness-configs/roo-code.md) | VS Code storage, YAML modes, JSON MCP | IDE user-data | MigrationOnly; archived |
| SWE-agent | [swe-agent.md](../harness-configs/swe-agent.md) | composed YAML | explicit config/batch | Full candidate for config-run instances |
| Trae Agent | [trae-agent.md](../harness-configs/trae-agent.md) | YAML; deprecated JSON | explicit config/env | Full candidate |
| Warp Agent CLI/app | [warp.md](../harness-configs/warp.md) | CLI TOML, MCP JSON, workflows YAML | Linux XDG/profile constrained | Constrained |
| Windsurf/Devin Desktop | [windsurf.md](../harness-configs/windsurf.md) | MCP JSON, rules/skills; IDE storage | IDE user-data | Constrained |
| ZCode | [zcode.md](../harness-configs/zcode.md) | versioned-path JSON | fixed path | SingleInstance; schema research gate |
| Zed AI/ACP | [zed-acp.md](../harness-configs/zed-acp.md) | JSON settings, ACP wrappers, MCP | wrapper registrations | Constrained; version gates required |
| Vibe Kanban | [orchestrators.md](../harness-configs/orchestrators.md) | app profiles/env/MCP/worktrees | orchestrator profiles | MigrationOnly/community-maintained |
| Conductor | [orchestrators.md](../harness-configs/orchestrators.md) | user/repo TOML, env, scripts | macOS worktrees/profiles | Constrained |
| Sculptor | [orchestrators.md](../harness-configs/orchestrators.md) | env, harness settings, containers | workspace/container | Constrained |

## Adapter delivery waves

### HAD-07 — representative spine

Implement in this order:

1. Claude Code.
2. Codex CLI.
3. Aider.
4. OpenCode.
5. Cline.
6. ZCode read/single-instance.
7. OpenClaw read/daemon model.
8. Crush read plus safe write research outcome.

This spans JSON, TOML, YAML/env, JSONC, IDE storage, fixed paths, daemons, and executable config.

### HAD-08 — relocated-root CLI wave

Copilot CLI, Goose, Qwen, Kimi Code, Grok Build, Mistral Vibe, Forge, Kode, Pi, Nanocoder,
Hermes, MiMo, Junie, then DeepSeek/Kiro/OpenClaw after gates close.

### HAD-09 — explicit/project/env wave

Amp, Continue, Factory Droid, gptme, Plandex, SWE-agent, Trae, OpenHands, Letta Code.

### HAD-10 — IDE/orchestrator/constrained wave

Antigravity, Cursor, Kilo, Warp, Windsurf, Zed, Auggie, Conductor, Sculptor.

### HAD-11 — migration-only wave

Gemini CLI, Amazon Q, iFlow, Roo Code, legacy Kimi, Vibe Kanban.

Migration-only support includes:

- Detect and inspect.
- Back up/export relevant user-editable config.
- Point to successor and show source-to-target mapping where documented.
- Never create new instances by default.
- Never delete predecessor data during successor setup.

## Research closure protocol

For each source-marked gap:

1. Recheck official docs/repository/schema for installed version.
2. Capture sanitized real fixtures on every relevant OS.
3. Record verified path, precedence, mutation surface, and isolation behavior.
4. Test concurrent launches where wrapper isolation is uncertain.
5. Update harness research doc.
6. Move support state only after conformance tests exist.

Community/inferred claims may guide a probe but cannot authorize a write.

Priority gaps:

- Antigravity config/auth/MCP paths and HOME behavior.
- DeepSeek full schema, plugins, profiles, skills, lifecycle, sandbox.
- Crush command-backed mutation or safe managed include.
- Kiro full schema and BYO limitations.
- OpenClaw schema, gateway, ports, plugins/skills.
- ZCode full schema/MCP/skills and version path history.
- Zed current gateway/ACP/MCP shapes.
- Goose exact unverified keys.
- Warp XDG behavior outside documented Linux path.
- IDE credential/global-storage relocation on each OS.

## Per-adapter change-set template

1. Metadata/detection/version parser.
2. Sanitized fixtures: minimal, realistic, foreign keys, malformed, old/new.
3. Path/surface/precedence resolver.
4. Read/effective-state mapping.
5. Owned mutation mappings.
6. Isolation/wrapper/discovery plan.
7. Skill/plugin/MCP mappings.
8. Conformance and failure tests.
9. Support ledger/research-doc update.

Small adapters may combine these; never combine unrelated harnesses merely to reduce change count.

## Exit gate

- [ ] Every ledger row is registered in code/data.
- [ ] Every writable row passes conformance suite.
- [ ] Every limited row returns typed state and evidence.
- [ ] Unknown versions cannot write.
- [ ] Auto-managed/keychain/opaque files cannot enter mutation plans.
- [ ] All path/version/precedence claims have fixtures or official evidence.
- [ ] No harness is dropped because it lacks full isolation or custom endpoints.

