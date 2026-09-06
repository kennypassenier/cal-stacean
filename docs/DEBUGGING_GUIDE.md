# Almanac — debugging guide

Something is not on the calendar, or something is on it twice, or a
notification arrived. This document is how to find out why, in the order
that finds it fastest.

Running the machine — installing, releasing, restoring — is
[OPERATIONS_RUNBOOK.md](OPERATIONS_RUNBOOK.md). Making it do things is
[USER_GUIDE.md](USER_GUIDE.md).

---

## 1 · The evidence trail

Almanac is built so that every event leaves a trail in four places. Walk
them in this order; each one narrows the problem to one side of a
boundary.

| # | Look at | Question it answers | How |
|---|---|---|---|
| 1 | `/healthz` | Is the process alive at all? | `curl -s 10.10.10.12:8080/healthz` |
| 2 | `/v1/debug/status` | Did Almanac accept it, and where did it route it? | with the operator token |
| 3 | the journal | Is it waiting, delivered, or set aside? | the `pending` count in the same status |
| 4 | `journalctl -u almanac` | What did Google say? | on the LXC |

**The single most useful fact:** a payload that got a `202` is in the
journal, and an entry in the journal is never dropped. So the question
is never "was it lost" — it is only ever "where did it stop". Step 2
tells you whether the problem is before Almanac or after it, and that
halves the search immediately.

```bash
# Step 2, in full
curl -s -H "Authorization: Bearer $ALMANAC_TOKEN" \
  http://10.10.10.12:8080/v1/debug/status | jq .
```

That answers with the profiles it has loaded, the journal depth and its
oldest waiting entry, and the recent routing decisions — which source,
which profile, which calendar, and whether it worked.

---

## 2 · Nothing appeared on the calendar

| Symptom | Most likely cause | How to confirm | Fix |
|---|---|---|---|
| The source got **401** | Wrong token, or the source id in the URL does not exist | Both answer the same 401 on purpose, so check the URL first — a typo in `source_id` looks exactly like a bad token | Compare the URL against a loaded profile in `/v1/debug/status`; reissue the token from `/dashboard/sources` |
| The source got **202**, nothing on the calendar, `pending` is climbing | Google is unreachable or refusing | `journalctl -u almanac \| grep "delivery failed"` | Read the error's remedy — every error carries one. Nothing is lost; it retries |
| The source got **202**, `pending` is 0, still nothing | It went to a **different calendar** than you are looking at | `/v1/debug/status` shows the target calendar id per profile | Compare against the calendar you have open. This is the most common false alarm |
| The source got **422** | The payload does not match the profile | The response names the missing field and the profile | Fix the profile or the payload; check with dry-run first (§5) |
| The source got **502** from `/sync` | Google was unreachable *at that moment* | The payload is still journalled | Nothing to do. It goes out on the next pass — 502 here means "not yet", never "lost" |
| The source got **500** | The journal could not be written | `journalctl` names the path and the reason | Usually disk or permissions on `/opt/almanac/data` |
| Nothing at all, no request logged | It never arrived | Capture surface (§5) | The problem is on the source's side, not here |

**A calendar you cannot see is not an empty calendar.** A calendar the
service account creates is owned by the service account and invisible
to everyone else until it is shared. If a brand-new calendar looks
empty, first check that you are actually looking at *that* calendar and
not at a same-named older one.

---

## 3 · The same thing appeared twice

Almost always one of three things:

**The profile has no `external_id_field`.** Without it, every post
creates a new event — there is no key to match on. Add it, or have the
source send an `Idempotency-Key` header.

**The source changed its id.** The upsert matches on the value of
`external_id_field`. If the source used `sensor.waste` yesterday and
`sensor.waste_collection` today, those are two different things as far
as Almanac can tell, and it is right to make two events.

**Two sources are reporting the same thing.** Almanac does not merge
across sources, by design. Two profiles pointing at the same calendar
will both write.

To confirm which: `/v1/debug/status` shows the recent routing
decisions, including whether each delivery created or updated.

---

## 4 · A notification arrived

Almanac sends these to Home Assistant. Each is also a runbook section.

| `op` | What happened | Urgency |
|---|---|---|
| `almanac-update` | It installed a new version and restarted into it | None — this is it working |
| `almanac-update-reverted` | A new version came up and did not become healthy, so the old one is back | Look today. Runbook R4 |
| `almanac-update-unverified` | A published release failed signature or checksum verification, three times | Look now. Either the release is broken or the release host is not to be trusted. Runbook R5 |
| `almanac-entry-set-aside` | One event failed permanently three times and was given up on | Look today. It is kept in the journal with its reason and shown on the debug surface |
| `almanac-journal-backlog` | The journal is over half its size cap | Look today. Deliveries have been failing long enough to build up. Runbook R8 |

`almanac-entry-set-aside` is the one worth understanding: Almanac
retries transient failures forever, but a *permanent* failure — a
calendar that no longer exists, a payload Google rejects outright —
would otherwise block everything behind it. After three permanent
failures the entry is set aside with its reason, and the queue moves on.

