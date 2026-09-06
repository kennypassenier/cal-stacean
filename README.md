# Almanac

An adapter that turns webhooks from anything into Google Calendar
events, across several calendars.

## Why "Almanac"

An almanac is not a calendar. A calendar is the grid; an almanac is the
book that gathers what is *going to happen* — from many unrelated
sources, each with its own way of saying it — and lays it out on one
set of dates. Tide tables, planting dates, eclipses, feast days:
different observers, different formats, one volume.

That is exactly this service. Home Assistant, Grafana, Uptime Kuma and
a Claude session each speak their own webhook dialect; Almanac
translates each one and writes it onto the right calendar. It is a
gatherer, not the calendar itself — the calendar stays at Google.

The name also survives the project growing. "cal-stacean" was a pun on
Rust's crab tied to one integration that no longer exists; "Almanac"
still fits the day a fifth source is added.

## What it does

- **Many sources, one hub.** Each source gets its own ingest endpoint
  and its own bearer token, so one can be revoked without touching the
  others.
- **Many calendars.** A source's mapping profile decides which calendar
  its events land on — "infra", "hobbies", whatever the split is. This
  is the point of the project, not a feature bolted on.
- **Nothing is lost.** Every accepted payload is written to a durable
  journal and fsynced *before* the request is answered, so a crash or a
  power cut costs nothing. Undelivered entries go out on the next start.
- **Redelivery converges.** Events are upserted by a private property
  on the Google event, so retrying never produces a duplicate.
- **It explains itself.** A dry-run endpoint shows what a payload would
  become without writing it, and a capture surface records incoming
  requests verbatim — so a new source is reverse-engineered from what
  it actually sends rather than from a guess.
- **It runs unattended.** It survives reboots and power cuts, retries
  through outages instead of giving up, updates itself from signed
  releases, and puts the previous version back if a new one does not
  come up.

Google's credentials live in exactly one place — this service. Nothing
else in the homelab ever holds them.

## Documentation

Start with whichever question you have.

| I want to know | Document |
|---|---|
| How do I connect a source, change an event, delete one? | [docs/USER_GUIDE.md](docs/USER_GUIDE.md) |
| Why is it not on my calendar? | [docs/DEBUGGING_GUIDE.md](docs/DEBUGGING_GUIDE.md) |
| How do I install, release, restore, rotate the account? | [docs/OPERATIONS_RUNBOOK.md](docs/OPERATIONS_RUNBOOK.md) |
| How is it built? | [docs/ARCHITECTURE_REFERENCE.md](docs/ARCHITECTURE_REFERENCE.md) |
| Why is it built that way? | [docs/ARCHITECTURE_DECISIONS.md](docs/ARCHITECTURE_DECISIONS.md) |
| What is proven, and where? | [docs/TEST_PLAN.md](docs/TEST_PLAN.md) |
| What is it for, and not for? | [docs/SCOPE.md](docs/SCOPE.md) |
| What was agreed to build? | [docs/FEATURES.md](docs/FEATURES.md) · [docs/REALIZATION_PLAN.md](docs/REALIZATION_PLAN.md) |
| The Home Assistant side specifically | [docs/integrations/home-assistant.md](docs/integrations/home-assistant.md) |
| What changed between versions | [CHANGELOG.md](CHANGELOG.md) |

[docs/legacy/](docs/legacy/) holds documents kept for history and not
maintained — the Phase 1 inventory of the service this replaced, and
the AFK queue that closed empty.

## HTTP surface

| Method | Path | What it is |
|---|---|---|
| `POST` | `/v1/ingest/{source_id}` | Accept a payload, journal it durably, answer 202 |
| `POST` | `/v1/ingest/{source_id}/sync` | The same, but wait for delivery and return the Google event id |
| `DELETE` | `/v1/ingest/{source_id}/events/{external_id}` | Remove an event this source created, addressed by the id the source itself used |
| `GET` | `/healthz` | Liveness, no authentication — this is what Uptime Kuma watches |
| `GET` | `/metrics` | Prometheus counters, no authentication — monitoring that cannot log in reports a healthy service as down |
| `GET` | `/v1/debug/status` | Profiles, journal depth and recent routing decisions |
| `GET` | `/v1/debug/capture` | Recently captured requests, verbatim |
| `GET` | `/` | Operator UI (the kit's): status; `/sources` profiles and calendars, `/clients` tokens, `/captures` captures |

Ingest endpoints authenticate with that source's own bearer token. The
debug endpoints and the dashboard use the operator's credential, and
refuse every request when none is configured — an unconfigured admin
surface closes rather than opens. `/healthz` and `/metrics` are open
deliberately, and both are numbers only: no token, calendar id or
payload content appears in either, which is asserted by a test rather
than intended.

## Configuration

Secrets come from [Latch](https://github.com/kennypassenier/latch) and
are injected straight into the process, never written to disk:

```bash
latch run -- ./target/release/almanac
```

Everything else is environment variables and mapping profiles. See
[.env.example](.env.example) for the complete contract — it is the
real list, checked against the code, rather than whatever a secrets
manager happened to hold.

A mapping profile is a small TOML file per source, naming the target
calendar and how to read that source's payload. There are working
examples in [fixtures/profiles](fixtures/profiles), which are also what
the regression tests pin.

## Development

```bash
cargo test --all          # the whole suite
./.claude/hooks/gates.sh   # what a commit has to pass: fmt, clippy -D warnings, tests, boundaries
```

The code splits into `src/core` (pure logic, no I/O) and `src/shell`
(HTTP, files, Google). That boundary is enforced by a gate rather than
by convention, because a single crate gives the compiler no way to
enforce it.

Releases are cut and signed locally, never in CI — see the runbook.

## Deployment

systemd on its own LXC ([deploy/almanac.service](deploy/almanac.service)),
because self-update replaces the running binary and a container would
silently discard that on the next recreation. A compose file
([deploy/docker-compose.yml](deploy/docker-compose.yml)) ships alongside
it for the future homelab-v2 migration.

The binary works out which of the two it is in: inside a Docker or
Podman image it switches self-update off and says so, leaving updates to
whatever builds the image. LXC is deliberately not treated as an image —
that is a long-lived machine, not a rebuilt artifact, and it is where
self-update is meant to run.

Installation, first run and recovery are in
[docs/OPERATIONS_RUNBOOK.md](docs/OPERATIONS_RUNBOOK.md).

## License

MIT — see [LICENSE](LICENSE).
