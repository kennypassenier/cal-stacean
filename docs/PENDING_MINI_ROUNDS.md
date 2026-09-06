# Open points

Things decided but not yet finished, kept here rather than in a
conversation — a conversation gets summarised and then the point is
gone. Each entry says what is waiting, and what closes it.

## Measurement owed: does norm N4 hold without a reminder?

**Opened** 2026-09-02, by the correction form for the latch key loss
(standing rule 29, all nine fields ratified by Kenny).

**The fault.** Almanac's only encrypted secret store had exactly one
durable copy — the `LATCH_KEY_ALMANAC` line in the container's
`EnvironmentFile`, which is in the restic backup — and nothing said so
anywhere. When the workstation keyring emptied during a system upgrade,
a recovery survey across every project filed
`almanac/dev/.env.enc` as *"a `dev` environment … Nothing operational
depends on this file"*, while it held the credentials the live service
runs on. Nothing was lost, and that was luck rather than design.

**The gate that let it through.** Phase 8, the documentation gate. The
runbook answered "the journal is gone" (R11), "the service account must
be replaced" (R15) and "the state moves" (R16), and never "the key is
gone". An approval form that asks about the strongest claims in a
document cannot find a missing scenario.

**Also present in**, measured on 2026-09-02: kyu, kyu-runner and
newsflash — three running services, all consuming latch secrets, none
documenting key loss. They fix it in their own sessions, not from here.

**The measure.** ECOSYSTEM norm N4: a service's runbook names every copy
of its secret key, what each copy survives, and gives a runnable recipe
that restores the workstation's copy from the running deployment.
Almanac's own is R17, written and shipped. Enforcement is discipline,
reinforced by a fixed question in the doc-writer agent's brief — not
code, and marked as such per standing rule 24.

**What closes this entry.** At the next Phase 8 documentation gate of
any project: did the key-loss section appear without Kenny or Claude
asking for it? Write the answer here and close it. If it did not, the
fallback is already decided: a check that fails the documentation gate
for a project with a latch link and no such section.

**Review of the norm itself:** at the retrospective of the third project
to apply it.


## Closed 2026-09-02: does the release guard actually block a red CI?

**The fault.** CI was red on every commit from 2026-08-29 10:07 to
2026-09-02, through releases 1.1.0 through 1.5.0. The `container` job
could not start the image it had just built —
`version 'GLIBC_2.39' not found` — because the `rust:1.97-slim` builder
moved to a trixie base while the runtime stage was still bookworm. The
`gates` job stayed green the whole time, and nobody read the rest. No
published binary was affected: those are built natively, and v1.5.0
needs GLIBC_2.39 while CT 112 has 2.41, measured before it was said.

**The gates that let it through.** Branch protection on `main` requires
the `gates` check but allows a bypass, and every push used it. And R1,
the release procedure, never said "check CI first" — seven times.

**Also present in:** `binary-puzzle-toolkit`, the only other repository
with `enforce_admins: false`. Its CI is green, so nothing accumulated
there, but the gap is the same.

*Corrected while carrying this out:* the approved measure said the same
guard would go into that repository's Makefile. It has no Makefile —
it releases from a tag-triggered workflow — so there is no equivalent
place to put it, and standing rule 19 keeps work on a project inside a
session opened in that project. It is therefore **still open there**,
and the shape of the fix has to be decided in its own session: either
`enforce_admins` on, or a guard in whatever it does use to tag. Written
down here rather than quietly dropped, because a measure that covered
two repositories and silently covers one is how a correction stops
being one.

**The measure**, code-enforced: `scripts/check-ci.sh`, run as the first
step of `make tag-*`, before the version bump.

**Measured immediately, on the real thing** — which is why this entry
opens closed rather than pending:

    ./scripts/check-ci.sh d49cb69   CI is green on d49cb69.          exit 0
    ./scripts/check-ci.sh 345d847   CI is RED — refusing to release. exit 1
    ALMANAC_ALLOW_RED_CI=1 …        skipped, releasing anyway.       exit 0
    ./scripts/check-ci.sh 88e6f83   no CI run found — has it been…   exit 2

Tested by calling the guard with a commit rather than by running the
release target, deliberately: the homelab put three fake tags on GitHub
testing their equivalent, because their first attempt used `exit 0`
inside a make recipe and each recipe line is its own shell, so make
carried on to the tag and the push. `make -n tag-minor` confirms the
guard is the second line and nothing mutates before it.