---

## 5 · Tools for when the tables did not help

### Dry-run: what would this payload become? (M9)

No writing, no Google, just the answer:

```bash
curl -s -X POST -H "Authorization: Bearer $ALMANAC_TOKEN" \
  -H "content-type: application/json" -d @payload.json \
  http://10.10.10.12:8080/v1/debug/dry-run/home-assistant | jq .
```

Use this before blaming Google. It separates "the mapping is wrong"
from "the delivery failed" in one call.

### Capture: what is the source actually sending? (K13)

Open the source's row on the Sources page (`/clients`): **Last requests**
holds what that token sent — method, path, headers with credentials
masked, body cut at the kit's limit — for the kit's capture TTL. Point a
source you are still investigating at `POST /v1/ping` with its client
token, or press **Send test** on the row. Almanac's own capture endpoint
went in 4.0.1; the kit's masking is the same rule: an `Authorization`
header is replaced before it is stored.

### Metrics: is this a blip or a pattern? (M13)

`almanac_deliveries_failed_total` climbing while
`almanac_events_delivered_total` also climbs is a retry story, not an
outage. Only sustained failures with no deliveries are a problem.

`almanac_journal_readable 0` means the scrape could not read the journal
at all — and the depth gauge is then absent rather than zero, so an
alert written against "pending == 0" will not quietly hide it.

### Ask Google, not Almanac

When the question is "did it really land, and does it look right", read
the calendar back rather than trusting Almanac's own log:

```bash
ALMANAC_SHOW_CALENDAR=<calendar id> latch run -- cargo run --example show_events
```

It prints each event's summary, start, end, free/busy marker and the
private property Almanac matches on. Almanac's log says what it sent;
this says what Google stored, and those are different claims — an event
can be accepted and stored as something other than what was meant.

### The log

```bash
journalctl -u almanac -f                       # follow
journalctl -u almanac | grep "delivery failed" # what Google said
journalctl -u almanac | grep "checked for a new release" | tail -3
```

Every error in the log carries its own remedy line. That is enforced by
a test — `every_variant_carries_its_remedy_through_display` — so an
error message without a suggested action is a bug, not a gap in this
document.

---

## 6 · Things that look broken and are not

**`/healthz` says ok while Google is down.** On purpose. Almanac riding
out a Google outage via the journal is Almanac working correctly.
Reporting itself unhealthy would make Uptime Kuma page you for
something that needs no action.

**Almanac is quiet.** Since 2.1.0 it writes one `alive` line per hour
even when nothing is happening:

```
INFO almanac::shell::heartbeat: alive accepted=3 delivered=3 failed=0
     dead=0 pending=0 sources=1 uptime_secs=7204
```

No `alive` line for more than an hour means the process is not turning,
which is different from "no events" — `accepted=0` on a line that keeps
arriving is an idle hub working correctly. `pending=-1` means the
journal could not be read at all, reported as its own value rather than
as a zero.

`ALMANAC_HEARTBEAT_INTERVAL_SECS=0` switches the line off; if it is
absent when you expect it, check that first.

**The version number has not moved in six hours.** That is the check
interval. Look for `checked for a new release` in the log — it appears
either way, so a silent updater and a working one are distinguishable.
If that line is absent since the last start plus five minutes, the
updater really is not running.

**Self-update does nothing in a docker container.** By design (AR20) —
a binary replaced inside a container is discarded when the container is
recreated, while looking identical to the image it came from. The log
says so on startup. LXC is not treated as an image, so the live
deployment does update itself.

**A setting looks unset when you check the unit.** Almanac's secrets and
settings do not live in the unit's `EnvironmentFile`, and they are not
in the environment of the process systemd started either — that process
is `latch run`. They are injected into its **child**, which is where
almanac actually runs. So:

```bash
# Both of these say the variable is missing, and both are wrong:
grep ALMANAC_CALENDAR_OWNER /appdata/almanac/almanac-config/latch.env
tr '\0' '\n' < /proc/$(systemctl show -p MainPID --value almanac)/environ

# Ask the child, or ask latch directly from a linked checkout:
latch run --env dev -- sh -c 'echo $ALMANAC_CALENDAR_OWNER'
```

`latch.env` holds only `LATCH_KEY_ALMANAC` and `ALMANAC_UPDATE_MODE` (3.0.0; the 2.x `ALMANAC_SELF_UPDATE` still works with a warning);
everything else arrives from Latch at startup. Measured from both sides
on 2026-09-03 — this session via `latch run`, the homelab via the
child's `/proc/<pid>/environ` — after the homelab's first two checks
looked at the file and the MainPID and concluded the setting was gone.

**A second process refuses to start.** The data directory takes an
exclusive lock. Two processes sharing one journal is corruption; the
refusal is the feature. Runbook R10.

**`created: false` in a sync response.** That is the upsert working — it
found the existing event and updated it. `true` would mean a duplicate.
