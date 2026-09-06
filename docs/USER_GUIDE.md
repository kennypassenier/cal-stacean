# Almanac — user guide

How to make Almanac do things: connect a source, shape what it writes
onto a calendar, correct it when a source changes its mind, and take it
away again.

This is the "how do I" document. What went wrong and how to find out is
[DEBUGGING_GUIDE.md](DEBUGGING_GUIDE.md); how the machine is run is
[OPERATIONS_RUNBOOK.md](OPERATIONS_RUNBOOK.md).

Feature IDs (K5, M9, …) are in the margins so a claim here can be traced
to [FEATURES.md](FEATURES.md) and to the test that keeps it true.

---

## 1 · The shape of the thing

Something posts a JSON payload to Almanac — Almanac's own event shape.
Almanac looks up that source's **profile**, which says which calendar to
write to, and puts the event there.

```
Home Assistant ─┐
Grafana ────────┼─→ Almanac ─→ Google Calendar (several calendars)
Uptime Kuma ────┤
a Claude session┘
```

Three things follow from that, and they are the whole design:

**A source never holds Google's credentials.** Almanac does, and
nothing else in the homelab does (K12). A source holds only its own
bearer token, which grants exactly one thing: posting as that source.

**Almanac answers before Google does.** A payload is written to a
durable journal and flushed to disk *before* the 202 comes back (AR16),
and a background worker delivers it. A source that got a 202 can forget
about it — a power cut between the 202 and the calendar costs nothing,
the entry goes out on the next start.

**Sending the same thing twice is safe.** Events are matched by an id
the source chose, stored as a private property on the Google event, so
a redelivery updates the existing event instead of making a second one
(K2).

---

## 2 · Connecting a new source

### 2.1 · Find out what it actually sends (K13)

