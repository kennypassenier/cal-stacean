# Changelog

All notable changes to Almanac. Format loosely follows
[Keep a Changelog](https://keepachangelog.com/); versions follow
[semver](https://semver.org/).

Releases are signed. Every published release carries `SHA256SUMS` and
`SHA256SUMS.minisig`, signed offline with the key whose public half is
compiled into the binary — which is what the self-updater verifies
against before it installs anything.

## [Unreleased]

### Added

- **`almanac --version` / `-V`** prints the compiled version and exits,
  touching nothing else. Found missing by the Homelab Rust session,
  running the binary by hand outside `latch run` to sanity-check the
  2.4.0 deploy: every other special mode (`--check`, `update`) needs
  the full production configuration on purpose, because they answer
  "can this run here" — but "what version is this" got the same
  treatment by default, so asking started the whole process and
  complained about a missing webhook and an unreadable profiles
  directory instead of just answering.

  It answers about the file, not the process: under a supervised
  update `almanac update` replaces the binary before the homelab
  restarts the unit, so for that window `--version` and `/healthz` can
  correctly disagree — noted in `docs/OPERATIONS_RUNBOOK.md` R12b after
  the same session found it during 2.4.0's own rollout. `/healthz` is
  what answers "what is actually running"; `--version` is for the file.

### Changed

- **kp-themes v3.0.0 adopted** (K25). The framework-free modules are pure
  since this release — importing `theme-picker.js` no longer attaches
  itself, so the head now imports `attachThemePickers` explicitly and
  calls it, rather than loading `js/auto.js`: that one script also wires
  up datatables, comboboxes, date pickers and eight other components
  almanac's dashboard does not use, and vendoring all of it for one
  button would put ten files nobody reads behind the commit gate.

  The picker also groups its options into light and dark sections by
  default now (TH63). Grouping needs the split before the page paints,
  which is exactly the fact this project stopped hand-keeping in Rust
  after the K25 correction — so `dark_themes()` reads it out of the same
  vendored registry the picker's names are already checked against,
  rather than a second copy that could disagree with the first one.

  `theme-picker.js` gained a dependency on `js/strings.js` for its
  status text, so that file is now vendored too — six files behind the
  gate rather than five. Almanac's UI is English (standing rule 1),
  which is the package's own default since 3.0.0, so nothing calls
  `setStrings()`; the theme *names* stay Dutch (Formeel, Donker,
  Zonnewende, …) because README frames them as Kenny's names for his
  themes, not interface chrome, the same reason they were not English
  before the package's own default flipped.

  Fixed in passing: the picker's `aria-label` had been "Thema kiezen"
  since v1.0.0, in an otherwise English UI — a leftover from when the
  package's own default was Dutch too. It is "Choose a theme" now, and
  so are the new group headings ("Light" / "Dark").

- **kp-themes v1.0.0 adopted** (K25). Eleven themes instead of seven —
  `high-contrast`, `sepia`, `blueprint` and `solstice` are new — and the
  picker's behaviour now comes from the package rather than from a copy
  written here.

  The stale copy was invisible: the commit gate compared the one file
  almanac had vendored, that file was in step with itself, and the
  theme list living in Rust was outside what the gate looked at. So the
  gate now covers every vendored file, and a test checks almanac's list
  against the vendored registry in both directions — including on CI,
  where kp-themes is not on the machine.

  Each option's two hand-copied swatch colours are gone, 21 in total: a
  swatch wears the theme now (`<span class="kp-swatch" data-theme="…">`
  reads that theme's live tokens). Adjusting a palette upstream would
  have left almanac previewing colours the theme no longer had, and
  nothing would have failed.

  `static/theme-bootstrap.js` stays almanac's own: Bootstrap reads
  `data-bs-theme`, which the package knows nothing about. It listens to
  the package's `kp-theme-change` event rather than to the buttons, and
  keeps no list of which themes are dark — every theme declares its own
  `color-scheme`, so the browser is asked instead of told. The head does
  the same thing once, after the stylesheets and still before the first
  paint. The kyu session, which ported this an hour earlier, had made
  the list mistake before and said so; almanac's first attempt had the
  same list and it is gone.

  **The gate that guards the copies now has two severities**, which is
  kyu's observation about the version I first wrote. An edited copy is
  refused — a change made in a vendored file disappears at the next
  re-vendor and nothing else would report it — and that check runs off
  recorded checksums, so it works on CI where kp-themes is not present.
  A copy that has merely fallen behind only says so: taking a release is
  a decision with a moment of its own, and one project's release should
  not break another project's unrelated commits.

  `scripts/vendor-kp-themes.sh` does the update in one step and records
  what it took, so there is no file anyone has to keep in step by hand.
  It refuses to vendor a working copy that differs from its own tag.

  Thirty lines of picker styling left `theme-bridge.css` with it: the
  package styles its own picker now, and what remains in that file is
  the one thing it cannot do for us — pointing Bootstrap's variables at
  its tokens.

## [3.0.0] — 2026-09-05 (unreleased; branch `chassis-migration`)

Built on [chassis-rs](https://github.com/kennypassenier/chassis-rs) v1.2.0.
Ingest, the journal, delivery, the token store, the dashboard and the
Google client are unchanged; the kit now owns the command line, the
transport knobs, logging, `/healthz`, `/metrics`, readiness (`Type=notify`),
the graceful stop and — replacing 1 900 lines of Almanac's own — signed
self-update. The environment and `/healthz` change, hence 3.0.0.

### Migration

- **Environment.** `ALMANAC_BIND` → `ALMANAC_LISTEN` (alias with a warning);
  `ALMANAC_SELF_UPDATE` (on/off) → `ALMANAC_UPDATE_MODE`
  (`off`/`supervised`/`autonomous`; the alias maps on → autonomous, off →
  off, with a warning; **unset now means off** — 2.x checked and installed
  by default when a release key was compiled in); `ALMANAC_UPDATE_URL` now names the directory holding
  `VERSION`, `SHA256SUMS`, `SHA256SUMS.minisig` and the binary — a 2.x value
  ending in `/releases` is completed to `/releases/latest/download` with a
  warning; unset it and the kit derives it from the repository. `RUST_LOG`
  → `ALMANAC_LOG` (`ALMANAC_LOG_FORMAT=json` is new). **`ALMANAC_STATE_DIR`
  must point at an existing directory** (the kit probes it at `--check` and
  start; on CT 112 that is `/opt/almanac`, so nothing moves). The per-path
  overrides (`ALMANAC_PROFILES_DIR`, `_DATA_DIR`, `_JOURNAL`, `_TOKEN_STORE`)
  still work but are deprecated in favour of the one root (K20).
  Unchanged: `ALMANAC_SECRET_KEY`, `ALMANAC_BOOTSTRAP_TOKEN`,
  `ALMANAC_CAPTURE_TOKEN`, `ALMANAC_NOTIFY_WEBHOOK`,
  `ALMANAC_HEARTBEAT_INTERVAL_SECS`, `ALMANAC_CALENDAR_OWNER`, the Google
  credentials via `latch run --`.
- **Command line.** `--version`, `--check`, `update` keep their meaning
  (`--check` still prints `almanac <v> --check: ok` and leaves a running
  instance alone); new are `--help`, `--print-config`, `--healthcheck`,
  `gen-secret`, `rekey`. An unknown argument is refused with exit 1 (2.x
  ignored it).
- **`/healthz`** = `{"status","version","subsystems":{"journal":{"ok",
  "detail"}}}`; still unauthenticated, still ignorant of Google on purpose;
  **503 only when the journal cannot be read.** `/metrics` keeps every
  `almanac_*` series except `almanac_build_info`, which the kit now emits
  (same name, same label), and gains `almanac_uptime_seconds` and
  `almanac_http_requests_total`.
- **Startup order.** The listener binds first and `READY=1` follows; the
  Google authentication retry (AR21) runs after the bind, and the delivery
  worker starts once it succeeds — the journal accepts events meanwhile, as
  the design always promised. A permanently broken key still exits 1.
- **Under the kit's layers:** request ids, security headers with a strict
  CSP (`script-src 'self'`, `font-src 'self'`), an in-flight cap, a request
  timeout and a body limit (1 MiB, `ALMANAC_MAX_BODY_BYTES`). The dashboard's
  inline scripts moved to `/static/almanac-*.js`, the no-flash snippet is
  the kit's `theme-boot.js`, and the display fonts are served from the kit's
  vendored set — no CDN, works offline.
- **Self-update** is the kit's: releases must carry `almanac`, `SHA256SUMS`,
  `SHA256SUMS.minisig` with the trusted comment `kennypassenier/almanac
  v<version>` and `VERSION`; `scripts/sign-release.sh` writes them, the
  release workflow (`release.yml`, new) builds the binary and the image.
  **Existing releases (≤ 2.4.0) carry minisign's default comment and are
  refused** — the first kit-based version must be installed by hand (or by
  the homelab). Autonomous checks are deferred while captures are retained
  (AR25, via the kit's update gate); a rolled-back version is skipped until
  a newer one appears (kit CF-3). The notifications `almanac-update`,
  `-reverted` and `-unverified` still reach Home Assistant: the kit's
  `on_update_event` (1.3.0, asked for by this migration) hands every update
  event to Almanac's own notifier. AR24's "three failed verifications
  before notifying" is the kit's knob since 1.4.0
  (`ALMANAC_UPDATE_NOTIFY_AFTER_FAILURES`, default 3 — Almanac's value): a
  failing release host is reported once, on the third failed check, and
  once more when checks succeed again.
- **Deployment.** `Type=notify` unit with the kit's hardening and the latch
  wrapper (`deploy/almanac.service`), binary at `/opt/almanac/bin/almanac`,
  `deploy/service.yml` for the homelab stack with `update_cmd`, journald
  drop-in. `ExecStartPre=… --check` refuses a broken environment before
  the old binary is stopped.

## [2.3.0] — 2026-09-03

### Changed

- **A payload almanac can never map is refused at the door, with 422.**
  Kenny's decision after the JobTracker session measured what the two
  502s looked like from outside. Almanac answered 502 both when Google
  had hiccuped and when the body was unusable, so a caller could not
  tell "wait, almanac is retrying" from "retrying will never help" —
  JobTracker was showing "almanac will try again" for a date sent
  without `all_day`, a sentence nobody could disprove.

  No new field: HTTP already separates "your request is wrong" from "my
  upstream broke", and almanac was not using it. **502 now means Google
  and only Google. 422 means the source must send something else.**

  The same change closes the asynchronous half. A misspelled field used
  to get a reassuring 202 and surface much later in the dead letter,
  because the body was only parsed in the delivery worker. It is parsed
  at the door now, so a mistake is named while the sender can still act,
  and nothing that can only ever fail enters the journal.

  The cost, accepted: an unmappable payload is refused rather than
  stored. It was lost either way — this way the sender hears about it.

### Fixed

- **A second click could still make a second calendar** (K24). Found
  while filling in the correction form for 2.2.0's stale-list bug, in
  the field that asks where the same fault sits elsewhere — so by
  measurement rather than by noticing it again in use.

  "Make calendar" is find-or-create precisely so a double submit cannot
  produce two. But it looked for the existing calendar in Google's
  list, and that list lags a create by seconds, so both clicks found
  nothing and both created. CT 112's journal shows it happening at
  19:56 on 2026-09-03: two `deleted a calendar` lines where one
  calendar had been asked for.

  Almanac now remembers what it made — the mirror of what it already
  remembered deleting — and consults that before asking Google. The
  create path is serialized per calendar name, so two tabs cannot race
  either. The same memory puts a fresh calendar on the page before
  Google lists it: the absence misled as much as the stale presence
  did, and invited the second click.

### Changed

- **The Google stub can model an eventually-consistent list.** It
  answered instantly and consistently, and that assumption is what let
  both halves of this bug through 42 dashboard tests. `lag_new_calendars`
  holds a created calendar out of `calendarList` until `catch_up`, which
  is what Google does for the first seconds.

## [2.2.0] — 2026-09-03

### Fixed

- **A deleted calendar stayed on the page** (K24). Google's calendar
  list is eventually consistent: one deleted a second ago still comes
  back in the very next list call, so the page rendered straight after
  the delete showed the thing that had just been removed. Measured —
  Kenny deleted one and it stayed; asking Google minutes later showed it
  genuinely gone.

  Almanac knows what it deleted, so it says so rather than re-asking a
  source that has not caught up. The memory clears itself: an id is
  forgotten as soon as Google's own list stops carrying it, so it never
  grows and never outlives the truth.

### Added

- **Every destructive button asks first, and every slow one says it is
  working** (standing rule 31, added from this). Delete a source, a
  calendar or an unusable profile and a confirmation names what will
  happen — these sit in table rows beside each other, where the distance
  between "issue a token" and "delete the calendar and every event on
  it" is a few pixels. While the action runs the button disables itself,
  spins and says so.

  Driven by `data-confirm` and `data-busy` attributes rather than
  per-button code, so a new button gets both by declaring them instead
  of by somebody remembering to wire it up. A test asserts that every
  destructive form carries both.

### Changed

- **The "calendar created" log line now names the sharing** (K24):
  `shared_with` and `role=owner`, and it says "and shared it". Without
  that, "created and shared" and "created and visible to nobody" read
  identically from outside — and the second is the outcome that has gone
  wrong here twice. Observed by the homelab session while verifying the
  button on the live service: they could confirm the calendar was made
  and had no way to confirm it was shared without a person opening
  Google Calendar.

  A test asserts the grant against what the Google stub actually
  received, rather than against the sentence describing it.

## [2.1.0] — 2026-09-03

### Added

- **A heartbeat line** (M14). One INFO line per interval — the
  counters, the journal depth, how many sources are served and how long
  the process has been up — whether or not anything happened.

  Kenny saw "no data" on the Grafana dashboard. Almanac had written
  nothing for 48 hours and looked exactly like a dead service; it was
  simply idle, which no log could distinguish. It used to have a
  heartbeat by accident: the self-updater logged a line every six hours
  on purpose, and switching updates over to the homelab took that with
  it — correct on its own terms, and it removed the only recurring sign
  of life.

  `/metrics` answers "how many" and `/healthz` answers "does it
  respond"; neither answers "is the background work still turning",
  which is the failure almanac has actually had. Standing rule 23 asks
  for exactly this line.

  `ALMANAC_HEARTBEAT_INTERVAL_SECS` sets the interval, default 3600.
  `0` switches it off; anything unparseable falls back to the default
  rather than to silence, because a typo must not quietly disable the
  thing whose job is to report silence. The first line lands after one
  interval rather than at startup, with its own test — the same shape
  as the updater bug, where an unconsumed first tick both duplicated the
  startup lines and drifted by a whole period.

- **A calendars panel on the dashboard** (K24). Make a calendar by name,
  and see every calendar with the sources that write to it. Kenny, after
  using the add-a-source form against the live service and getting the
  sharing mail: *"We gaan de optie om een nieuwe kalender uit die
  dropdown halen. We gaan in de plaats een nieuw paneel maken waar we de
  kalenders kunnen beheren."*

  They are two jobs: adding a source is frequent and small, making and
  removing calendars is rarer and heavier. Mixing them put a destructive
  capability inside a form used casually.

  **Delete is disabled rather than hidden** until no source writes to
  that calendar — a dead button saying "2 source(s) still write here"
  tells someone the capability exists and what to do first, where a
  missing button tells them nothing. The endpoint repeats the check on
  arrival: the page is a snapshot, and a source can appear between the
  render and the click. Deleting a calendar removes every event on it,
  for everyone it is shared with, and the panel says so in those words.

  The source form's calendar field is now a plain dropdown of what
  exists.

- **The kp-themes palettes, with the shared picker** (K25). Seven
  themes — formal, light, dark, cyberpunk, pastel, terminal, topo — with
  a picker in the navbar, stored in `localStorage` under `theme` and
  applied as `data-theme` plus `.dark` on `<html>`: exactly the contract
  `@kp-soft/themes` defines, so a choice made here means the same thing
  in every other project of Kenny's.

  **Taken from kyu rather than written twice.** Two sessions were
  building the same behaviour-only port of the package's React switcher
  at the same moment — almanac has no npm, no build step and no React,
  and neither does kyu. `theme.js` and `theme-bridge.css` are kyu's
  files verbatim; `themes.css` is the package's own, vendored with its
  version in the header. A second implementation of a stored contract is
  how three projects end up disagreeing about what "theme" means.

  The seven themes live once, in Rust, and the script carries no list at
  all: it reads `data-theme` and `data-dark` off the rendered options.
  A test pins the contract — the key, the default, and the `.dark`
  toggle — because that is the cheapest guard against the drift the
  shared package exists to prevent.

  **The commit gate refuses a vendored copy that has drifted.** It
  compares `static/themes.css` against `~/Projects/kp-themes` whenever
  that repository is on the machine, and says out loud when it is not
  there rather than passing quietly. The risk a copy actually runs is
  not being wrong today but ageing silently, and that is the risk this
  covers — taken from kyu, which built and proved it first. Proven the
  same way here: `--radius` changed by a thousandth, the gate refused
  and printed both lines; reverted, green again.

  A contrast gate was considered and **not** built. kp-themes runs its
  own before tagging, so the copied file has already passed it; running
  it again answers an answered question and would only fire if someone
  edited the copy, which its header forbids and the drift gate catches.
  A gate covering the wrong risk costs more than none, because it feels
  like cover.

  Two additions to the shared bridge, found here and going back to kyu:
  Bootstrap's `.bg-*` utilities read `--bs-body-bg-rgb`, a
  comma-separated triple an `hsl()` token cannot produce, so they keep
  Bootstrap's own colour unless pointed at the tokens directly; and the
  navbar's link colours are hardcoded rather than read from variables.

### Changed

- **"Token issued" reads like a date.** It showed
  `2026-09-03T01:47:02.351384747+00:00` — every digit true, and nobody
  reads it. Now `3 Sep 2026, 03:47` in the reader's own zone, with
  `2 hours ago` beside it. A value that cannot be parsed is shown
  unchanged rather than blanked: a timestamp nobody can read is still
  evidence.

- **The Add source button lines up with the controls again.** It sat
  below them because `align-items-end` stretched its column to the
  tallest one, which is whichever field carries the longest hint.

- **Making a calendar shows that it is working.** The button disables
  itself, spins and says "Asking Google…" until the page comes back — a
  round trip to Google behind a button that looks idle invites a second
  click, and a second click used to mean a second calendar.

## [2.0.0] — 2026-09-03

**A source now speaks Almanac's language.** The mapping profile stops
describing what a payload means; the call carries the event.

### Changed — breaking

- **Every per-event option travels in the call** (K23). `all_day`,
  `color`, `busy`, `status`, `reminders`, `end`, `duration_minutes`,
  `duration_days` and `timezone` are payload fields. In the v1 format
  they were profile settings — one value for every event that source
  would ever send — so a source could not say "this one event is
  all-day" or "this one is red". Only the profile could, for all of
  them at once.

  Kenny, reading the dashboard's own help text: *"Die moeten natuurlijk
  gewoon in de api call die we vanuit onze sources krijgen zitten."*

- **The translation layer is gone.** A profile no longer names which
  payload field means the title. Offered the choice between adding a
  direct mode beside the old behaviour and removing it, Kenny chose
  removal: *"voor aanpassingen hadden we HTTPSwitchboard! dus doe het
  volgens mijn model!"* A webhook that cannot change what it sends goes
  through HTTPSwitchboard, which exists to translate message shapes.

  What made this cheap, measured before it was done: **Grafana and
  Uptime Kuma had never delivered a single event.** The entire journal
  history on CT 112 was home-assistant (5), the since-deleted
  energy-prices (4) and job-tracker (2). Their profiles and fixtures
  went with the layer they existed to prove.

- **Nothing outside the program can stop it starting.** Kenny,
  2026-09-03: *"een kapot profiel mag niet het opstarten van de app
  belemmeren … De app moet ten allen tijde zelf kunnen opstarten op
  zichzelf."*

  Loading profiles can no longer fail. A file that is malformed, written
  for `schema_version = 1`, unreadable, or claiming a `source_id`
  another file already uses is reported and left unserved; almanac
  starts and serves everything else. A missing profiles directory and a
  directory with nothing usable in it are both legitimate states — that
  is what a fresh machine looks like — and almanac serves zero sources
  and says so.

  This replaces the rule the module used to follow, and the reason is
  the dashboard: **the page from which a bad profile gets deleted is
  part of what would not have started.** A service that cannot come up
  because of a file it is supposed to manage has no way back.

  Every unusable file is listed on `/dashboard/sources` under *Not being
  served*, with what is wrong with it and a **Delete** button — by file
  name, because a broken profile often has no readable `source_id`,
  which is frequently the thing wrong with it.

  Two profiles claiming one `source_id` no longer take the whole set
  down either: the first in sorted order serves, the second is reported.
  AR15 keeps its meaning — one identity, one profile — with a resolution
  that is deterministic and local.

  An unserved source answers 401 to its own posts, the same as an
  unknown source, so its sender learns immediately rather than through
  silence.

- **A profile is now routing only:** `source_id`,
  `target_calendar_id`, and optional `timezone` and
  `default_duration_minutes`.

- **Unknown payload fields are refused by name.** A call sending
  `allDay` instead of `all_day` gets a message rather than a timed event
  and no explanation.

- **Delete replaces retire on the dashboard** (K21). Kenny: *"De optie
  retire die we nu hebben moet de hele source wissen."* Token and
  profile both go, immediately. The kyu model borrowed a day earlier
  kept the row because message history hangs off a kyu app; nothing
  hangs off a source here. Events already on the calendar are left
  alone — deleting a source says something about the source, not about
  what already happened.

### Fixed

- **A call Almanac could never find again is now refused at the door.**
  Without an `external_id` in the payload or an `Idempotency-Key`
  header, no marker is written on the Google event: every resend
  duplicates and delete answers 404. Measured against the live service
  by the JobTracker session on 2026-09-03 — two identical posts, two
  events, a 404 — hours after the dashboard started writing profiles.
  A default in a template fixes the next source; a refusal at ingest
  fixes all of them.

- **Colours are named, and an unknown one is refused.** Google's API
  takes a `colorId` — `"1"` to `"11"` — while its UI shows names. The
  `grafana` profile asked for `"tomato"`, which Google would have
  refused or ignored, and nobody would have known because that profile
  never sent an event. A source now writes `"tomato"` and Almanac
  translates it; anything unrecognised is refused rather than silently
  producing an event in the calendar's default colour, which is
  indistinguishable from having asked for nothing.

### Migration

**A source whose profile is still v1 gets a `401` on every post** — not
a 422, and nothing that mentions a schema version. That is deliberate:
an unserved source is indistinguishable from an unknown one, so probing
cannot map which sources exist. But it does mean the sender sees
"unauthorized" for a reason that has nothing to do with its token, and
the JobTracker session spent time working that out. The explanation is
on `/dashboard/sources` under *Not being served*, and in the log at
startup.

Nothing has to happen at upgrade time: an old profile is left unserved
and listed on the dashboard, the rest keep serving. To bring one back, reduce it to `schema_version = 2`,
`source_id` and `target_calendar_id`, and press *Reload profiles from
disk* — no restart. The source must also send Almanac's event shape; if
it cannot, HTTPSwitchboard goes in front of it. See
`docs/USER_GUIDE.md` §2.2 and runbook R18.

## [1.7.0] — 2026-09-03

### Fixed

- **A source added from the dashboard could create events Almanac could
  never delete** (K21). The written profile left out
  `external_id_field`, on the reasoning that naming it makes the field
  required in every payload and so would refuse a new source's first
  post. True, and the wrong trade: without it Almanac writes no marker
  on the Google event, so every resend duplicates **and**
  `DELETE /v1/ingest/{source}/events/{id}` can never find it again.

  Measured against the live service by the JobTracker session hours
  after 1.6.0 shipped: two identical posts, two events, and a delete
  answering 404. A refusal that names a missing field is recoverable in
  seconds; duplicates nothing can remove are not.

### Changed

- **The written profile now names every field that costs nothing to
  name.** Which those are was measured rather than assumed, because
  "what else is quietly missing" is the question that produced the bug
  above. Naming a field makes it one of two things: required in every
  payload (`title_field`, `start_field`, `external_id_field`,
  `end_field`) or optional (`description_field`, `location_field`). The
  optional two are now both named, so a source that sends a `location`
  gets it on the event instead of having it silently dropped.

  Everything else a profile can do — `all_day`, `busy`,
  `duration_days`, `end_field`, colours, statuses, reminders — changes
  behaviour rather than carrying a value and has no safe default. Those
  stay lines to add by hand.

### Internal

- The retirement test asserted that two strings appear on the page,
  both of which would still appear if retiring had done nothing. It now
  also asserts that the retire control is gone, which only happens if
  the state really changed. Same fault the homelab recorded as F263: an
  assertion about existence passes on a wrong value as happily as on a
  right one, and is harder to spot than a missing assertion because it
  looks like coverage.

## [1.6.0] — 2026-09-03

### Changed

- **Adding a source is two fields, not a profile** (K21). Kenny opened
  the surface shipped hours earlier and said what it should have been:
  *"enkel een naam van de bron en de naam van de target kalender"*. He
  was right, and measuring the three deployed profiles shows why the
  first version looked reasonable and was still wrong — they differ in
  almost every field because each matches a webhook nobody here
  controls (`commonLabels.alertname`, `monitor.name`). A source Kenny
  adds himself is one he controls, so it is cheaper for the source to
  speak Almanac's shape than for Almanac to learn a fourth.

  The form takes a source name and a calendar. Almanac writes the
  profile — the plain shape, field for field the deployed
  `home-assistant` profile.

  The calendar is a **dropdown of the ones that exist**, plus one entry
  that means *+ New calendar…* and reveals a box for its name. Picking
  should not require knowing a calendar id — an opaque string nobody
  types on purpose — and adding one should not require leaving the
  page. Submitting creates the calendar, shares it with
  `ALMANAC_CALENDAR_OWNER`, writes the profile and lists the source
  ready for a token, in one act.

  Creating still goes through find-or-create rather than a bare create:
  two tabs, or a second source added to a calendar made a minute ago,
  must not each get their own. A duplicate calendar is close to
  invisible — events land, nothing errors, and half of them are on a
  calendar nobody has open.

  The list is fetched when the page renders, and a failure to reach
  Google does not take the page down with it: the dropdown says why it
  is empty while the token controls below keep working, which is what
  someone came for when Google is unreachable.

  Creating needs `ALMANAC_CALENDAR_OWNER`. Without it an unknown name is
  refused with that reason rather than made into a calendar owned by the
  service account and visible to no human — a mistake this project has
  made twice already.

  `external_id_field` is part of the written profile. It was left out
  for one day, on the reasoning that naming it makes the field
  *required* in every payload — true, and the wrong trade. Without it
  there is no marker on the Google event, so every resend creates a
  duplicate **and** `DELETE /v1/ingest/{source}/events/{id}` can never
  find it again: Almanac cannot clean up what it made. Measured against
  the live service on 2026-09-03 by the JobTracker session — two
  identical posts, two events, a delete answering 404. A loud refusal
  naming a missing field beats silent duplicates nothing can remove.

  Anything the plain shape cannot express is still a file, edited by
  hand and picked up by *Reload profiles from disk*.

### Internal

- `shell::testing` is compiled into the library instead of being
  `cfg(test)`. Integration tests link the library as an ordinary
  dependency and could not see it, and the alternative was a second
  hand-rolled Google stub under `tests/` — the same fixture maintained
  twice, which is the shape of every drift this project has had to
  repair.

## [1.5.0] — 2026-09-02

### Added

- **Add a source from the dashboard** (K21). `/dashboard/sources` now
  opens with an editable starter profile: save it and the source is
  live, no restart. A profile placed on the machine by hand is picked up
  by *Reload profiles from disk* on the same page.

  Kenny went looking for that button and it was not there — while the
  user guide said the dashboard would "register the source". Adding one
  meant logging into the container, writing a file and restarting the
  service, because profiles were read exactly once at startup. The
  sentence in the guide is corrected in the same change.

  The submitted text is checked by `Profile::parse` — the function
  startup uses — rather than by a second copy of the rules in the
  browser. Fourteen settings, several mutually exclusive; two lists of
  the same constraints drift, and the half that drifts is the one that
  says "fine" to what the service then refuses.

  Nothing is overwritten: this page adds sources. A save whose
  `source_id` matches an existing profile is refused, and so is one
  whose file already exists, because replacing a working profile over a
  retyped id is the mistake that could not be undone from the same page.

- **Retire a source from the dashboard** (K21), on kyu's model at
  Kenny's request: revoking an app there keeps its row with a badge
  rather than erasing it. *Retire* revokes the source's token and
  renames its profile to `<source_id>.toml.retired` — which the loader
  does not read — so the source leaves the running set while the file,
  and the row, stay as the record that it existed. Renaming the file
  back and reloading undoes it.

  Refused while that source still has undelivered events, and the
  refusal says how many. The worker resolves an entry's calendar
  through its profile and the journal never drops an entry, so retiring
  first would strand them: unreachable, erroring on every pass, forever.

  *Revoke* is now labelled *Revoke token*, because it always meant "take
  the key away, leave the source" and there are two destructive buttons
  on that row now.

  Neither adding nor retiring touches events already on the calendar.

### Security

- **`source_id` is now checked for the characters it contains**, not
  only for being non-empty. It has always been a URL segment; with K21
  it also names the file the profile is written to, so
  `"../../etc/cron.d/x"` had to stop being a legal value. Letters,
  digits, `.`, `-` and `_`, not starting with a dot. The three deployed
  source ids are unaffected, asserted by a test.

## [1.4.0] — 2026-09-01

### Added

- **`ALMANAC_STATE_DIR`** (K20) — one setting moves Almanac's whole
  state tree. `profiles/` and `data/` derive from it, and the journal
  and token store from the resolved data directory. Unset, the root is
  the working directory, which is exactly what almanac did before.

  Asked for by the homelab, which is moving the native services onto
  bind-mounted host paths so a container can be destroyed and recreated
  for nothing. It tried almanac on 2026-08-31 and could not: four
  independent path settings whose defaults *happened* to form a coherent
  tree, with nothing to move. Now a standing requirement in the dev
  procedure — rule 28, "state has an address, and Kenny owns it".

  The four per-path settings remain and still win where present, with a
  test asserting the live deployment's exact configuration resolves
  unchanged. Adopting this release changes nothing anywhere; moving is a
  separate, deliberate act.

## [1.3.1] — 2026-08-31

### Fixed

- **`almanac update` would have done nothing under the homelab, and
  reported success.** The command read the release URL from the
  environment, but the homelab runs `update_cmd` outside systemd and so
  never sees the unit's `Environment=` lines — and with
  `ALMANAC_SELF_UPDATE=off`, which the supervised arrangement requires,
  the updater refused to build at all. Both paths ended in "not
  configured", exit 0, nothing changed, and a supervisor reading that as
  a successful update.

  The command now falls back to a compiled-in release URL — a property
  of the project, like the signing key — and ignores the
  `ALMANAC_SELF_UPDATE` switch, which governs the background loop rather
  than an explicit instruction. The periodic updater is unchanged: an
  unset URL there still means "this machine does not self-update".

  Caught between publishing 1.3.0 and switching the deployment over,
  which is the only window in which it was findable.

## [1.3.0] — 2026-08-30

### Added

- **`almanac update`** (K19) — one update, no restart, for a supervisor
  that owns both. Fetches, verifies, probes and installs, then exits;
  writes no probation state, because the thing that called it preserved
  its own copy of the binary and can roll back from outside a process
  that never starts, which this process cannot.

  Built so the homelab can manage almanac's updates. Its supervised
  update preserves the binary, runs `update_cmd`, restarts only if the
  binary actually changed, health-checks and restores on failure —
  which is why its stack file deliberately carried no `update_cmd`
  until now: two systems each holding a rollback would race. The split
  is along what each can actually do.

  `ALMANAC_SELF_UPDATE=off` stops the periodic updater; the explicit
  command still works with it set, because the variable governs the
  background loop, not an instruction from whoever is supervising.

## [1.2.1] — 2026-08-30

### Fixed

- **The dashboard's copy-token button could never work.**
  `navigator.clipboard` exists only in a secure context — https, or
  localhost — and the dashboard is served over plain HTTP on the LAN,
  which is neither. The button died with "navigator.clipboard is
  undefined" every time, in the only way the page is ever opened, and
  said so only in the browser console. Now: the modern API when it is
  genuinely present, `execCommand` next, and failing both the command
  appears already selected to be copied by hand.

### Added

- `examples/show_events.rs` reads a calendar back from Google —
  summary, start, end, free/busy marker and the private property the
  upsert matches on. Almanac's log says what it sent; this says what
  Google kept, and those are different claims.

### Added

- **Event length from the payload** (K18). `end_field` names the payload
  field holding the end, for sources that report a period rather than a
  moment. Exactly one of `duration_minutes`, `duration_days` and
  `end_field` may be set, refused at load time rather than resolved by
  read order. An end at or before the start is refused as well — Google
  accepts it and the result appears on no calendar, which is the worst
  kind of accepted.

  Found while building the first real source, not by testing: 267 tests
  were green and the four fields added hours earlier were proven against
  the real Google API, but a profile could only state a *constant*
  length. A cheap-power window is 480 minutes today and might be 45
  tomorrow. Third time in one day that using the thing found what
  testing did not.

### Added

- **All-day events** (K14). A profile with `all_day = true` produces a
  day marker rather than a timed block, which is what bin day, a
  birthday or a week away actually is. Accepts either a plain date or a
  timestamp from the source, so an existing sensor does not have to
  change to become an all-day source. Google's end date is exclusive
  and has its own test, because getting it wrong produces an event of
  zero length that shows up nowhere.
- **Location** (K15). `location_field` in a profile. The event model
  already had the field and already serialized it; it was hardcoded
  empty and unreachable — the second instance in one day of the thing
  the retrospective had just made a rule about.
- **Reminders** (K16). `[mapping.reminders]` with popup and email
  minutes, or `silent = true`. Omitting the block inherits the
  calendar's default, which is a third and different outcome from
  silence. Google's limits — five reminders, four weeks — are checked
  when the profile loads.
- **Free/busy and status** (K17). `busy = false` stops an infra
  incident from marking Kenny unavailable, which is the one addition
  here that met real data before it was recommended: both alert sources
  already send a status. `status_by` maps a payload field onto Google's
  three statuses in the same shape as `color_by`, and a fourth value is
  refused at startup.

`duration_minutes` becomes optional — an all-day profile has no minutes
to give — and a timed profile that omits it now fails at startup rather
than defaulting to a length nobody chose. Every existing profile is
unaffected.

**Declined in the same mini-round:** recurring events, attendees,
attachments, Meet links, per-event visibility. See `docs/FEATURES.md`
for why each; the recurrence reasoning in particular is worth reading
before anyone proposes it again.

## [1.0.0] — 2026-08-29

Almanac is finished in the sense that matters for a version number: the
interface it offers is settled, and it will be honoured. Everything
rated Essential is built and proven, the mapping-profile format is
pinned by a regression test that fails loudly if its shape changes, and
the guarantees that can only be shown on real hardware — power loss,
reboot, self-update, delete — have been shown there.

What 1.0.0 does not claim is mileage. At the time of release nothing
was posting to Almanac on its own; every payload it had handled was put
there deliberately. That is a statement about use, not about
readiness, and it is recorded here rather than glossed over.

### Fixed

- The debug surface reported `upsert_key: null` for every routing
  decision, including ones that had plainly deduplicated against a key.
  Found by using Almanac once as a source really would (Phase 9
  field test), and it mattered: it is the field someone reads when chasing
  a duplicate, and it sent them to edit a profile that was already
  correct.

## [0.1.4] — 2026-08-29

### Added

- Self-update now refuses to run inside a Docker or Podman image and
  says why, rather than depending on `ALMANAC_SELF_UPDATE=off` being
  present in the compose file. AR20 had always required this; only now
  does the binary enforce it. LXC deliberately does not count as an
  image — that is where self-update is meant to run.

### Documentation

- `docs/USER_GUIDE.md`, `docs/DEBUGGING_GUIDE.md` and
  `docs/ARCHITECTURE_REFERENCE.md` written.
- README honesty pass; runbook renumbered through R15, including how to
  replace the Google service account and the three traps in doing it.
- `docs/legacy/` for the Phase 1 inventory and the closed AFK queue.

## [0.1.3] — 2026-08-29

### Added

- **Prometheus metrics** at `GET /metrics` (M13). Six counters, journal
  depth, and a version label; no authentication, because a scraper that
  cannot log in reports a healthy service as down. No per-source
  labels, deliberately. An unreadable journal reports itself as
  unreadable rather than as empty.

### Fixed

- **The first self-update check happened six hours after start, not
  five minutes.** The interval was created before the startup delay and
  its immediate first tick consumed, so the delay achieved nothing.
  Every one of the nine self-update tests passed, because they all call
  `check_once` directly and none go near the loop. Found by running the
  drill on real hardware.
- A completed update check now logs a line either way. At debug level a
  working updater and a silently dead one produced identical logs,
  which is how the bug above stayed hidden.

## [0.1.2] — 2026-08-29

Superseded within the hour by 0.1.3; carried the interval fix only.

## [0.1.1] — 2026-08-29

First release published as a signed GitHub Release with all four
assets, and the first one a running instance was asked to install by
itself.

## [0.1.0] — 2026-08-29

First release of Almanac as a rebuilt service, deployed to CT 112 and
serving.

Almanac replaces `cal-stacean`, which was a single 1,681-line
`src/main.rs` with a hardcoded Vikunja integration, Infisical for
secrets, and no durability. Nothing of that shape survives; the
event-mapping and upsert pattern was kept as the template for the
general mapping-profile design. See `docs/legacy/INVENTORY.md` for what
was there and the 19 defects that shaped the rewrite.

### Added

- **Many sources, one hub** — per-source ingest endpoints, each with its
  own independently revocable bearer token (K6). Almanac is the only
  thing in the homelab holding Google credentials (K12).
- **Many calendars** (K3) — each mapping profile names its own target,
  and Almanac creates and shares the calendars itself rather than
  needing them made by hand.
- **A durable journal** (AR16) — every accepted payload is fsynced
  before the 202 is answered, replayed on start, and compacted as it
  grows. A power cut costs nothing.
- **Upsert by external id** (K2) — redelivery converges on the same
  event instead of duplicating, with `Idempotency-Key` (M7) for sources
  with no natural id.
- **A generic mapping-profile engine** (K5) — declarative per-source
  TOML, validated at startup so a bad timezone is caught then rather
  than by Google days later (M4).
- **Delete by external id** (K8) — a source can remove what it created,
  and only what it created.
- **Self-update from signed releases** (M10) — verify, probe with
  `--check`, keep the previous binary, revert if the new one does not
  become healthy.
- **An operator dashboard** (M12), **dry-run** (M9), **raw request
  capture** (M11) and a **debug status surface** (K11).
- **Graceful shutdown** (M2), **retry with backoff** (M3), and a dead
  letter for entries that can never be delivered (T1).
- Secrets from Latch, injected into the process and never written to
  disk — asserted by tests that run the real binary and grep its output.

[2.3.0]: https://github.com/kennypassenier/almanac/releases/tag/v2.3.0
[2.2.0]: https://github.com/kennypassenier/almanac/releases/tag/v2.2.0
[2.1.0]: https://github.com/kennypassenier/almanac/releases/tag/v2.1.0
[2.0.0]: https://github.com/kennypassenier/almanac/releases/tag/v2.0.0
[1.7.0]: https://github.com/kennypassenier/almanac/releases/tag/v1.7.0
[1.6.0]: https://github.com/kennypassenier/almanac/releases/tag/v1.6.0
[1.5.0]: https://github.com/kennypassenier/almanac/releases/tag/v1.5.0
[1.4.0]: https://github.com/kennypassenier/almanac/releases/tag/v1.4.0
[1.3.1]: https://github.com/kennypassenier/almanac/releases/tag/v1.3.1
[1.3.0]: https://github.com/kennypassenier/almanac/releases/tag/v1.3.0
[1.2.1]: https://github.com/kennypassenier/almanac/releases/tag/v1.2.1
[1.0.0]: https://github.com/kennypassenier/almanac/releases/tag/v1.0.0
[0.1.4]: https://github.com/kennypassenier/almanac/releases/tag/v0.1.4
[0.1.3]: https://github.com/kennypassenier/almanac/releases/tag/v0.1.3
[0.1.2]: https://github.com/kennypassenier/almanac/releases/tag/v0.1.2
[0.1.1]: https://github.com/kennypassenier/almanac/releases/tag/v0.1.1
[0.1.0]: https://github.com/kennypassenier/almanac/releases/tag/v0.1.0
