# Test plan

What is proven, where, and — just as important — what is deliberately
not. Written at the end of Phase 7, from the test-gap audit and the
mandatory security review, with Kenny's decision recorded for every gap
either of them found.

The rule this document exists to enforce: **an accepted limitation is a
decision, written down. A gap nobody decided about is a hole.**

## The suites

| Suite | What it covers |
|---|---|
| unit tests in `src/core/*` | The pure logic: error classification, mapping, the journal's replay model, version comparison, the worker's pacing state machine, token hashing, sealing, HTML escaping, profile validation. No I/O, so these are fast and total. |
| unit tests in `src/shell/*` | The I/O side against local stubs: the Calendar client's retry loop, token refresh, per-source auth, the encrypted store, the self-updater, the delivery path's calendar routing. |
| `tests/kit_door.rs` | The door on the kit (4.0.0): a source posts with the client token under its own name and nothing else, the 3.x tokens are imported once and keep working, the debug views need the admin, captures take any client, health and metrics stay open. |
| `tests/kit_dashboard.rs` | The pages on the kit (4.0.0): the door and the 3.x redirects, the Sources page and its token state, K21 add/delete, K23 unusable files and reload, K24 calendars, captures rendered inert. |
| `tests/ingest_http.rs`, `tests/admin_http.rs` | The ingest and debug surfaces in-process, as the admin: journalling, idempotency, unwritable journal, dry-run, capture cap and redaction, metrics. |
| *(the kit's suite)* | Login, logout, sessions, forged cookies, cookie attributes, CSRF, CSP and the never-a-token-in-a-page rule are chassis-rs's to prove since 4.0.0. |
| `tests/self_update.rs` | Self-update end to end against a local release host with real minisign verification: install, tampered binary, tampered manifest, foreign signing key, unstartable version, downgrade, incomplete release, unreachable host. |
| `tests/process_lifecycle.rs` | The real binary as a process: SIGTERM draining cleanly, the startup retry after an unreachable Google, a broken key exiting, two processes on one data directory, `--check` against a live instance. |
| `tests/mapping_regression.rs` | Each source's real payload byte-compared against a pinned event, so a mapping change that alters output is visible in the diff. |
| `tests/no_secrets_in_logs.rs` | Every one of the secrets, plus process arguments. |
| `tests/calendar_e2e.rs`, `tests/power_loss_drill.rs` | **Live**, against a real calendar. `#[ignore]`d locally; run by the `live-tests` workflow. |

## Where each Essential feature is proven

| Feature | Proven by |
|---|---|
| K1 calendar CRUD | `calendar_e2e` (live) |
| K2 upsert, no duplicates | `calendar_e2e` (live), `core::upsert` unit tests |
| K3 multiple calendars | `shell::delivery` — two profiles, two calendars, plus the pre-upsert lookup |
| K4 token refresh | `shell::auth` — reuse, refresh, cold start, and AR18's single-flight |
| K5 mapping engine | `core::mapping` + the three pinned fixtures |
| K6 per-source tokens | `shell::ingest` (the name check), `tests/kit_door.rs` (through the kit's door) |
| K7 durable ingest | `shell::journal`, `tests/ingest_http.rs`, the power-loss drills |
| K8 synchronous API | `shell::ingest` — delivery, auth, 502-with-payload-kept, and delete including cross-source isolation |
| K9 alert sources | `mapping_regression` + `tests/ingest_http.rs` at the HTTP layer |
| K11 debug surface | `tests/admin_http.rs` |
| K12 secrets via Latch | `tests/no_secrets_in_logs.rs` |
| K13 health endpoint | `tests/kit_door.rs` |
| M2 graceful shutdown | `tests/process_lifecycle.rs` — a real SIGTERM to a serving process |
| M3 retry with backoff | `shell::calendar_client` — 503-then-success, and a 403 tried once |
| M4 startup validation | `core::profile`, including the IANA timezone check |
| M7 idempotency keys | `shell::delivery`, `tests/ingest_http.rs` |
| M8 one version | `scripts/check-version.sh` in CI and the commit hooks |
| M10 self-update | `tests/self_update.rs`, `core::update`, `shell::update` |
| M11 raw capture | `tests/kit_door.rs` — redaction and the door through the endpoints; `shell::admin` unit tests for the cap |
| M12 dashboard | `tests/kit_dashboard.rs` (Almanac's pages) and the kit's suite (login, sessions, tokens) |
| M13 metrics | `core::metrics` for the rendering, `tests/admin_http.rs` for the endpoint, the open-without-a-token rule, and the acceptance criterion asserted against a state holding a token, a calendar id and a household detail |

## Not covered, by decision

Each of these was put to Kenny as a gap with its concrete failure mode,
and each answer is his.

### T18 · Google's 403 reason strings are a spot check

*Accepted as a known limitation, 2026-08-29.*

The transient/permanent classification is exhaustively tested across
status codes — 5xx and 4xx are complete ranges. But a 403 carries no
information in its status code: Google uses the same code for "you are
going too fast" and "you may not touch this calendar", and the
difference is a reason string in the body. That list is a spot check of
five values.

An undocumented or newly-added reason — `sharingRateLimitExceeded`, for
instance — is therefore treated as permanent. **The direction is safe**:
we give up rather than hammer. The cost is an occasional event that
would have succeeded on a retry, and which now goes to the dead-letter
after three attempts instead.

Why accepted rather than closed: Google changes this list without
announcement, so a test pinned to today's list ages into a false sense
of completeness. The impact is one delayed event, not a wrong or lost
one.

### S1 · The published Home Assistant webhook id

*Accepted, 2026-08-29. Kenny's words: "het is maar een logkanaal".*

A live Home Assistant webhook URL was committed to this public
repository. It has been removed from the working tree, but it is in the
history and must be considered public.

The exposure, stated plainly so the acceptance is informed: a webhook
id is the whole of that automation's authentication, and `local_only`
bounds it to the LAN rather than to people who should have it. Anyone
who can reach the Home Assistant host can therefore post forged homelab
events. Because `op` doubles as the deduplication and acknowledgement
key, a forged event replaying a real `op` — `almanac-update-unverified`,
say — can pre-acknowledge or collapse the genuine alert. So the
accepted risk is not only "false lines in a log": it includes an
attacker being able to suppress a real notification.

Kenny weighed that and chose not to rotate, because the channel carries
no secrets and acts on nothing. Nothing in Almanac depends on the
webhook being authentic.

What follows from the decision, and is now enforced: the URL is not in
any tracked file, `.env.example` marks it a secret, and the systemd unit
takes it from Latch rather than carrying it inline.

## Proven on the deployment, not in CI

Some things only exist on real hardware. These were run against CT 112
and the results are recorded here because nothing re-runs them.

| What | When | Result |
|---|---|---|
| M10 self-update, end to end | 2026-08-29 14:17–14:22 | 0.1.2 saw the published 0.1.3, verified the signature and checksum, probed the new binary, swapped it, restarted into it, and cleared its own probation. `/metrics` went from 404 to 200 without anyone touching the machine. |
| K19 handover to the homelab | 2026-08-30 08:38 → 08:50 | The same command, before and after 1.3.1. On 1.3.0, run the way the supervisor runs it: `nothing to do`, exit 0 — a silent no-op the supervisor would have read as success. On 1.3.1: `already on 1.3.1`, exit 0, having actually checked. The periodic updater then switched off, logging that whatever supervises the process owns updates. |
| M10 self-update, unattended | 2026-08-29 16:36–16:42 | 0.1.4 → 1.0.0 with nobody touching the machine: published, seen five minutes after a restart, verified, probed, installed, restarted into, and its probation cleared 60s later. The third full run of the day, after 0.1.2→0.1.3 and 0.1.3→0.1.4. |
| M10 first-check timing | 2026-08-29 13:43:27 → 13:48:28 | The first check falls five minutes after start, not a whole interval later. This is the drill that found the bug it now guards. |
| AR21 startup retry | 2026-08-29, hard power cut | Started before the network settled, logged "could not reach Google yet; retrying", and recovered on its own. |
| AR16 replay | 2026-08-29, hard power cut | An accepted-but-undelivered event went out on the next start, without duplicating. |
| K8 delete | 2026-08-29 | Deleted by external id; a second delete answered `not_found` rather than pretending. |

## Known limitations that are not test gaps

- **No real reboot or self-update has been run on hardware.** The
  mechanism is proven end to end against a local release host, and
  `--check` is proven against the real binary with the real Latch
  secrets. What is unproven is the last step — SIGTERM, systemd, the new
  binary — on the actual LXC. That belongs to the deployment drill,
  which Kenny holds behind a go per action (D9).
- **Coverage measurement is informational.** The CI job reports it and
  does not gate on it. The number that matters is which files are never
  touched at all — which is how the gaps this phase closed went
  unnoticed — not a percentage.

## What Phase 7 changed

The audit found 24 gaps and the security review 2. Four of the 24 were
not gaps but live defects, each now fixed with a regression test that
fails against the old code:

1. A forgotten capture disabled self-update permanently, because expiry
   only ran while somebody had a capture page open.
2. A login racing a token issue could deadlock the whole service,
   ingest included, while `/healthz` kept answering 200.
3. A dropped connection to the Calendar API was classified permanent,
   so a two-second blip surfaced as a failure.
4. The runbook's first-install step named a directory that does not
   exist.

Both security findings were also mine, from the same day: the published
webhook id above, and a capture endpoint whose only credential was the
one that opens everything.

Everything else was closed as tests, except T18 and S1 above.
