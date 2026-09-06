# Features — Almanac

Phase 2 output. Ratings use the fixed scale **Essential · Desired ·
Later · Don't do**, confirmed by Kenny across two gate rounds on
2026-08-28 (round 1: existing/Kenny's features, IDs `K*`; round 2:
Claude's own proposals — gaps, hardening, quality-of-life, IDs `M*`).
IDs are permanent: they appear in commits, test names, docs, and forms
from here on. Changes after the freeze go through a mini-round only
(`FORM_PROTOCOL.md` §5).

## Round 1 — existing / Kenny's features

| ID | Feature | Rating | Test expectation |
|---|---|---|---|
| K1 | Calendar CRUD core — create/update (full replace)/delete on Google Calendar events, plus the typed `GoogleEvent` model | Essential | E2E against a real test calendar: create → read → modify → delete round-trip. |
| K2 | Upsert via external ID — find an event by a private extended property so a repeated source update modifies the existing event instead of duplicating it | Essential | Automated test sending the same source event twice; asserts exactly one Google event exists. |
| K3 | Multiple calendars — create/list/target several calendars (e.g. "infra", "hobbies"); each mapping profile picks its own target calendar | Essential | Test with two profiles writing to two different calendars; asserts no cross-contamination. |
| K4 | Automatic OAuth2 token refresh — fixes the current one-shot-token defect (dies after ~1h uptime) | Essential | Test with an expired/near-expired token asserting refresh happens before the next Google call; plus a test for initial-token-fetch failure. |
| K5 | Generic mapping-profile engine — declarative per-source field mapping (title/time window/color, upsert key, target calendar) replacing hardcoded Vikunja-specific Rust | Essential | Test loading a profile from a sample file and correctly translating a sample payload into a `GoogleEvent`, independent of source. **Superseded 2026-09-03 by K23:** the field mapping is gone; a source sends Almanac's own event shape and the profile is routing only. What K5 replaced — a hardcoded per-source translation in Rust — stays replaced; what took its place is now the payload contract rather than a per-source table. |
| K6 | Per-source bearer tokens on every inbound endpoint (Latch-issued, independently revocable) | Essential | Test asserting requests without/with a wrong token fail (401/403) and a valid token succeeds; assertion that tokens never appear in logs. |
| K7 | Source: Home Assistant (`rest_command`-compatible ingest endpoint) | Essential | E2E test with a sample HA payload producing an event on the correct calendar. |
| K8 | Source: Kenny's Claude sessions via a token-scoped REST API (Almanac is the only thing that ever holds the Google service account credentials) | Essential | Test creating/updating/deleting an event via the REST API using a session token. |

*K8 amendment, 2026-08-29:* the delete verb its acceptance criterion asks for was never built. The Phase 7 gap audit found it; Kenny asked for it during the closing form ("delete moet er uiteraard nog bij"). `DELETE /v1/ingest/{source_id}/events/{external_id}` now exists, addressed by the external id the source itself used rather than by Google's event id — the caller never has to have kept it. A source can only ever address keys under its own prefix, so one source cannot delete another's events even knowing the external id.
| K9 | Source: alert systems (Uptime Kuma, Grafana webhooks) → dedicated infra calendar | Essential | E2E test per system with a sample webhook payload. |
| K10 | Source: Super Productivity mini-plugin | **Later** — explicitly deferred, lowest priority, possibly the last thing added | Defined only if/when picked up. |
| K11 | Debug/introspection surface — structured logs plus a status/query endpoint showing what came in, which profile routed it, what went to Google (no UI) | Essential | Test querying the debug surface for a processed event and getting back the expected routing info. |
| K12 | Secrets via Latch — full replacement of Infisical, local and CI | Essential | Test asserting no secret appears in plaintext in logs, new commit history, or process arguments. |
| K13 | CI: full test suite gates every push, red blocks merge | Essential | The CI setup itself is the evidence: red on a deliberately broken test, green on a healthy commit. |
| K14 | All-day events — a profile can produce a day marker rather than a timed block | Essential | A profile with `all_day = true` produces a Google event carrying `start.date`/`end.date` and never `dateTime`; a timed profile produces the opposite. Both asserted, since sending both is what Google rejects. *(Added 2026-08-29 via mini-round.)* |
| K15 | Location reachable from a mapping profile | Essential | A profile naming `location_field` puts that payload field on the event; the pinned regression fixture shows it. Closes a half-built field that was serialized and always empty. *(Added 2026-08-29 via mini-round.)* |
| K16 | Reminders per profile — a set of overrides, or deliberate silence | Gewenst | A profile asking for reminders produces them on the event; one asking for silence produces `useDefault: false` with no overrides; one saying nothing omits the block so the calendar's own default applies. *(Added 2026-08-29 via mini-round.)* |
| K17 | Free/busy and event status per profile | Gewenst | A profile with `busy = false` produces `transparency: "transparent"`, so an infra incident does not mark Kenny busy; `status_by` maps a payload field onto Google's three statuses and rejects any other value at startup. *(Added 2026-08-29 via mini-round.)* |
| K18 | Event length from the payload — a profile may name the field holding the end time instead of a fixed duration | Essential | A profile with `end_field` produces an event ending where the payload says; setting it alongside `duration_minutes` or `duration_days` is refused at startup. *(Added 2026-08-29 via mini-round.)* |
| K19 | *(3.0.0: the kit's `update` subcommand, same contract)* `almanac update` — one supervised update, no restart, for a manager that owns the restart and the rollback | Essential | Installing under supervision leaves no probation state, while the ordinary path still writes one; both asserted. *(Added 2026-08-30 at Kenny's instruction — see amendment.)* |
| K20 | *(3.0.0: the kit's `ALMANAC_STATE_DIR`, probed at `--check` and start; the per-path overrides are deprecated)* One documented knob for the whole state tree — `ALMANAC_STATE_DIR`, with every path derived from it | Essential | A profile tree and a data tree both move by setting one variable; the four existing per-path settings still win where present, asserted against the live deployment's exact configuration. *(Added 2026-09-01 — standing rule 28.)* |
| K21 | Manage sources from the dashboard — add one with a name and a calendar chosen from a dropdown of what exists (or a new one, created and shared on the spot), writing the profile itself and loading it without a restart; delete one, which revokes its token and removes its profile; plus a reload for profiles placed by hand | Essential | Round trip: a profile written through the surface is read back by `load_all`; a second source on the same calendar reuses it rather than creating a duplicate; a name that would escape the profiles directory is refused and creates neither file nor calendar; an unknown calendar without `ALMANAC_CALENDAR_OWNER` is refused rather than created invisible; a duplicate `source_id` is refused before it can break the next start; a `source_id` that would escape the profiles directory is refused and writes nothing outside it; an existing file is never overwritten; deleting removes the file, refuses while that source still has undelivered events, revokes its token, and leaves calendar events alone. *(Added 2026-09-02 via mini-round — see amendment note below.)* |
| K23 | A source speaks Almanac's own event shape — every per-event option (all-day, colour, free/busy, status, reminders, length, timezone) travels in the call rather than sitting fixed in a profile; the profile becomes routing only, and translating other shapes moves to HTTPSwitchboard | Essential | A pinned fixture using every option; a call with an unknown field refused by name; a call with neither `external_id` nor an `Idempotency-Key` header refused at ingest; a v1 profile refused with a message saying what changed. *(Added 2026-09-03 via mini-round — see amendment below.)* |
| K24 | A calendars panel on the dashboard — make one by name, and see every calendar with the sources that write to it; delete is offered only for a calendar no source uses | Essential | A calendar in use renders no delete control and the endpoint refuses it by name; one nothing writes to is deletable and gone at Google; making the same name twice makes one calendar; making one needs a session. *(Added 2026-09-03 via mini-round — see amendment below.)* |
| K25 | The kp-themes palettes on the dashboard — every theme the package ships, with the package's own picker, stored and applied exactly as `@kp-soft/themes` defines | Essential | Every theme in the vendored registry rendered as an option, checked against that registry rather than against a list typed twice; each option wearing its own theme as its swatch; the stored contract (key `theme`, default `formal`, the `.dark` toggle) asserted against the vendored modules; the assets served and the no-flash script proven to sit in the head, settling Bootstrap's own switch in the same breath. *(Added 2026-09-03 via mini-round; v1.0.0 adopted 2026-09-04 — see the amendments below.)* |

## Round 2 — Claude's proposals (gaps, hardening, quality-of-life)

| ID | Feature | Rating | Test expectation |
|---|---|---|---|
| M1 | Health/readiness endpoint (`GET /healthz` or similar) *(3.0.0: the kit's shape — `status`, `version`, one `journal` subsystem; still unauthenticated and Google-blind; 503 only when the journal cannot be read)* | Desired | Test asserting the route returns 200 without requiring auth (deliberate exception to K6 — health checks carry no secrets). |
| M2 | Graceful shutdown — SIGTERM/SIGINT drains in-flight requests before stopping | Essential | Test sending a shutdown signal during an in-flight request, asserting it still completes cleanly. |
| M3 | Retry with backoff on transient Google API errors (429/5xx) | Essential | Test simulating a 429/503 followed by success; asserts the event still lands without the caller retrying itself. |
| M4 | Config & mapping-profile validation at startup, with actionable error messages | Essential | Test with a deliberately broken profile asserting startup fails with a message naming the specific problem. |
| M5 | Rate limiting / request body size caps on inbound endpoints | **Later** | Defined only if/when picked up. |
| M6 | Per-source timezone support (currently hardcoded UTC) | Desired | Defined during design — no current known practical failure, revisit if a source needs it. |
| M7 | Idempotency-key support for sources/payloads without a natural external ID (fills the gap K2's upsert leaves for ad-hoc, ID-less calls) | Essential | Test sending the same call with the same idempotency key twice, asserting one event results. |
| M8 | One coherent versioning/tagging scheme; Docker image tagged to match the git version (fixes the current drift between manual and CI auto-tagging, and `:latest`-only image pushes) | Essential | CI check asserting the pushed image tag exactly matches the git tag from the same run. |
| M9 | Dry-run/validation tool for a mapping profile — shows the `GoogleEvent` a profile+sample payload would produce, without writing to Google | Desired | Test feeding a profile + sample payload, asserting the expected (unsent) `GoogleEvent` structure comes back. |
| M12 | Management dashboard — login (remember-me cookie + logout, not browser basic auth), register a source and generate/revoke its token, copy-paste commands carrying a real token (masked, reveal for 10s, copy without displaying), plus the K11/M11 status views. `/healthz` and any metrics stay open so monitoring cannot fail closed. | Essential | Every page rendered with seeded state; a revoked token stops working *immediately*; the printed command carries a working token; plaintext-scan proving no token reaches logs, metrics or any page except behind the reveal control. *(Added 2026-08-28 via mini-round during the L3 report — see amendment note below.)* |
| M13 | Prometheus metrics endpoint — delivered events, failed deliveries, journal depth, entries set aside, token refreshes, exposed in the Prometheus text format | Gebouwd | Scraped successfully by the real Prometheus on CT 113; a test asserting no token, calendar id or payload content appears in the output. *(Added 2026-08-29 via mini-round — see amendment note below.)* |
| M14 | A heartbeat line — one INFO line per interval carrying the counters, the journal depth, the source count and uptime, whether or not anything happened | Essential | The first line lands after one interval rather than at startup; it keeps coming; it stops on shutdown; it carries the numbers someone would look for; the interval is configurable and `0` switches it off, while a typo falls back to the default rather than to silence. *(Added 2026-09-03 via mini-round — see amendment below.)* |
| M11 | Raw request capture — a debug endpoint that accepts any inbound request, stores it verbatim (headers + full body, in memory, capped and expiring), and hands it back on request, so an undocumented webhook's real shape can be observed before a mapping profile is written for it | Essential | Test posting an arbitrary payload to the capture endpoint and reading back the exact headers and body; test that the cap and expiry both hold. *(Added 2026-08-28 via mini-round during L2 — see amendment note below.)* |
| M10 | *(3.0.0: delivered by chassis — signature bound to the version, skip after rollback, capture guard via the kit's update gate; Almanac's own updater removed)* Full self-update — the running service checks for, verifies (checksum manifest), and applies new versions itself; keeps the previous binary, verify-before-replace, clean handover of port and in-flight requests | Essential | E2E test against a local mock release: old binary updates to new, health endpoint answers throughout minus the swap window, rollback works when the new binary fails verification. *(Added 2026-08-28 via mini-round during Phase 4 — see amendment note below.)* |

## Tally

| Rating | Count | IDs |
|---|---|---|
| Essential | 20 | K1–K9, K11–K13 (12), M2, M3, M4, M7, M8, M10, M11, M12 (8) |
| Desired | 3 | M1, M6, M9 |
| Later | 2 | K10, M5 |
| Don't do | 0 | — |
| **Total** | **25** | |

No items were flagged as missing in either round; both open-items fields
were left blank.

## Freeze

**Frozen 2026-08-28.** Kenny confirmed the original tally (22 features)
via the Phase 2 report form. Changes from here on go through a
mini-round only (`FORM_PROTOCOL.md` §5).

**Amendment 2026-08-28 (mini-round, during Phase 4):** while deciding
AR19 (update mechanism) Kenny challenged the assumption that the
CI/Docker flow counts as self-update and asked for real self-updating
software. A mini-round added **M10 · Full self-update**, which Kenny
rated **Essential**. The tally above includes it. Consequences for
built work: none (nothing built yet); consequences for planning: the
release flow must produce a checksum manifest before M10 can be built,
and M10's design is coupled to M2 (graceful shutdown) and AR16 (journal
buffers during the swap) — see `ARCHITECTURE_DECISIONS.md` AR19.

**Amendment 2026-08-28 (mini-round, during the L2 report):** Kenny
raised that many apps ship webhooks without documenting their payload
shape, so writing a mapping profile for one means guessing. A
mini-round added **M11 · Raw request capture**, which he rated
**Essential**. It is distinct from K11 (which shows what happened
*after* an existing profile processed an event): M11 captures verbatim
what arrived *before* any profile exists for that source. Consequences
for built work: none; planned into L4 alongside K11, with which it
shares the admin-token surface.

**Amendment 2026-08-28 (mini-round, during the L3 report):** three of
Kenny's four L3 follow-up questions assumed a UI that did not exist.
The underlying need turned out to be concrete rather than cosmetic:
tokens for every service have to be manageable without SSH-ing into
the LXC for each one. A mini-round added **M12 · Management
dashboard**, rated **Essential**, modelled on `kyu`'s W2 so the
two services are managed the same way. It carries a matching change to
AR17 (tokens encrypted at rest rather than hashed, and a single
authentication path — Kenny rejected Claude's proposal to keep
hand-managed profile hashes alongside the store, on the grounds that
two parallel paths drift apart). Consequences for built work: the
profile schema loses `token_hash`; `examples/issue_token.rs` is
superseded by the dashboard. Bootstrap CSS is vendored into the repo
and image rather than loaded from a CDN, since a LAN-only service must
not need the internet to render its own status page.

A general, cross-service key manager — Kenny's larger ambition — is
deliberately **not** folded into Almanac. Almanac repeats the kyu
pattern; the central-issuer idea is recorded as an ecosystem candidate
for its own project so it gets its own scope phase rather than being
smuggled in here.

## M13 amendment (mini-round, 2026-08-29)

The frozen list had no metrics feature. The word appeared exactly once,
in passing inside M12: "`/healthz` and any metrics stay open so
monitoring cannot fail closed", and again in its acceptance criterion
"no token reaches logs, metrics or any page". Anticipated in the design,
never specified, never built.

**The new insight:** a Prometheus now runs on CT 113 and already scrapes
kyu and the Proxmox fleet, and Kenny named Almanac as a target in
his metrics form. Without this endpoint Almanac would be the only
service in the fleet with no metrics — and the numbers already exist
inside it, kept in memory for the debug page and thrown away on every
restart.

A detour worth recording: the first suggestion was to point Prometheus
at `/healthz`. The homelab session corrected that — Prometheus parses
its own exposition format, not JSON, so the target would sit permanently
"down" on a service that is running perfectly. That correction also
matches AR21, which already names Uptime Kuma as the watcher of
`/healthz`. Liveness there, metrics here.

**Consequences for what is already built:** none. A new endpoint beside
the existing ones on the same port, nothing rebuilt.

**Decision:** adopted as Desired, 2026-08-29 — after the deployment
drills (Traefik route, reboot, self-update on hardware), not before. The
same no-tokens rule that the dashboard and the log already enforce with
tests applies to it.

## Google Calendar field coverage (mini-round, 2026-08-29)

Kenny asked whether Almanac can use everything a Google Calendar event
offers. It could not, and nowhere said so: the event model carried seven
of Google's fields and `docs/SCOPE.md` never listed that as a limit.

**Tested against reality first, and only half of it could be.** The
three sources that exist today all send point-in-time incidents — the
pinned fixtures show Grafana sending `startsAt`/`status`/summary and
Uptime Kuma sending `time`/`status`/monitor name. None of them needs a
day marker, a reminder or a repeat. Everything asked about lives on the
household side of Almanac, and nothing is connected there yet: a search
of Home Assistant found no waste sensors and no calendar entities. The
recommendations for K14, K16 and K17 were therefore presented as
hypotheses rather than as tested proposals, and Kenny rated them
without a worked example of his own ("weet ik zelf nog niet").

**Adopted:** K14 all-day (Essential), K15 location (Essential), K16
reminders (Desired), K17 free/busy and status (Desired).

**Declined, with reasons worth keeping:**

*Recurrence (`RRULE`)* — **Don't do.** Not for lack of value: it has a
genuine design problem that deserved its own round rather than a field.
Almanac's whole model is one payload, one event, and K2's upsert rests
on it. A recurring event is one Google event with instances beneath it,
so an update from a source either rewrites the series or one occurrence,
and both answers are defensible. Half-building it is how a source
silently overwrites a whole series one day. The workaround is real: a
source posts each occurrence, e.g. a week ahead every Monday.

*Attendees* — **Don't do.** Adding guests means Almanac starts sending
mail to people. A profile mistake stops being a wrong calendar entry and
becomes an invitation to the wrong person, which cannot be taken back —
a different class of consequence from everything else here. Sharing the
calendar already solves the household case.

*Attachments, Meet links, visibility* — **Don't do.** Attachments need
Drive scopes Almanac deliberately does not hold; Meet links belong to
meetings with people, not to bin day; per-event visibility only matters
on one calendar shared with several people, and a second calendar is
simpler and already supported (K3).

**Consequences for what is already built:** none of the four additions
changes an existing profile. Every new key is optional and absent means
today's behaviour. `duration_minutes` becomes optional rather than
required, because an all-day profile has no minutes to give — existing
profiles that supply it are unaffected.

## Variable event length (mini-round, 2026-08-29)

Found while building Almanac's first real source rather than by
testing — the third time in one day that using the thing found what 267
green tests did not.

**The case.** Kenny's Home Assistant knows when electricity is cheap:
the EPEX sensor carries all 96 quarter-hours of the day with a price
position for each, so the actual cheap windows can be computed rather
than guessed. Verified against live data before proposing anything: on
2026-08-29 that yields one contiguous window from 08:45 to 16:45.

**What pinched.** A mapping profile can say "start at this field" but
only "and last this many minutes" as a constant. A cheap-power window is
480 minutes today and might be 45 tomorrow. The length is in the
payload and no profile could reach it.

The workaround was available and rejected: post a fixed hour and put the
real window in the title. That puts a one-hour block on the calendar for
an eight-hour window — a calendar showing something other than what it
says, which is worse than no calendar entry.

**Decision:** `end_field` adopted as Essential. Exactly one of
`duration_minutes`, `duration_days` and `end_field` may be set, checked
at startup like the existing all-day contradiction. Absent, a profile
behaves exactly as before.

**Consequences for what is already built:** none. Every existing profile
uses `duration_minutes` and is untouched.

**Why Essential rather than Desired:** every source that reports a
*period* rather than a *moment* hits this, and that is not only energy
prices — "the washing machine ran from X to Y", "away from Monday to
Friday", "the backup took three hours" are all the same shape.

## Supervised updates (2026-08-30)

Kenny: *"Zorg dat het Homelab Rust dit project binnenkort kan beheren,
dan kan dit gesprek gearchiveerd worden."* Recorded as an amendment
rather than a mini-round form because the instruction *is* the decision;
what follows is only how it was carried out.

**The state before.** The homelab adopted CT 112 on 2026-08-29 and backs
it up nightly, but `stacks/almanac/service.yml` deliberately carried no
`update_cmd`, with the note: *"the app has (or may have) its own
complete rollback mechanism, and two systems restoring binaries can
fight each other. Ownership is Kenny's call (form pending)."*

**Why almanac could not simply be handed the job.** The homelab's
supervised update preserves the binary, runs `update_cmd`, and restarts
**only if the binary actually changed** — then health-checks and, on
failure, restores the preserved copy from outside. Almanac's own updater
restarts itself and arms its own revert. Pointing `update_cmd` at that
would give two systems a rollback each, and they would race.

**The split that resolves it.** Almanac knows how to find a release,
verify its signature and checksum, and prove the new binary starts on
this machine. The homelab knows how to restart a unit and restore a
binary when the process is dead — something a dead process cannot do for
itself. So: `almanac update` does the first half and stops, writing no
probation state; the homelab does the second half.

`ALMANAC_SELF_UPDATE=off` on the deployment stops the periodic updater,
so only the supervisor initiates. The explicit `update` command still
works with that set — the variable governs the background loop, not an
instruction from whoever is in charge.

**Consequences for what is already built:** none. The unsupervised path
is unchanged and still arms AR23's revert, with a test asserting it, so
a machine running almanac without a supervisor keeps exactly today's
behaviour.

## State has an address (2026-09-01)

Requested by the homelab session on Kenny's instruction (his form item
A1, 2026-08-31), and now a standing requirement rather than a one-off:
dev-procedure **rule 28**, *state has an address, and Kenny owns it*,
with a mandatory Phase 2 item behind it. Verified in that repo before
acting on the report rather than taken on trust.

**How it was found.** The homelab is moving the four native Rust
services onto bind-mounted host paths, so a container can be destroyed
and recreated for nothing and the host's restic job can reach the state.
It tried almanac on 2026-08-31 and the attempt failed live — eight
minutes down, reverted, nothing lost. Almanac was the one service in the
house that could not be moved.

**What was actually wrong.** Almanac had four independent settings —
`ALMANAC_PROFILES_DIR`, `ALMANAC_DATA_DIR`, `ALMANAC_JOURNAL`,
`ALMANAC_TOKEN_STORE` — whose *defaults happened to* form a coherent
tree. Happening to agree is not being derived. There was no single thing
to move, and the deployment set all four absolutely, so relocating meant
editing four values in agreement and hoping.

**Decision.** `ALMANAC_STATE_DIR` names one root; `profiles/` and
`data/` are derived from it, and the journal and token store from the
resolved data directory rather than from the root — someone who moves
only the data directory means the journal too, and a journal separated
from the lock that guards it is two processes away from a corrupted log.

The four per-path settings stay, and a specific one wins over the root.
Deployments already set them, and a release that silently relocated a
live journal because a tidier knob had appeared would be the worst kind
of upgrade.

**No cache is excluded because there is no cache.** Rule 28 asks for
regenerable state to live outside the backed-up root; almanac keeps none
on disk, and saying so is more useful than inventing a directory to
satisfy the shape of the rule.

**Consequences for what is already built:** none, deliberately. The
default root is `.`, which reproduces the previous relative defaults
exactly, and there is a test asserting CT 112's four absolute settings
resolve unchanged. The migration is the homelab's to perform when it
chooses.
## K21 amendment (mini-round, 2026-09-02)

**Where it came from.** Kenny opened `/dashboard/sources` to add a
source and could not find the button. He was not misreading the page:
the dashboard listed the loaded profiles and offered *Issue*,
*Re-issue* and *Revoke* per profile, and nothing else. Adding a source
meant logging into CT 112, writing a `.toml` file by hand, and
restarting the service — because profiles were read exactly once, at
startup.

The user guide meanwhile said the dashboard would "register the source",
which is why he went looking. That sentence has been corrected in the
same change.

**What was decided, and then corrected the same evening.** The first
version asked for the whole mapping profile in a textarea, validated by
`Profile::parse` so the browser held no second copy of the rules. Kenny
rejected it on sight: *"dat zou enkel een naam van de bron en de naam
van de target kalender moeten zijn"* — and he was right about his own
sources. Measuring the three deployed profiles shows why the first
version looked reasonable and was still wrong: they differ in almost
every field (`title` vs `commonLabels.alertname` vs `monitor.name`),
because each matches a third-party webhook that will not change what it
sends. A source Kenny adds himself is one he controls, so it is cheaper
for the source to speak Almanac's shape than for Almanac to learn a
fourth.

So the form is two fields, and the profile it writes is the plain shape
— field for field the deployed `home-assistant` profile that
`tests/mapping_regression.rs` already pins, with `external_id` as the
id field.

*Amended 2026-09-03, same day:* that field was first left out, reasoning
that naming it makes it required in every payload and so would refuse a
new source's first post. The JobTracker session measured what the
omission actually cost against the live service — two identical posts
produced two events, and the delete endpoint answered 404, because
without the field Almanac writes no marker and can never find its own
event again. A refusal that names a missing field is recoverable in
seconds; duplicates nothing can remove are not.

Anything the plain shape cannot express is still a file, edited by hand
and picked up by the reload — which is what the three existing profiles
are.

**The calendar is chosen from a list, never typed as an id.** A
calendar id is an opaque string nobody types on purpose, and having to
go and find one first was half of why adding a source was a chore. The
dropdown lists what the service account can see; *+ New calendar…*
reveals a name box and creates one on submit. Kenny's own framing when
he asked for it: he wants as many calendars as he likes, and coupling
one to a service should happen in the same act as making the service.

Creating goes through find-or-create rather than a bare create,
because a duplicate calendar is close to
invisible: events land, nothing errors, and half of them are on a
calendar nobody has open. Creating requires `ALMANAC_CALENDAR_OWNER`;
without it an unknown name is refused rather than turned into a calendar
owned by the service account and visible to no human, a mistake this
project has already made twice.

**What it changed in what was already built.** Profiles moved behind a
lock so the set can be swapped while the service runs; readers take an
`Arc` and drop the guard immediately, so a reload never blocks a
request. `source_id` gained a character rule — it was only ever checked
for being non-empty, and it is now also a filename, so
`"../../etc/cron.d/x"` had to stop being a legal value. The three
deployed source ids are unaffected and a test says so.

**Ending a source: kyu's model, by name.** Kenny asked for deletion and
then said "kopieer het kyu model" — where revoking an app keeps its row
with a badge rather than erasing it. *Retire* therefore revokes the
source's token and renames its profile to `<source_id>.toml.retired`,
which the loader does not read. The file stays, the row stays, and
renaming it back plus a reload undoes it. A source retired, recreated
and retired again keeps both files: the older one is the record of a
different configuration.

*Revoke token* keeps its old meaning — take the key away, leave
everything else — and is relabelled so the two are not confused.

**A retirement is refused while that source still has undelivered
events.** The worker resolves an entry's calendar through its profile,
and the journal never drops an entry; retiring first would leave those
entries unreachable, erroring on every pass forever. The refusal says
how many are waiting.

**Deliberately not built:** editing an existing profile from the
dashboard, and deleting anything outright. Replacing a working profile
because a `source_id` was retyped is the one mistake that could not be
undone from the same page, so a save that would overwrite is refused.
Neither adding nor retiring touches events already on the calendar —
those belong to the calendar now, and a button that silently swept up
months of entries would be the most expensive click on the page.


## K23 amendment (mini-round, 2026-09-03)

**The question that started it.** Kenny read the dashboard's own help
text — "a colour per severity, an all-day event — is a line in that
file" — and asked why. *"Die moeten natuurlijk gewoon in de api call die
we vanuit onze sources krijgen zitten."*

**He was right, and the measurement said how right.** In the v1 format
`all_day`, `busy`, `duration_minutes`, `duration_days` and the reminders
were *static*: one value for every event that source would ever send.
Colour and status were half payload-driven — the profile named a field
and carried a value→colour table. So a source could not say "this one
event is all-day" or "this one is red". Only the profile could, for all
of them at once.

**Why it had been built that way**, which is the part worth keeping: the
first three profiles matched webhooks nobody here controls. Grafana
sends `commonLabels.alertname`; Uptime Kuma sends `monitor.name`. They
do not change for us, so Almanac changed for them.

**What settled it.** Offered the choice between adding a direct mode
beside the translation and removing the translation entirely, Kenny
chose to remove it: *"voor aanpassingen hadden we HTTPSwitchboard! dus
doe het volgens mijn model!"* — his own message-shape translator, built
and drilled, exists for exactly that job.

The measurement that made this cheap: **Grafana and Uptime Kuma had
never delivered a single event.** The whole journal history on CT 112
was home-assistant (5), the since-deleted energy-prices (4) and
job-tracker (2). The objection "but those webhooks cannot change" was
real in theory and empty in fact — there was nothing to break. Their
profiles and fixtures were removed with the layer they existed to prove.

**Two things came out of the rewrite that were not asked for.** The
`grafana` profile asked Google for colour `"tomato"`, and Google's API
takes `colorId` — the string `"1"` to `"11"`. It would have been refused
or ignored, and nobody would have known, because that profile never sent
an event. Colours are now named and translated, and an unknown one is
refused rather than silently producing an event in the default colour.
And a call carrying neither `external_id` nor an `Idempotency-Key` is
now refused at the door rather than becoming an event Almanac can never
find again — the fault the JobTracker session measured hours earlier.

**What a profile is now:** `source_id`, `target_calendar_id`, and two
defaults a call may leave out. Nothing that describes an event.


## K24 amendment (mini-round, 2026-09-03)

**Where it came from.** Kenny used the K21 add-a-source form against the
live service — the calendar was created and the sharing mail arrived, so
that path is proven end to end — and then asked for the calendar half to
move out of it: *"We gaan de optie om een nieuwe kalender uit die
dropdown halen. We gaan in de plaats een nieuw paneel maken waar we de
kalenders kunnen beheren."*

He is right that they are two jobs. Adding a source is a frequent, small
act; making and removing calendars is rarer and heavier, and mixing them
put a destructive capability inside a form people use casually.

**What the panel shows.** Name, the sources that write to it, and a
delete. The middle column is the one that matters: Google knows what a
calendar is, only Almanac knows who writes to it, and that is exactly
what decides whether deleting is safe.

**Delete is disabled, not hidden, and not merely refused.** Kenny's
rule: it becomes live only when no source uses that calendar. A dead
button that says why ("2 source(s) still write here") tells someone the
capability exists and what to do first; a missing button tells them
nothing. The endpoint repeats the check on arrival, because the page is
a snapshot and a source can be added between the render and the click.

**Deleting a calendar deletes every event on it, for everyone it is
shared with.** That is Google's semantics, not a choice this project
made, and the panel says so in those words. It also names the
interaction with K21's source delete: deleting a source deliberately
leaves its events alone, so a calendar emptied that way still holds
them until it is removed here.

**Making one is find-or-create.** A double submit, two tabs, or a
retyped name must not produce a second calendar. A duplicate is close to
invisible — events land, nothing errors, and half of them are on a
calendar nobody has open.


## K25 amendment (mini-round, 2026-09-03)

**What Kenny asked for:** the kp-themes themes, "dezelfde theme picker
in de interface en dezelfde manier om die themakeuze op te slaan in de
browser".

**Why this is mostly not almanac's code.** The package ships a React
hook and a JSX switcher; this dashboard is server-rendered HTML out of a
Rust binary with no npm and no build step, so neither can be used. What
transfers is the CSS and the *contract* — `localStorage` key `theme`,
`data-theme` plus `.dark` on `<html>`, default `formal`.

While this session was working, the kyu session was porting exactly the
same thing for exactly the same reason. Two behaviour-only ports of one
stored contract is the duplication kp-themes exists to prevent, one
level up, so almanac takes kyu's files verbatim rather than writing a
second version. `themes.css` is the package's own file, vendored with
its version in its header; a copy goes stale silently, and saying so in
the file is the only thing keeping that honest.

**The seven themes live once, in Rust.** `theme.js` deliberately carries
no list: it reads `data-theme` and `data-dark` off the options the
server rendered. JobTracker's session made that point first — derive the
dark set from the metadata rather than copying today's answer, because
an eighth dark theme would leave a hardcoded list of three quietly
wrong.

**Two real findings that went back to the shared bridge.** Bootstrap's
`.bg-*` utilities read `--bs-body-bg-rgb` — a comma-separated triple no
`hsl()` token can produce — with `!important`, so a themed page keeps
Bootstrap's own background unless those utilities are pointed at the
tokens directly. And the navbar's link colours are hardcoded per
Bootstrap theme rather than read from variables.

**A correction worth recording.** The first version of both comments in
that file cited measurements taken through `getComputedStyle` in a
preview pane, which turned out to report a stale value — a literal
colour set inline read back as the previous theme's. The claims were
rewritten to cite what could be read out of the loaded stylesheet
itself, which is checkable. An explanation that sounds measured and is
not is worse than none: the next person believes it.


## M14 amendment (mini-round, 2026-09-03)

**Where it came from.** Kenny looked at the almanac dashboard in Grafana
and saw "no data" everywhere. Two causes sat under it; one was the
homelab's (no log shipper on that container, since fixed) and one was
ours: almanac had written nothing to its log in 48 hours and was, from
the outside, indistinguishable from a service that had died.

**It had not died, and the measurement said so.** `accepted`,
`delivered` and `pending` were all zero: nothing was posting, and a hub
with no traffic has nothing to say. Almanac does log per accepted and
per delivered event; there was simply nothing.

**The part nobody had noticed.** Almanac used to have a heartbeat by
accident — the self-updater logged `checked for a new release` every six
hours, deliberately whether or not there was one, because a silently
stopped updater and a working one otherwise look identical (a real bug,
0.1.3, found on hardware). When the homelab took over updates on
2026-08-30, `ALMANAC_SELF_UPDATE=off` switched that task off and took
the only recurring sign of life with it. Correct on its own terms, and
it removed something load-bearing that nobody had recognised as such.

**Why not just read the counters.** They were the honest counter-argument
and Kenny weighed it: `/metrics` already answers "did almanac process
anything today", and `/healthz` already answers "does the process
respond". Neither answers "is the background work still turning" — and
that is exactly the failure almanac has actually had, when the update
loop ticked six hours late while nine tests passed and the process
answered every request. Standing rule 23 exists because of it: *a
periodic background task logs one line per cycle, even when there was
nothing to do.*

**The interval is a knob** (`ALMANAC_HEARTBEAT_INTERVAL_SECS`, default
3600). `0` switches the line off, which is a real answer for a machine
whose logs are precious. A value that cannot be parsed falls back to the
default rather than to silence: a typo must not quietly disable the one
thing whose job is to report silence.

**The first line comes after one interval, not at startup** — and that
detail has its own test, because it is the same shape as the updater
bug: a tokio interval fires immediately, so a loop that does not consume
that first tick both duplicates the startup lines and then drifts by a
whole period.

## K25 second amendment (v1.0.0 adopted, 2026-09-04)

**What Kenny asked for:** kp-themes released 1.0.0, and almanac should
take the themes as they now exist.

**Four themes almanac could not offer, and nothing was failing.** The
package went from seven themes to eleven — `high-contrast`, `sepia`,
`blueprint` and `solstice` are new. The commit gate compared the one
file almanac had vendored, and that file was in step, so the gate was
green while the picker was a version behind. A check that guards one
file guards one file; the list in Rust was outside it.

That is now covered from the other side: a test reads the vendored
registry and asserts that almanac offers exactly the themes it names,
in both directions. It runs on CI too, where kp-themes is not on the
machine — it reads what almanac shipped rather than what is upstream.

**The behaviour is no longer almanac's own.** v1 ships a framework-free
picker built for exactly this case — its own comments name almanac and
kyu as the reason it exists — so the hand-written `theme.js` is gone and
the package's three modules are vendored instead. Almanac still writes
the markup, because a menu that only exists once JavaScript has run is
an empty box on first paint.

**21 copied colours are gone.** Each option used to carry two hand-typed
hex values to draw its swatch. A swatch now wears the theme:
`<span class="kp-swatch" data-theme="…">` reads that theme's live
tokens, because `data-theme` works on any element. The most ordinary
change upstream is adjusting a palette, and it was exactly the change
that would have left almanac previewing a colour the theme no longer
had, with nothing to notice it.

**What stayed almanac's own, deliberately:** `theme-bootstrap.js`.
Bootstrap decides light or dark from `data-bs-theme`, which the package
knows nothing about and should not. It listens to the package's own
`kp-theme-change` event rather than to the buttons, so it keeps working
if the markup moves — and it keeps no list of dark themes. Every theme
declares its own `color-scheme`, so it asks the browser what is applied
instead of holding an answer a palette change upstream would falsify.
The head settles the same thing once, placed after the stylesheets
because that is the first moment the declaration is readable, and still
inside `<head>`, so nothing has painted.

Almanac's first attempt printed a list of dark theme names from Rust
into that snippet. The kyu session, porting the same release an hour
earlier, had made that exact mistake before — believing in a fourth
dark theme when there were three — and said so unprompted. There are
four now, which is the argument.

**Proven by clicking it**, not only by tests: the local dashboard, all
eleven themes in the menu, `solstice` applied and surviving a page load
with the cards, tables and navbar following the tokens rather than
staying Bootstrap-light.

## K25 third amendment (v3.0.0 adopted, 2026-09-05)

**What Kenny asked for:** kp-themes released 3.0.0 — "every feature of
every component became configurable" — and almanac should take it, in a
style consistent with how other projects adopt the same release, without
Claude talking to the kp-themes project itself.

**The picker had gone quiet, and nothing would have said so.** Since
3.0.0 the framework-free modules are pure: importing `theme-picker.js`
attaches nothing by itself. Almanac's page still loaded it exactly the
old way — one `<script type="module" src="…">` tag — which would have
kept building, kept passing its existing tests (they check the markup
and the assets, not whether a click does anything), and produced a
picker whose buttons open the popover and do nothing else. The head now
imports `attachThemePickers` explicitly and calls it.

Weighed against `js/auto.js`, which upstream's own comments name almanac
as a consumer of: that one script also wires up datatables, comboboxes,
date pickers, wizards, uploads and six more components almanac's
dashboard does not render anywhere. Attaching only the picker keeps the
vendored surface — and the commit gate's checksum list — proportional to
what almanac actually ships, at the cost of not matching upstream's own
suggested one-line integration. Decided here rather than asked upstream,
per this round's instruction; worth revisiting if almanac ever adopts a
second kp-themes component.

**The picker groups themes into light and dark now, by default.** That
split has to exist before the page paints, because almanac writes the
whole menu server-side — and "which themes are dark" is precisely the
fact K25's second amendment already stopped hand-keeping in Rust, after
kyu's session was found believing in a fourth dark theme that did not
exist. `dark_themes()` reads the flag out of the same vendored registry
`k25_the_rust_theme_list_matches_the_packages_own_registry` already
checks almanac's names against, so there is exactly one place that list
can come from, and a new test
(`k25_the_picker_groups_light_and_dark_from_the_registry_not_a_copy`)
checks every theme lands in the section the registry says it belongs in.

**`theme-picker.js` now imports `js/strings.js`** for its status text —
the message shown when a browser refuses to remember the choice. Vendored
as a sixth file rather than configured: almanac's UI is English (standing
rule 1), which is the package's own default since 3.0.0, so there is
nothing to call `setStrings()` with. The eleven theme *names* stay Dutch
(Formeel, Donker, Zonnewende, …) — README frames them as Kenny's names
for his themes rather than interface chrome, the same reason they did
not follow the package's default to English earlier either.

**Fixed in passing, found while rewriting the function these both live
in:** the picker's `aria-label` had read "Thema kiezen" since v1.0.0, in
a dashboard whose own header comment says the UI is English. A leftover,
almost certainly, from when the package's own default was Dutch too —
it is "Choose a theme" now, and so are the new group headings.

**Proven by clicking it**, not only by tests: the local dashboard, the
menu now split into a "Light" and a "Dark" section, every theme in the
section the registry puts it in, a theme click still applying and
surviving a reload.

## Step 2 amendment (the kit's dashboard and door, 2026-09-06)

Decided in the Almanac step-2 form and its deep-dive round (A2-1…A2-4,
Kenny, 2026-09-06): every project works the way a new `chassis new`
project works, so the dashboard, the login, the session, the per-source
tokens and the door move to chassis-rs. Almanac 4.0.0.

- **M12 (dashboard).** The login page, the session cookie, logout and
  the token controls (issue, reveal for 10 s, copy a working command,
  re-issue, revoke) are the kit's, on its clients page labelled
  **Sources**. Almanac keeps two pages inside the kit's layout: `/sources`
  (profiles, calendars, unusable files — K21/K23/K24) and `/captures`
  (M11); `/` is the kit's status page with a Journal and a Sources
  section. The 3.x addresses `/dashboard`, `/dashboard/sources`,
  `/dashboard/captures` redirect. Proven by `tests/kit_dashboard.rs`
  through the real `chassis::App`; the plaintext-scan rule holds (the kit
  never renders a token into a page; `tests/kit_door.rs` asserts it for the
  imported ones).
- **K6 (per-source tokens).** A source's token is a kit client under the
  source's own name. The kit's door checks the bearer; the ingest handler
  then checks that the client's name is the source in the path (the admin's
  login token passes as any source, for Kenny's scripts). Unknown source
  and foreign token still answer the same 401. The 3.x tokens are copied
  unchanged into the kit's sealed `clients.json.enc` on the first start of
  4.0.0 (`shell::kit::import_source_tokens`), so JobTracker and every
  other source keep their configuration; `tokens.json` stays as history.
- **M11 (capture) — retired 2026-09-06 (4.0.1, A2-2 revisited).** The
  kit's K13 (last N requests per client token, on the source's row of the
  Sources page, credentials masked, body cut) is the capture surface; the
  kit's own FEATURES said so ("replaces Almanac's captures page") and the
  A2-2 form of the step-2 session contradicted it. Almanac's capture
  store, `POST /v1/debug/capture/{label}`, `GET /v1/debug/capture` and the
  Captures page are gone; `/captures` and `/dashboard/captures` redirect to
  `/clients`. `POST /v1/ping` answers any client (the **Send test**
  target). AR25 ("never restart under an investigation") retires with it.
  Proven by `tests/kit_dashboard.rs::a_ping_from_a_source_lands_on_its_row_as_a_last_request`.
- **K21 (manage sources).** Adding a source writes the profile and loads
  it without a restart as before; its token is then issued on the kit's
  Sources page (two steps instead of one button). Deleting a source removes
  its profile and its kit client. Reload and the unusable-files list are
  unchanged.
- **K24 (calendars).** Unchanged in behaviour, rendered by the `/sources`
  template; the create-lag and delete-lag memories still apply.
- **K25 (themes).** The kit's layout ships kp-themes 3.1.0 and its picker;
  Almanac's vendored copy, Bootstrap, `theme-bridge.css`, the checksum gate
  and the picker tests are gone.
- **Environment.** `ALMANAC_BOOTSTRAP_TOKEN` is renamed to the kit's
  `ALMANAC_TOKEN` (hard, no alias — A2-4: 4.0.0); a start with the old
  name set refuses and says so. `ALMANAC_SECRET_KEY` now also seals the
  kit's client and session stores (same 64-hex value).