**Fallback if it proves unusable:** `enforce_admins` on both
repositories and changes go through a pull request. **Review:** at the
first release where this guard blocks something it should not have.

---

## Correction · the dashboard believed Google's list (2026-09-03)

Ratified by Kenny, all nine fields as proposed, 2026-09-03.

**What went wrong.** The dashboard showed a calendar it had itself had
deleted a second earlier, and gave no sign that a button was still
working. Google's calendar list is eventually consistent: one deleted
moments ago comes back in the next list call, which is exactly the call
the page makes after the delete. Measured in both directions —
`cargo run --example inspect_calendar_access` minutes later showed the
calendar genuinely gone at Google, so the delete had worked and the
list was what lied.

**Which gate let it through.** The 42 dashboard tests, which run on
every commit and could say nothing here: they talk to a stub that
answers instantly and consistently, and a stub cannot double-click. The
Phase 7 test plan accepted "the dashboard is proven against a stub" as
a known limitation with open eyes; this is what that limitation cost.
Same shape as the four days of red CI: a check that exists and asks the
wrong question is harder to find than a missing one.

**Where the same fault already sat** — measured, not guessed. Two
places read Google's calendar list; only one had been fixed. The other
is the create button, which is find-or-create precisely so a double
submit cannot make two calendars — and it looks for the existing one in
that same lagging list. The homelab's journal for CT 112 shows the
consequence at 19:56: two `deleted a calendar` lines where one calendar
had been asked for. The busy button makes that click unlikely; two tabs
still defeat it.

**The measure.** Almanac remembers what it made, the mirror of what it
already remembered deleting, and consults that memory before asking
Google. The create path is serialized per calendar name. The stub
learned to lag, so the tests can now ask the question that matters.

**Enforcement:** code. `k24_a_second_click_while_google_lags_does_not_make_a_second_calendar`
fails without the guard — verified by removing it and watching the test
go red, not by assuming. Plus three more: the fresh calendar appears on
the page before Google lists it, the memory lets go once Google catches
up, and a calendar created and deleted inside the same lag stays gone.

**Cost.** The confirmation costs a click on every delete, including the
hundred times it was not needed. The memory lives in the process and
dies with it, which is correct: after a restart Google has long caught
up. It buys nothing against a calendar created outside almanac in the
same second — theoretical here.

**CLOSED — measured on the live service, 2026-09-04.** Kenny created a
calendar and deleted one on the dashboard and reported the list correct
in both directions immediately: "getest en goedgekeurd". Checked which
version that was rather than assuming, because the create side only
exists in 2.3.0 and the homelab's deploy is a step of its own:

    curl http://10.10.10.12:8080/healthz
    {"status":"ok","version":"2.3.0"}

So both repairs were proven on the machine, in the place where the stub
cannot speak. The loop this entry existed to keep open is done.

**Fallback if that measurement fails:** stop reading Google's list to
render the page at all. Almanac knows on disk which calendars it made
and where each source writes, and that never lags; Google would then be
consulted only for what almanac did not make. Not the proposal, because
it gives up the one thing the current list does well — showing what
exists outside almanac.

**Review:** at the next work that touches the calendars panel.

## Open after 4.0.1 (2026-09-06, from the chassis-rs session)

| Item | What | Who |
|---|---|---|
| CF-7 measurement (chassis-rs) | Log in from Chrome on almanac.kp-soft.dev and delete calendar `almanac-test` — the exact action that was refused on 3.0.0. Claude's half is done on CT 112 (Chrome-header form → 200, cross-site → 403 as a page, script → 403 JSON) | Kenny |
| latch push | CT 112's latch clone holds the `ALMANAC_BOOTSTRAP_TOKEN` → `ALMANAC_TOKEN` rename (and the dropped `ALMANAC_CAPTURE_TOKEN`) as an uncommitted `.env.enc` change; CT 112 has no PAT, so GitHub (kennypassenier/secrets, almanac/dev) is behind until `latch push` runs from a machine with one — or the PC gets PAT + key and Claude does it | Kenny |
| D5 · scaffold files | `chassis sync` reports 657 lines of drift (no `deny.toml`, own `ci.yml`, older hooks). Own step on a branch: `chassis sync --write`, fix what the kit CI turns red (the login token in the container check), report item | Claude, next |
| `trusted_proxies` empty | Startup warning since 3.0.0: behind Traefik every client shares the proxy's IP; set `ALMANAC_TRUSTED_PROXIES` in the unit/env — announced to the homelab | Homelab Rust session |
