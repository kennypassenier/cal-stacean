# Almanac — architecture reference

The system as built, in September 2026. This describes what is there;
[ARCHITECTURE_DECISIONS.md](ARCHITECTURE_DECISIONS.md) records *why*
each choice was made and which objection forced it.

---

## 1 · The shape

```
        HTTP in                    Almanac                     out
   ┌──────────────┐      ┌────────────────────────┐    ┌──────────────┐
   │ Home Asst    │      │  ingest  → journal     │    │              │
   │ Grafana      ├─────→│            (fsync)     │    │    Google    │
   │ Uptime Kuma  │ 202  │              ↓         ├───→│   Calendar   │
   │ Claude       │      │  worker  → delivery    │    │              │
   └──────────────┘      │            (upsert)    │    └──────────────┘
                         │                        │
   Prometheus ←─ /metrics│  dashboard · admin     │───→ Home Assistant
   Uptime Kuma ←─/healthz└────────────────────────┘     (notifications)
```

One process, one port, one data directory. No database, no message
broker, no sidecar. The durability that would normally come from a
queue comes from an append-only file that is fsynced before the request
is answered.

## 2 · Accepting and delivering

The two halves are deliberately separated, and the seam between them is
the journal.

**Accept** (`shell::ingest`) authenticates the source, maps the payload
with that source's profile, appends the entry to the journal, **fsyncs
it**, and only then answers 202. The whole point of that order: a
source that received a 202 may forget the payload forever, so the 202
must not be able to outrun the disk.

**Deliver** (`shell::worker`) runs a loop that reads pending entries and
sends them to Google. It marks each done in the journal *after* Google
confirms. That ordering means a crash between the two is possible and
harmless: on the next start the entry replays, and the upsert converges
on the same event instead of making a second one (AR16).

The loop paces itself (`core::pacing`): a fast interval while things
are working, a backoff ladder during an outage, and back to fast on the
first success. It reports a backlog once per outage rather than once
per pass, because an alert that fires every ten seconds is an alert
nobody reads.

An entry that fails **permanently** three times is set aside as dead —
kept in the journal with its reason, visible on the debug surface, and
reported once — so one undeliverable payload cannot hold up everything
behind it (T1). Transient failures are retried indefinitely; the
difference between the two is decided in `core::retry`, which reads
Google's own reason strings (a 403 for "rate limit" is transient, a 403
for "permission denied" is not).

## 3 · The core/shell split (AR13)

`src/core` is pure: no HTTP client, no filesystem, no clock. `src/shell`
is everything that touches the world.

| `core` | what it decides |
|---|---|
| `mapping` | payload + profile → an event |
| `upsert` | existing events + key → create or update |
| `retry` | an HTTP status and body → transient or permanent |
| `journal` | records → what is pending |
| `pacing` | a pass's results → the next interval, and whether to warn |
| `update` | versions and state → install, revert, or leave alone; and whether self-update should run here at all |
| `metrics` | counters → the Prometheus exposition format |
| `profile`, `token`, `secrets`, `html`, `observability` | validation, hashing, sealing, escaping, ring buffers |

| `shell` | what it does about it |
|---|---|
| `ingest`, `admin`, `dashboard` | the HTTP surfaces |
| `journal`, `durability`, `datadir` | the append-only log, fsync, the exclusive lock |
| `worker`, `delivery` | the delivery loop and one delivery |
| `calendar_client`, `auth` | Google, and the OAuth2 token |
| `token_store`, `notify`, `profiles`, `update` | encrypted tokens, Home Assistant, profile loading, self-update |

The boundary is enforced by a grep in the commit gate rather than by
convention, because a single crate gives the compiler no way to enforce
it. It exists so the decisions above can be tested exhaustively without
a server, a temp directory, or a network — which is why `core` carries
the majority of the tests.

Its one blind spot is worth stating: a decision that is *correct in
`core` but never reached from `shell`* passes every test. That is
exactly how the self-update interval bug survived nine green tests, and
why the loop itself now has a test on a paused clock.

## 4 · Identity and secrets

**Sources** authenticate with their own bearer token. Tokens are stored
sealed with XChaCha20-Poly1305 in a file that never contains the
plaintext, and compared by SHA-256 in constant time. Revocation takes
effect on the very next request.

An unknown source id and a wrong token produce the identical 401, so
probing cannot enumerate which sources exist.

**The operator** authenticates with a bootstrap token, exchanging it for
a session cookie. An admin surface with no token configured refuses
everything rather than opening up — the failure mode of a missing
config is closed, not open.

