# Subplan 16 — VPS deploy and geo-exit

Parent: [master plan](master-plan.md)
Task prefix: VPS (deploy)
Estimate: 6–9 reviewable change sets (superai's share of the 10–14 combined estimate
this flow shared with the toride plan's gains before the split)

## Outcome

`superai vps deploy llmproxy` puts the proxy on a VPS from the workstation, TLS-
terminated or tunnel-only, with every remote mutation previewed, backed up, and
rollback-documented; `superai vps status` reads its health back. superai still does
not proxy anything itself (goal.md "Routing"): it deploys and configures llmproxy,
and reads its ledger through subplan 15's client.

The remote-exec layer comes from the toride companion plan
([export/toride-vps-services.md](export/toride-vps-services.md)): an exec service and
an upload service in toride-ssh with argv-disciplined `ssh`/sftp invocations, unit
install plus `daemon-reload` composed over toride-service's manager, an abuse family in
toride's own suite, and 0.2 publishes of toride-ssh and toride-service to crates.io.

Dependencies:

- llmproxy companion plan PRX-05: musl release artifacts and shell installer; without
  them, deploy means building on the VPS, which this plan does not want as the only
  path.
- toride 0.2 publishes (toride companion plan, VPS-01 item 3).
- Subplan 15's USG-08 client for the `/usage/summary` probes in `vps status`.
- Subplan 14's CLI surface for the subcommand pattern.

## VPS-02 — `superai vps deploy llmproxy` (repo: superai)

Size: M/L. A new superai-core module bridging toride the way `toride_bridge.rs` does
for installs, plus a CLI surface `superai vps <deploy|status|upgrade> llmproxy
--host <user@host>`:

1. Preflight: ssh reachability, host-key policy check, OS/systemd presence (via the
   exec service), disk space.
2. Install: upload the musl binary and root-owned config skeleton (provider TOMLs
   locally rendered, secrets interpolated on the VPS from an env file or prompted
   upload, never printed), install the systemd unit (dynamic-user, hardened
   `NoNewPrivileges`/`ProtectSystem=strict` shape), enable and start.
3. TLS ingress: toride-proxy nginx site (or caddy when chosen) terminating TLS for the
   proxy's public listener, certbot/Let's Encrypt via toride-proxy's certs module,
   config diff previewed before commit, matching superai's preview-then-commit posture.
4. Tunnels: private mode mounts nothing public; the VPS's llmproxy listens on the
   wireguard interface or tailnet only. toride-wireguard writes the interface config
   (server peer plus a client peer for the workstation); toride-tailscale is the
   alternative when the user runs a tailnet. Harness configs then point at the tunnel
   address, which superai already writes (goal.md "Routing").
5. Geo-exit (optional flag): a forward proxy on the exit VPS reachable only over the
   tunnel, plus per-provider `egress.proxy` entries in the llmproxy provider TOMLs
   pointing at it (llmproxy companion plan, PRX-04). The exit VPS's location
   determines upstream geography; llmproxy itself never selects source addresses.

Every remote mutation is previewed with a diff and committed stepwise with rollback
notes, mirroring the install-plan discipline (`install_plan.rs`, `install_execute.rs`).
The remote root is the user's machine, not superai's: files superai did not create are
backed up before overwrite, the same rule as harness configs.

## VPS-03 — `superai vps status`

Size: S. Read-only health: service state via toride-service, `/health` and
`/usage/summary` probes through the tunnel, cert expiry from toride-proxy's cert
parsing, disk and load via the exec service. Feeds `superai usage` with the VPS ledger
as a source.

## Tests

- VPS flow: preview diffs against fixture remote trees; unit install backs up first;
  secrets never appear in previews, journals, or logs (abuse-suite extension).

## Exit gate

- [ ] `superai vps deploy llmproxy` reaches a healthy TLS-terminated or tunnel-only
      deployment with previewed, backed-up, rollback-documented steps.
- [ ] `superai vps status` reports service state, `/health` and `/usage/summary`
      through the tunnel, cert expiry, disk and load, all read-only.
- [ ] No secret ever appears in previews, journals, or logs; superai's abuse suite is
      extended for the VPS flow (the toride-side family lives in the toride companion
      plan, the llmproxy-side family in the llmproxy companion plan).

## Out of scope

- Changes inside the toride or llmproxy repos: their companion plans own those; this
  subplan consumes published artifacts only.
- Bind-out/source-address selection in llmproxy: exit geography comes from where the
  tunnel endpoint runs (llmproxy companion plan, PRX-04 layer 2).
- Any GUI for the deploy flow.

## Unverified and excluded

The VPS flow is fixture-proven only: preflight, previewed deploy and upgrade
with backup-first commit and rollback journaling, on-VPS interpolation of
provider secrets and the wireguard key, and read-only status with cert expiry
all run against the `FixtureRemote` backend in crates/superai-core/src/vps/;
the `/usage/summary` ledger probe in `superai vps status` is proven separately,
against a local mock HTTP server in the CLI suite. Still blocked:

- The `/health` probe that VPS-03 and the exit gate name: the frozen llmproxy
  contract (docs/plans/export/llmproxy-usage-ledger.md) defines no `/health`
  endpoint, so status probes the ledger only. Needs the endpoint from the
  llmproxy companion plan, then the probe here.
- The geo-exit forward proxy on the exit VPS (VPS-02 step 5): only the
  per-provider `[egress]` table rendering exists; no step deploys the proxy
  itself. Owed here.
- Live verification on a real host and on PRX-05 musl artifacts, and the
  toride 0.2 publishes that `TorideRemote` refuses every verb until: the
  companion plans own both.

Beyond these, nothing new beyond subplan 14's list and the companion plans'
own sections.

## Open questions

- VPS baseline: assumed a systemd Debian/Ubuntu image, matching toride's systemd and
  nginx focus. A different baseline changes VPS-02's unit and ingress steps only.