Do not guess the payload shape from documentation. Issue the new source a
client token on the Sources page (`/clients`, any name, e.g.
`my-new-thing`), point the source at Almanac with that token — any path
will do, `POST /v1/ping` answers 200 to any client — and open the
source's row on the Sources page: **Last requests** shows exactly what
arrived, method, path, headers (credentials masked) and body (cut at the
kit's limit), for the kit's capture TTL. The **Send test** button on that
row posts one ping with the token, so "does my token work?" is one click.

```bash
# On the source's side, temporarily send to this instead:
POST http://10.10.10.12:8080/v1/ping
Authorization: Bearer <client token>
```

Almanac's own capture endpoint and Captures page (3.x–4.0.0, M11) are
gone since 4.0.1: the kit keeps the same evidence on the row of the
source that sent it.

Captures live in memory, are capped, and expire. They are for an
afternoon of reverse-engineering, not a log.

### 2.2 · Add the source, and send it events

**Adding it takes two fields.** On `/sources`, **Add a
source** asks for a name and a calendar (K21). The calendar is a
dropdown of the ones that exist; choose *+ New calendar…* and a box
appears for its name. Submitting writes the profile, creates the
calendar if it is a new one and shares it with you, and lists the source
ready for a token. Live immediately, no restart.

The calendar comes from a dropdown of the ones that exist. To make a
new one, use the **Calendars** panel below the source list: it lists
every calendar with the sources that write to it, and offers a delete
for the ones nothing writes to — deleting a calendar removes every event
on it, for everyone it is shared with, which is why the button stays
dead until you have deleted the sources first.

**The profile it writes says only where events land.** Since 2.0.0 that
is all a profile is:

```toml
schema_version = 2
source_id = "kobo"
target_calendar_id = "2774a1…@group.calendar.google.com"
timezone = "Europe/Brussels"
default_duration_minutes = 60
```

| Key | Required | What it does |
|---|---|---|
| `schema_version` | yes | Always `2`. A v1 profile is refused with a message saying what changed, not misread. |
| `source_id` | yes | The URL segment this source posts to, and the name of its token. Unique across all profiles (AR15). |
| `target_calendar_id` | yes | Which calendar this source's events land on (K3). |
| `timezone` | no | The zone timestamps are read in when a call does not say. Defaults to `Europe/Brussels`; checked when the profile loads (M4). |
| `default_duration_minutes` | no | How long events last when a call gives neither an end nor a duration. Defaults to 60. |

**Everything else is in the call.** What an event *is* — its title,
length, colour, whether it is all-day — is something the source knows
per event, so the source says it per event:

| Field | | |
|---|---|---|
| `title` | required | the event title |
| `start` | required | RFC 3339, or `2026-09-07` when `all_day` |
| `external_id` | required¹ | this source's own id for the thing |
| `description` | optional | the body |
| `location` | optional | where it is |
| `end` | optional | RFC 3339. Wins over `duration_minutes` |
| `duration_minutes` | optional | how long, when there is no end |
| `all_day` | optional | `true` makes a day marker instead of a block |
| `duration_days` | optional | how many days an all-day event covers; 1 |
| `busy` | optional | `false` shows it without consuming availability |
| `color` | optional | a Google colour by name (`tomato`) or id (`11`) |
| `status` | optional | `confirmed`, `tentative` or `cancelled` |
| `reminders` | optional | see below |
| `timezone` | optional | overrides the profile's, for this event |

¹ `external_id` may be left out **only** if the call carries an
`Idempotency-Key` header instead (M7). A call with neither is refused
with a 422: without one of the two, Almanac writes no marker on the
event and can never find it again — every resend makes another, and
delete answers 404. That is not a hypothetical; it happened on
2026-09-03, before this was refused at the door.

```json
POST /v1/ingest/kobo
{"title": "Week weg", "start": "2026-09-07", "all_day": true,
 "duration_days": 5, "color": "tomato", "busy": false,
 "external_id": "kobo/holiday/2026-09"}
```

**Unknown fields are refused, not ignored.** A call sending `allDay`
instead of `all_day` gets a message naming it, rather than a timed event
and no explanation.

**Reminders**, when you want them:

```json
"reminders": {"popup_minutes_before": [30], "email_minutes_before": [1440]}
```

```json
"reminders": {"silent": true}
```

Omitting the block inherits the calendar's own default, which is a third
and different outcome from silence. Google allows at most five
reminders, none further out than four weeks; both are checked before the
event is sent.

**A source that speaks a different shape.** Almanac used to translate:
a profile named which payload field meant the title, and Grafana's
`commonLabels.alertname` became one. That translation is gone as of
2.0.0 — Kenny's decision, and his reason: *"voor aanpassingen hadden we
HTTPSwitchboard!"* A webhook that cannot change what it sends goes
through HTTPSwitchboard, which exists to translate message shapes, and
Almanac stays one thing.

### 2.3 · Check the profile before connecting anything (M9)

The dry-run endpoint shows exactly what a payload would become,
**without writing to Google**:

```bash
curl -s -X POST \
  -H "Authorization: Bearer $ALMANAC_TOKEN" \
  -H "content-type: application/json" \
  -d '{"title":"Bin day","entity_id":"sensor.waste","start":"2026-09-01T07:00:00Z"}' \
  http://10.10.10.12:8080/v1/debug/dry-run/home-assistant | jq .
```

If a required field is missing, the answer says which field, in which
profile — not "mapping failed".

### 2.4 · Issue the source its token (K6, M12)

On the **Sources** page the kit provides (`/clients` in the navigation,
next to Almanac's own Sources page): **Issue token** with the source's
name gives it one. *Copy command* puts a working `curl` on your
clipboard without showing the token, *Reveal* shows it for ten seconds.
Paste it into the source's configuration. Tokens are stored sealed with
`ALMANAC_SECRET_KEY`; the file on disk never contains the plaintext.

Each source's token opens only its own endpoint. One source's token
posting as another is rejected exactly like a wrong token — and so is
an unknown source id, so probing cannot tell "no such source" from
"wrong token".

Revoking is immediate: the very next request with that token fails.

### 2.5 · Point the source at Almanac

```
POST http://10.10.10.12:8080/v1/ingest/home-assistant
Authorization: Bearer <that source's token>
Content-Type: application/json

{"title": "Bin day", "entity_id": "sensor.waste",
 "start": "2026-09-01T07:00:00Z"}
```

```json
202 Accepted
{"status": "accepted", "entry_id": "01J…"}
```

202, not 200, and deliberately: it means *"this is safely written down
and will happen"*, not *"this is on your calendar"*. The event appears a
moment later.

For Home Assistant specifically, including a `rest_command` that retries
properly, see [integrations/home-assistant.md](integrations/home-assistant.md).

---

## 3 · Changing and removing events

### 3.1 · Updating an event (K2)

Post again with the same `external_id_field` value. Almanac finds the
existing event by the private property it stored and replaces it.

```bash
# same entity_id, new time → the same event moves
{"title": "Bin day", "entity_id": "sensor.waste",
 "start": "2026-09-02T07:00:00Z"}
```

There is no separate update call, and there is no risk in sending the
same thing twice — which is the point, because a source retrying after
a timeout has no idea whether the first attempt landed.

**Without `external_id_field`**, every post creates a new event. If the
source has no natural id, send an `Idempotency-Key` header instead (M7)
and Almanac uses that as the key:

```
Idempotency-Key: shopping-run-2026-09-01
```

The profile's own `external_id_field` wins when both are present.

### 3.2 · Deleting a source (K21)

*Delete* on `/sources` removes a source entirely: its token (the kit
client under its name) and its profile file are both gone. It stops posting immediately,
no restart, and it is off the page.

Refused while that source still has events waiting in the journal, with
the count in the message: deliveries resolve their calendar through the
profile, so deleting first would strand them.

**Events already on the calendar are not touched.** They belong to the
calendar now (Kenny's decision, 2026-09-03: deleting a source says
something about the source, not about what already happened). To remove
them, delete them by external id (3.3) *before* deleting the source,
while its token still works.

### 3.3 · Deleting an event (K8)

Address it by the id the source itself used:

```bash
curl -X DELETE \
  -H "Authorization: Bearer <that source's token>" \
  http://10.10.10.12:8080/v1/ingest/home-assistant/events/sensor.waste
```

```json
{"status": "deleted"}
```

Deleting something that is not there answers `not_found` rather than
pretending to have done something. A source can only delete events it
created — one source's token cannot delete another's event, even
knowing the id.

### 3.4 · When you need the event id back (K8)

The ordinary endpoint answers 202 and does not wait. When the caller
genuinely needs to know it landed — a Claude session that wants to
report back — there is a synchronous variant:

```bash
POST /v1/ingest/{source_id}/sync
```

```json
200 OK
{"status": "delivered", "event_id": "abc123…", "created": true}
```

`created: false` means it updated an existing event. If Google is
unreachable, this answers 502 **and keeps the payload** — it is still
journalled and still goes out later. A 502 here means "not yet", never
"lost".

**502 and 422 mean opposite things, and that is the whole point.**

| Answer | What happened | What to do |
|---|---|---|
| `502` | Google hiccuped. The payload is journalled and the worker keeps trying. | Nothing. Waiting is the fix. |
| `422` | Almanac cannot turn this body into an event, and never will. Nothing was stored. | Send something else. `remedy` says what. |

They used to be the same code, which meant a caller could not tell
"almanac is retrying" from "retrying will never help" — and the second
sentence was being shown to people as the first. Both codes carry
`message` and `remedy`, so neither field distinguishes them; the status
code does.

The 422 is checked before anything is journalled, on the asynchronous
endpoint too. A misspelled field (`allDay` for `all_day`) is named
straight away rather than accepted with a 202 and failing much later in
the dead letter.

---

## 4 · Several calendars (K3)

Each profile names its own `target_calendar_id`, so the split is
whatever you want it to be. The live deployment uses two:

| Calendar | Sources |
|---|---|
| Almanac · Huishouden | home-assistant |
| Almanac · Infra | grafana, uptime-kuma |

Almanac creates its own calendars rather than needing you to make them
in Google's UI. `examples/create_calendars.rs` creates whatever is
missing and shares each one with a real person — sharing is not
optional and not a separate step, because a calendar the service
account creates is owned by the service account and invisible to
everyone else until it is shared. That mistake has been made here twice
and the tool now re-checks every calendar on every run.

Real calendar ids live in the deployment's profiles and deliberately
not in this repository — they are the household's, not the code's.

---

## 5 · Watching it work

| Where | What it tells you |
|---|---|
| `/` | The kit's status page: journal, sources, health, updates. `/sources` for profiles and calendars, `/clients` (Sources) for tokens and each source's last requests. The one place to look first. |
| `/v1/debug/status` (K11) | Which profiles are loaded, how deep the journal is, and how recent events were routed. |
| `/metrics` (M13) | Counters for Prometheus: accepted, delivered, failed, set aside, token refreshes, journal depth. |
| `/healthz` (M1) | Liveness only. Answers 200 while Google is down, on purpose — Almanac riding out an outage is working correctly, and a health check that goes red would be lying. |

`/healthz` and `/metrics` need no token, because monitoring that cannot
authenticate reports a healthy service as down. Everything else does.

---

## 6 · What Almanac will not do

- **It does not read calendars back to you.** It writes; Google Calendar
  is the reader.
- **It does not schedule anything itself.** No cron, no timers, no
  "remind me in an hour". A source decides when something happens;
  Almanac writes it down.
- **It does not merge or deduplicate across sources.** Two sources
  reporting the same event produce two events, on whichever calendars
  their profiles name.
- **It does not rate-limit inbound requests** (M5, deliberately Later).
  Every source is on the LAN and trusted; this would matter the day one
  is not.
- **It does not translate payload shapes** (2.0.0, Kenny's decision). A
  source speaks Almanac's event shape. Anything that cannot — a webhook
  from a system nobody here controls — goes through HTTPSwitchboard,
  which exists for exactly that.
- **It does not do repeating events** (declined 2026-08-29). Not for
  lack of value — a recurring event is one Google event with instances
  beneath it, and Almanac's whole model is one payload, one event. An
  update from a source would have to choose between rewriting the
  series and rewriting one occurrence, and half-answering that is how a
  source silently overwrites a whole series. Post each occurrence
  instead: a week ahead every Monday works today.
- **It does not invite anyone** (declined 2026-08-29). Adding guests
  means Almanac sends mail to people, and a profile mistake stops being
  a wrong calendar entry and becomes an invitation to the wrong person.
  Share the calendar instead.
- **It does not retry forever.** An entry that fails permanently three
  times is set aside as dead, kept in the journal with its reason, and
  reported — rather than blocking everything behind it (T1).