**Google's credentials** live in Latch and are injected into the process
environment at startup (AR8); they never reach disk, argv, or a log.
That is asserted by tests, not just intended: `tests/no_secrets_in_logs.rs`
runs the real binary and greps its output.

**Almanac is the only thing in the homelab that holds Google
credentials.** Every other system holds a token that can post as itself
and nothing more. That is the security argument for the whole hub
existing.

## 5 · Self-update (M10)

```
every 6h, and 5 min after each start
   ↓
fetch latest/download/VERSION           plain file, no API, no token
   ↓  newer?
fetch SHA256SUMS + SHA256SUMS.minisig
   ↓  minisign signature valid?         ← the only real trust anchor
fetch the binary, check it against the manifest hash
   ↓
run it once with --check                does it start on this machine?
   ↓
rename old → almanac.prev, new → almanac, restart
   ↓
serving for 60s?  → clear the probation
not serving?      → put almanac.prev back, and say so
```

The signature is what matters. A checksum served from the same host as
the binary proves nothing about the binary; the manifest is signed
offline, by hand, on Kenny's machine, and CI never holds the key.

**It refuses to run where it would be pointless or harmful.** Inside a
Docker or Podman image it switches itself off, because a binary
replaced in a container is discarded on the next recreation while
looking identical to the image (AR20). LXC deliberately does not count
as an image — the live deployment is an LXC container and self-update
is exactly what is wanted there.

## 6 · Observability

| Surface | Auth | For |
|---|---|---|
| `/healthz` | none | Uptime Kuma. Liveness only — it stays 200 through a Google outage, because riding one out is correct behaviour |
| `/metrics` | none | Prometheus. Six counters, journal depth, and a version label. No source labels, deliberately: a source id is one careless profile away from writing a household detail into a metrics database that keeps it for years |
| `/v1/debug/status` | operator | Profiles, journal depth, recent routing decisions |
| `/v1/ping` | any client token | "Does my token work?"; the request lands on that source's row (the kit's K13 captures, credentials masked before storage) |
| `/dashboard` | session | The human view |
| Home Assistant webhook | — | Five notifications, outbound, for things that need a person |

The two unauthenticated endpoints are unauthenticated on purpose:
monitoring that cannot authenticate reports a healthy service as down,
and a false outage costs more than these numbers are worth. On the
public route `/metrics` is blocked at the edge anyway — that trade is
right on a private network and wrong on the internet.

## 7 · Deployment as built

| | |
|---|---|
| Host | CT 112 on Proxmox, an LXC container, `10.10.10.12:8080` |
| Supervision | systemd, running the binary under `latch run` |
| Data | `/opt/almanac/data` — journal, sealed tokens, update state |
| Profiles | `/opt/almanac/profiles/*.toml`, holding the real calendar ids |
| Secrets | a ciphertext-only Latch clone; no credentials on the machine to pull with |
| Updates | itself, from signed GitHub releases |
| External | `almanac.kp-soft.dev` via Cloudflare tunnel and Traefik, behind Cloudflare Access; `/metrics` blocked at the edge |
| Metrics | scraped by Prometheus on CT 113 |
| Sources | all on the LAN, posting to `10.10.10.12:8080` directly |

The unit is hardened (`ProtectSystem=strict` with `/opt/almanac`
writable — it has to be, since self-update replaces the binary there)
and has its restart limit disabled, so a service that cannot reach
Google at boot keeps trying instead of parking itself in `failed` with
nobody watching.

## 8 · What is not here, and why

**No database.** The journal is an append-only JSONL file that is
compacted when it grows. For this volume a database would add an
operational dependency and buy nothing.

**No queue.** The journal *is* the queue, and it survives power loss
because of when it is fsynced rather than because of a broker.

**No multi-instance support.** The data directory takes an exclusive
lock and a second process refuses to start. Two processes on one
journal is corruption, and the refusal is the feature.

**No inbound rate limiting** (M5, Later). Every source is on the LAN.

**No reading calendars back.** Almanac writes; Google Calendar reads.

**No recurring events, and no invitations** (both declined by
mini-round, 2026-08-29). Recurrence is not a missing field but a
missing answer: a recurring event is one Google event with instances
beneath it, and Almanac's one-payload-one-event model — which K2's
upsert rests on — has no position on whether an update rewrites the
series or one occurrence. Invitations are declined because they would
make a profile mistake into mail sent to the wrong person, which cannot
be taken back.

Almanac uses eleven of Google's event fields as of K14–K17: summary,
description, location, colour, start, end, transparency, status,
reminders, extended properties, and the id.
