# Subplan 07 — providers, models, API-key placement, and health

Parent: [master plan](master-plan.md)  
Task prefix: PRV  
Estimate: 7–10 reviewable change sets

## Outcome

Providers are versioned data, not Rust branches. Core can validate a provider, render it through a
harness template/adapter, place API keys only in supported sinks, and run safe redacted health
checks. superai never proxies model traffic.

## Provider data model

### PRV-01 — provider definition schema

Versioned data fields:

- ProviderId, display name, lifecycle status.
- Supported protocols: OpenAI chat/completions, OpenAI responses, Anthropic messages, Gemini,
  or explicit vendor protocol identifier.
- Base endpoint variants by region/plan.
- Required/optional headers and request parameters.
- Auth inputs: API-key field name, header style, env variable names, placeholder policy.
- Model catalog and defaults.
- Capability contribution references.
- Health probe definitions.
- Documentation URLs and verification date.

No executable code, shell fragments, or secret values in provider definitions.

Adding a standard provider requires:

1. Add provider data.
2. Add/extend harness-provider template data.
3. Add fixtures/schema tests.
4. No Rust source edit.

### PRV-02 — model catalog

Model entry:

- stable provider-local ID and display name.
- status: active, preview, deprecated, retired.
- context/input/output limits.
- input/output modalities.
- tool/reasoning capability flags.
- optional harness aliases.
- health-test eligibility.

Avoid storing pricing unless a goal-approved consumer exists. Unknown vendor fields survive in
template data but are not invented.

Validation:

- unique IDs/aliases.
- default exists and is active unless explicitly legacy.
- positive limits and consistent modality/capability combinations.
- no duplicate normalized endpoint.

### PRV-03 — provider-to-harness rendering

Generic provider data does not know file paths. Harness template maps canonical fields to adapter
selectors:

- protocol/provider type.
- base URL.
- API-key literal/reference/env sink.
- model list/default/role slots.
- headers/body/compat options.
- required wrapper environment.

Render produces typed adapter mutations. If harness lacks custom endpoints or protocol, return
Unsupported with reason. Do not route through a proxy automatically.

### PRV-04 — API-key flow

Input lifecycle:

1. Caller provides key or external reference for one operation.
2. Validate non-empty/expected prefix only when provider documents it.
3. Choose adapter-declared sink.
4. Redact preview: show destination and auth style, never value.
5. Write through safe mutation with restrictive permissions.
6. Drop input after operation.

Allowed sinks:

- Harness user/instance config field.
- Harness-supported env file under isolated root.
- Wrapper reference to externally set env var.
- Harness-supported command helper/file reference.

Not allowed:

- Instance/registry/provider/template records.
- superai credential DB.
- generic keychain.
- generated wrapper literal unless the harness itself defines wrapper/env file as credential
  storage and caller explicitly selects it.
- logs/journal/errors.

OAuth/subscription/keychain login returns ExternalAuthRequired with harness command/instructions.

### PRV-05 — effective provider inspection

Read config fresh and return:

- detected provider/protocol/endpoint/model roles.
- credential presence and source type only, never secret.
- config layer that won.
- unsupported/unknown fields relevant to mutation.
- adapter/template compatibility.

Effective state is ephemeral. Do not persist it in registry.

### PRV-06 — health probe schema

Probe kinds:

- HTTP status endpoint.
- Model-list endpoint.
- Minimal authenticated request only where provider explicitly documents safe request.
- TCP connect for local services.
- Harness diagnostic command where it does not mutate/login.

Probe fields:

- URL derivation, method, headers/body template.
- auth reference.
- timeout and response size cap.
- accepted status/body predicate.
- TLS/private-network policy.
- rate/cost warning.

No universal GET /models assumption. No retry storm. No model completion by default.

### PRV-07 — health execution

Security:

- Explicit user/workflow invocation; no background polling in core.
- Validate URL scheme; block file and unsupported schemes.
- Private/loopback endpoints allowed only when definition/user intends local provider.
- Redirect limit and cross-host auth-header stripping.
- Response byte/time limits.
- Redact auth and response fields.
- Distinguish DNS/TLS/auth/rate-limit/server/schema/model-not-found failures.

Result is current observation with timestamp, not persisted source of truth.

### PRV-08 — provider lifecycle

Operations:

- Add provider to instance.
- Update endpoint/models/key.
- Switch default model/roles.
- Remove provider-owned entries only.
- Validate remaining default references before removal.

Shared config:

- Preserve providers/models not owned by operation.
- Refuse removing a provider referenced by other harness roles/agents unless request includes
  explicit reassignment.

## Tests

- Same provider data renders differently for Claude, Codex, Kimi, OpenCode, and ZCode fixtures.
- Adding a synthetic provider uses only data/templates.
- Unsupported managed-backend harness returns reason, no mutation.
- Secret sentinel absent from preview/diff/error/journal/registry.
- Key change backup contains old file but backup catalog does not expose key.
- Redirect does not leak Authorization across host.
- Local probe works only with allowed private-network definition.
- Provider removal preserves foreign entries and catches dangling defaults.

## Exit gate

- [ ] Provider/model schema validated and versioned.
- [ ] Standard provider addition is data-only.
- [ ] API keys never enter superai records/logs.
- [ ] Effective provider inspection reads fresh.
- [ ] Health is bounded, redacted, protocol-aware, and opt-in.
- [ ] No proxy or wire translation exists.

