# Operations runbook

What to do when something happens. Written for the case where it is
2am, the notification woke you up, and you do not want to re-derive the
design from the code.

Everything here assumes the standalone LXC running under systemd
(`deploy/almanac.service`). The homelab-v2 compose path is different in
exactly one way: self-update is off and homelab v2 owns updates.

---

## R1 · Cut a release

```bash
make tag-minor          # bumps Cargo.toml, commits, tags
git push && git push --tags
cargo build --release
./scripts/sign-release.sh
gh release create v<version> dist/v<version>/* --title v<version> --generate-notes
```

**`make tag-*` refuses to tag a commit whose CI is not green.** It runs
`scripts/check-ci.sh` before touching anything — before the version
bump, before the commit, before the tag. Red exits 1 and stops there;
"still running", "no run found" and "cannot reach GitHub" all exit 2 and
say which, because a guard that treats every network hiccup as a failure
gets deleted and then guards nothing. To release anyway, deliberately:
`ALMANAC_ALLOW_RED_CI=1 make tag-minor`.

That exists because CI was red from 2026-08-29 to 2026-09-02 — seven
releases — and nobody read it. The `gates` job was green throughout,
which is what made the red `container` job easy to keep not seeing.
Branch protection on `main` allows a bypass and every push used it, so
looking was the only thing that could have caught it.

`make tag-*` bumps `Cargo.toml` and tags in one step deliberately: the
version in the binary and the version in the tag have to agree, and
`scripts/check-version.sh` fails the build if they ever do not. That is
not pedantry — an updater that compares its own version against the
latest release either never updates or updates on every poll when those
two disagree.

Signing happens on your machine, never in CI. A checksum served from
the same host as the binary proves nothing; the signature is the only
thing standing between an unattended updater and a compromised release
host.

**The release is invisible until `VERSION` is attached to it.** That
one asset is how running instances discover a new version.

## R2 · First install on a fresh machine

```bash
# as root, on the machine itself
apt-get install -y ca-certificates git   # git: `latch run` reads a git clone
useradd --system --home-dir /opt/almanac --shell /usr/sbin/nologin almanac
mkdir -p /opt/almanac/{data,profiles} /etc/almanac
install -m 0755 almanac /opt/almanac/almanac
install -m 0755 latch /usr/local/bin/latch
cp fixtures/profiles/*.toml /opt/almanac/profiles/   # examples — see below
```

**The shipped profiles are examples, not configuration.** Their
`target_calendar_id` values (`primary`, `infra`) are placeholders that
exist so the regression tests have something to pin. Deployed as they
are, a Home Assistant event lands on the service account's own invisible
calendar and an alert fails permanently against a calendar that does not
exist — no data lost, but no calendar either. Replace the id in each
profile with a real one before pointing any source at the service.

To create the real calendars under the service account, which then owns
them and needs no manual step in the Google Calendar UI:

```bash
latch run -- cargo run --example create_calendars     # creates, idempotent
latch run -- cargo run --example inspect_calendar_access   # shows who can see what
```

A calendar the service account creates is **invisible to everyone else**
until it is shared. `inspect_calendar_access` shows the ACL of each; add
your own account with `share_calendars` (or once, by hand) or nothing
that lands there will ever be visible.

Latch needs its cached clone and a link. Copy the clone from your
desktop rather than logging Latch in here — the clone is ciphertext
only, so copying it puts nothing readable on the machine, and it means
no GitHub token lives on an unattended box:

```bash
# from your desktop
tar czf - -C ~/.latch repo | ssh <machine> 'mkdir -p /opt/almanac/.latch && tar xzf - -C /opt/almanac/.latch'
# on the machine: tell latch which project this directory is
printf 'repo = "kennypassenier/secrets"\n\n[[projects]]\nname = "almanac"\ndir = "/opt/almanac"\n' \
    > /opt/almanac/.latch/config.toml
chown -R almanac:almanac /opt/almanac && chmod 700 /opt/almanac/.latch
```

The one key that opens those secrets. Pipe it — never paste it on a
command line, where it is visible in `ps`:

```bash
# from your desktop; the value never appears on screen or on disk
latch key show --reveal | awk '/^value/{print $2}' | \
    ssh <machine> 'umask 077; read k; printf "LATCH_KEY_ALMANAC=%s\n" "$k" > /etc/almanac/latch.env'
```

Take the value from the `value` line, not from a pattern of your own:
the key is 68 hex characters, and an extraction that assumes 64 will
silently truncate it and produce "stored key has 32 bytes, expected 34"
much later.

```bash
cp deploy/almanac.service /etc/systemd/system/
systemctl daemon-reload && systemctl enable --now almanac
```

Before starting it, prove the configuration is complete:

```bash
sudo -u almanac latch run -- /opt/almanac/almanac --check
```

`--check` loads the profiles, checks every secret Latch injects, and
proves the key opens the token store — then exits. It takes neither the
port nor the data-directory lock, so it is safe to run against a
machine that is already serving.

## R3 · What self-update does

Every six hours, and never within five minutes of a start:

1. fetch `latest/download/VERSION`; stop if it is not newer;
2. fetch `SHA256SUMS` and its `.minisig`, and verify the signature
   **before downloading the binary**;
3. download the binary and check it against the signed manifest;
4. run the new binary with `--check`;
5. move the running binary to `almanac.prev`, put the new one in place,
   record that an update is unproven, and SIGTERM itself so systemd
   restarts into it.

It skips a cycle entirely while captured requests are still retained —
restarting mid-investigation would discard exactly the requests you
were looking at.

Watch it with:

```bash
journalctl -u almanac -f | grep -i updat
```

## R4 · "Update reverted" notification

The new version installed, restarted, and did not stay up for a minute.
The previous binary is already back in place and running; nothing needs
doing tonight.

```bash
systemctl status almanac                 # should be active, on the old version
journalctl -u almanac -n 200 --no-pager  # why the new one died
ls -l /opt/almanac/almanac*              # .prev is gone; it was moved back
```

The revert only happens after a second start with the update still
unproven, so a slow start is not mistaken for a broken one. Fix the
cause, cut a new release; do not re-publish the same version number.

If the notification says the previous binary **could not** be restored,
that is the one case needing hands: install a known-good binary from a
GitHub release by hand (R2's `install` line) and restart.

## R5 · "Release failed verification" notification

Raised after three consecutive failures, so a truncated download does
not wake you.

It means one of two things, and they need opposite responses:

- **the release host is serving something it should not** — do not
  install anything by hand until you know what happened;
- **the signing key changed** and running instances still carry the old
  public key — see R6.

Nothing was installed either way. The service is unaffected and still
running the version it had.

## R6 · The signing key is lost or regenerated

There is exactly one key, deliberately (AR24). A spare in the same
vault protects against rotation, not loss, and rotation only matters
across many machines — there is one.

So losing it is not an emergency, it is an afternoon:

```bash
minisign -G                                   # new key pair
# put RELEASE_PUBKEY in src/shell/update.rs = the base64 line of minisign.pub
```

Then cut a release with the new key (R1) and install that one build by
hand once (R2's `install` line + `systemctl restart almanac`). From
then on self-update works again, because the running binary now carries
the new public key.

Back up `~/.minisign/minisign.key` to Bitwarden. Losing it costs one
manual install; losing it *and* not noticing costs months of silently
skipped updates, which is why R5's notification exists.

## R7 · Latch and the key on the LXC

`/etc/almanac/latch.env` holds `LATCH_KEY_ALMANAC` — the per-project
key, not the passphrase. The passphrase would open every project's
secrets and the GitHub token; the project key opens Almanac's five
values and nothing else.

**Losing the key on the LXC is not catastrophic.** Latch keeps every
credential in one passphrase-encrypted escrow file held offline, so
recovery is `latch key restore`, or a `latch clone` from the desktop.
No token needs re-issuing.

Note, consciously accepted: vzdump backs up that key alongside the
encrypted store it opens, so **the backup is as sensitive as the
secrets themselves**. Treat it accordingly. The alternative — excluding
the key — produces a restore that stops halfway for manual work, which
is the last thing anyone wants during a real outage.

## R8 · "Journal backlog" notification

Deliveries have been failing long enough that the journal is over half
its cap. Once it fills, ingest starts refusing events and the sources'
own retries eventually give up — so this arrives while there is still
room to act, not after.

```bash
journalctl -u almanac -n 100 --no-pager | grep -i "delivery failed"
curl -H "Authorization: Bearer $ALMANAC_TOKEN" \
     http://localhost:8080/v1/debug/status
```

Usually Google is unreachable or the service account lost access to a
calendar. Nothing is lost while the journal has room; entries deliver
themselves once the cause is fixed. The worker backs off as the outage
lasts, up to half-hourly, and speeds back up the moment anything gets
through.

## R9 · Roll back on purpose

```bash
# Rename, never overwrite: writing over a binary that is executing
# fails with "Text file busy". A rename replaces the directory entry
# and leaves the running process on its old inode, which is exactly
# what the self-updater does.
install -m 0755 <old binary> /opt/almanac/almanac.new
chown almanac:almanac /opt/almanac/almanac.new
mv /opt/almanac/almanac.new /opt/almanac/almanac
systemctl restart almanac
```

Then either publish a newer release or the updater will put the newer
version straight back on its next check. To stop that:

```bash
systemctl edit almanac      # Environment=ALMANAC_SELF_UPDATE=off
```

## R10 · Two processes, one data directory

Almanac takes an `flock` on `data/.lock` at startup and refuses to
start if another process holds it. If you see that refusal, a previous
instance is still running — find it before doing anything else. Two
processes over one journal deliver the same event twice and can lose
delivery records.

`--check` does not take the lock, so it is always safe to run.

## R11 · Backup and restore

State lives in four places, and only one of them is on the LXC:

| What | Where | Restore |
|---|---|---|
| Profiles (examples) | git (`fixtures/profiles/`) | `git clone`, then set the real calendar ids |
| Real calendar ids | this deployment only | `cargo run --example inspect_calendar_access` lists them |
| Secrets | Latch escrow (offline) | `latch key restore` / `latch clone` |
| Journal (transient) | `/opt/almanac/data` | nothing to do — it is empty in steady state |
| Calendar data | Google | nothing to do |

**`restic ls` cannot show you a profile.** The homelab's nightly
snapshot of this state root is a single `almanac-data.tar`, so listing
the snapshot shows exactly one path and no files inside it. To confirm
that something is really in the backup — a profile, the token store —
restore the snapshot and run `tar tf` on the archive. Measured on
2026-09-03 by the homelab before deleting three profiles: the directory
name in the snapshot said nothing, and only the extracted listing showed
all four files.

Worth the extra minute whenever "it is in the backup" is the reason
something is about to be deleted. Trusting the path is how that sentence
becomes true-sounding and unverified.

**A restore can bring a retired source back to life.** Since 1.5.0 the
dashboard can retire a source, which renames its profile to
`<source_id>.toml.retired` and keeps the file (K21). Restoring the
profiles directory to a moment before that retirement restores the file
under its original `.toml` name, and almanac loads it again on the next
start — token and all, once one is issued. That is a restore doing
exactly its job, not a bug, but it means the source list after a restore
is the list as it was *then*, not as it was left. Check
`/sources` (and the kit's Sources page for the tokens) after any restore that reaches back past a
retirement.

Recorded on the homelab's side as F250 in the same measurement that
confirmed nothing in their deploy path ever deletes from `/appdata` —
so both the live profiles and the `.toml.retired` records survive a
deploy and ride along in the nightly snapshot.

Since 2026-08-29 the homelab also takes a nightly restic snapshot of
`/opt/almanac` and `/etc/almanac` to Drive. Worth knowing exactly what
that contains, because it is both halves of the same lock: `/etc/almanac`
holds `latch.env` with the project key, and `/opt/almanac` holds the
encrypted token store and the Latch clone the key opens.

That is not a new category of exposure — restic encrypts client-side
with AES-256 before anything leaves the machine, under Kenny's own
64-character password which lives in Bitwarden and never at Google, and
the same Drive already holds the homelab's full secrets vault under the
same encryption. What it does change is the inventory: whoever holds
that restic password now also holds lock and key for Almanac's secrets
in one place.

So a destroyed LXC is a rebuild, not a recovery: R2, restore the Latch
key, done. The journal is worth backing up only if it is non-empty at
the moment of the backup, which means deliveries were failing then.

**The data directory is exactly three plain files**, and that is worth
stating because it is not true of every service: `journal.jsonl`,
`tokens.json`, and an empty `.lock`. No database, so no companion files
that carry the recent writes somewhere else — the trap that costs
people a store is moving a SQLite `.db` without its `-wal` and `-shm`,
where the database looks tiny and complete while the actual data sits
in the log beside it. Nothing here works that way; copy the directory
and you have copied everything.

Copying it while the service runs is safe. The journal is append-only,
so the worst a hot copy can catch is a half-written final line, and
replay is written to tolerate exactly that — `replay_survives_a_torn_final_line`
in `shell::journal` is the test that keeps it true. Restore it by
putting the files back and starting the service; anything undelivered
goes out on that start.

## R12 · Metrics and what to alert on (M13)

`GET /metrics` on the same port, no token needed. Prometheus on CT 113
scrapes it:

```yaml
  - job_name: almanac
    static_configs:
      - targets: ["10.10.10.12:8080"]
```

Six series, all prefixed `almanac_`: events accepted, delivered,
failed, set aside, token refreshes, and the journal depth. Plus
`almanac_build_info{version="..."}`, which is how you see at a glance
which version the hub is actually on.

Two of these are worth an alert:

- `almanac_journal_pending` climbing and not coming back down means
  deliveries are failing. The hub is doing the right thing — that is
  what the journal is for — but nobody is looking at the reason yet.
- `almanac_journal_readable` at 0 means the scrape could not read the
  journal at all. The depth gauge is deliberately absent in that case
  rather than reported as zero, so an alert on `pending == 0` will not
  fire and hide it. Alert on this one separately.

`almanac_deliveries_failed_total` climbing while
`almanac_events_delivered_total` also climbs is a retry story, not an
outage. Only sustained failures with no deliveries are a problem.

Do not point Uptime Kuma at `/metrics` or Prometheus at `/healthz`.
Liveness is `/healthz` (JSON, watched by Uptime Kuma per AR21); metrics
are here (exposition format). Prometheus parses only its own format, so
pointed at `/healthz` it would report a perfectly healthy service as
permanently down.

## R12a · Do not restart within a minute of a self-update

Learned the hard way on 2026-08-29, in production.

After installing a new version Almanac runs it **on probation for 60
seconds** (AR23). A restart inside that window looks exactly like "the
new version did not come up", so the old binary is put back — correctly,
and silently as far as the person restarting is concerned. `/healthz`
reports the new version the whole time, so checking the version is not
enough to know the update has stuck.

What it cost: a profile using a feature only the new version understood
was installed a few seconds after the update, the restart that picked it
up reverted the binary, and the old version then refused the profile and
crash-looped. Three minutes down. Nothing was lost — the journal was
intact and the reverting is what it is supposed to do — but the outage
was entirely self-inflicted.

**Wait for the confirmation line, not the version:**

```bash
journalctl -u almanac -f | grep "update confirmed"
```

Only after that line has appeared is the previous binary released and a
restart safe. It arrives 60 seconds after the new version starts
serving.

## R12b · When the homelab manages updates

> **3.0.0 (chassis migration):** the updater is the kit's. The knob is
> `ALMANAC_UPDATE_MODE` = `off` | `supervised` | `autonomous` (the 2.x
> `ALMANAC_SELF_UPDATE` on/off is still honoured with a warning; unset means
> off). Under the homelab set `supervised` and let the stack's `update_cmd`
> call `almanac update` (see `deploy/service.yml`); the binary lives at
> `/opt/almanac/bin/almanac`. Releases must carry the signed manifest with
> the trusted comment `kennypassenier/almanac v<version>` — releases up to
> 2.4.0 do not and are refused, so the first 3.x is installed by hand.

Two update mechanisms exist and exactly one should be armed.

**Almanac alone** (the default): the periodic updater checks every six
hours, installs, restarts itself, and reverts on the next start if the
new version does not serve. Nothing else needed.

**Under the homelab**: set `ALMANAC_SELF_UPDATE=off` in the unit's
environment and give `stacks/almanac/service.yml` an `update_cmd`:

```yaml
update_cmd: runuser -u almanac -- /opt/almanac/almanac update
```

`almanac update` fetches, verifies, probes and installs — then exits
**without restarting**. The homelab restarts the unit, checks it came
up, and restores the binary it preserved beforehand if it did not.

Exit codes, because the homelab reads them: `0` both when there was
nothing to do and when a new version was installed — the caller decides
what happens next by comparing the binary's checksum, which is exactly
what it does. `1` only when the attempt itself failed, which leaves the
service on the version it is already running.

**Checking what happened after: the binary and the running process can
briefly disagree, on purpose.** `almanac update` installs the new
binary but does not restart — that is the homelab's job, next. Between
those two steps, `almanac --version` (reads the file on disk) and
`/healthz` (reports the running process) can legitimately answer
differently: `--version` already says the new number; `/healthz` still
says the old one, correctly, because that is still what is serving
traffic. Found on 2.4.0's rollout, when a nightly `update_cmd` run won
a race against a manual one: `/healthz` at the moment in question was
the right thing to have looked at, and it was. After a supervised
update, verify against `/healthz` — it answers "what is actually
running", which is the question that matters here. `--version` is for
confirming what a specific downloaded or installed file is, without
starting it.

**Never arm both.** Two systems each preserving and restoring a binary
will eventually restore each other's copy. If `almanac update` is in the
stack file, `ALMANAC_SELF_UPDATE` must be off; if it is not, it must
not be.

The explicit command still works while the variable is off. That is
deliberate: the variable governs the background loop, not an instruction
from whoever is supervising.

**It also finds releases without being told where they are.** The
supervisor runs `update_cmd` outside systemd, so the unit's
`Environment=` lines are absent from that process. 1.3.0 read the URL
from there and, not finding it, printed "self-update is not configured
here; nothing to do" and exited **0** — which the supervisor reads as a
successful update that installed nothing. Demonstrated on CT 112 before
the switch-over rather than reasoned about:

```
$ pct exec 112 -- runuser -u almanac -- /opt/almanac/almanac update
almanac update: self-update is not configured here; nothing to do
$ echo $?
0
```

1.3.1 compiles the release URL in, the same way the signing key already
is. When checking this by hand, run it the way the supervisor does —
`runuser`, no environment — not from a shell that happens to have the
variables.

## R13 · Is the self-updater still looking?

Every six hours, and five minutes after each start, the log gets one
line either way:

```
checked for a new release; already on the latest version=0.1.3
```

If that line has not appeared since the last restart plus five minutes,
the updater is not running — which is a real failure mode: it once sat
silent for six hours because the check interval was scheduled from
process start rather than from the end of the startup delay, and
nothing in the log distinguished that from working correctly.

To see when it last looked:

```bash
journalctl -u almanac | grep "checked for a new release" | tail -3
```

## R14 · Who installs new versions

Almanac updates itself, unless it is running from an image somebody
else builds.

**On the LXC (the live deployment):** it checks every six hours, and
five minutes after each start. It verifies the minisign signature and
the checksum, runs the new binary once with `--check` before trusting
it, keeps the old one as `almanac.prev`, and reverts if the new version
does not reach "serving" within a minute of starting. Nothing to
configure.

**In a docker image:** self-update switches itself off, and says so in
the log:

```
running inside a docker or podman image — self-update is off by
default, because a binary replaced inside a container is lost the
moment the container is recreated while looking identical to the image
it came from. Update by pulling a new image. Set ALMANAC_SELF_UPDATE=on
to override.
```

This is AR20 enforced by the binary rather than trusted to whoever
writes the compose file. A container that replaces its own binary keeps
running the new version until it is recreated, then silently goes back
to the image's version — and every diagnosis after that starts from the
wrong version number.

**LXC is not treated as an image.** An LXC container is a long-lived
machine with a filesystem that survives; a Docker container is a
rebuilt artifact. The check is specifically for Docker and Podman
(`/.dockerenv`, `/run/.containerenv`, and the OCI markers in
`/proc/1/cgroup`) and never for "am I in a container", which would
switch self-update off on exactly the machine it was built for.

**Turning it off or on by hand.** `ALMANAC_SELF_UPDATE` accepts
`off`/`false`/`0`/`no` and `on`/`true`/`1`/`yes`, case-insensitively.
Anything it does not recognise counts as **off** — a slipped finger
should not be what lets a process rewrite its own binary. An empty
value is the same as not setting it at all.

```yaml
# docker-compose.yml — the default already, stated for the reader
environment:
  ALMANAC_SELF_UPDATE: "off"
```

```yaml
# a container run as a long-lived pet, with the data directory on a
# volume, may opt back in
environment:
  ALMANAC_SELF_UPDATE: "on"
```

```bash
# on the LXC, to hand updates to something else
systemctl edit almanac      # Environment=ALMANAC_SELF_UPDATE=off
```

## R15 · Replacing the service account

Done once, on 2026-08-29, when the account was still called
`cal-stacean` and every mail Google sent said so. A service account's
name cannot be changed after creation, so a rename means a new account
— and a new account owns nothing, which is most of the work.

**In the Google Cloud console (only a person can do this):** create the
service account, give it no roles at all (Almanac only touches calendars
it created itself), add a JSON key, and make sure the Google Calendar
API is enabled on the project.

**Then, on Kenny's machine:**

```bash
latch edit .env      # replace CLIENT_EMAIL and PRIVATE_KEY
latch commit .env && latch push
latch run -- cargo run --example create_calendars      # new account, new calendars
latch run -- cargo run --example create_test_calendar  # for the live tests
latch edit .env      # point ALMANAC_TEST_CALENDAR_ID at the new one
latch commit .env && latch push
latch run -- cargo test --test calendar_e2e -- --ignored   # prove it before touching the deployment
gh secret set CLIENT_EMAIL PRIVATE_KEY ALMANAC_TEST_CALENDAR_ID   # the nightly live tests
```

**Then, on the deployment:** the LXC has a ciphertext-only clone with no
credentials to pull with, by design — copy `~/.latch/repo` across again
(`tar`, `pct push`, `chown -R almanac:almanac`), update
`target_calendar_id` in every profile under `/opt/almanac/profiles/` to
the new calendars, and restart. `almanac_token_refreshes_total` going to
1 on `/metrics` is the proof it authenticated with the new key.

**Three things that cost time and will again:**

`latch edit` honours `VISUAL` before `EDITOR`. On this machine `VISUAL`
was set to `“kate -b”` with typographic quotes, so every attempt tried
to launch a program literally named `“kate`. Set both variables, and
point them at an executable file — latch spawns the value as one
program name, not through a shell, so `EDITOR="python3 script.py"` is
read as a binary called `python3 script.py`.

The profiles on the deployment hold the calendar ids, and they are not
in this repository (deliberately — they are the household's, not the
code's). Changing accounts without changing the profiles leaves Almanac
authenticating perfectly and writing to calendars it no longer owns.

The old calendars stay in Kenny's calendar list until the old service
account is deleted; they are owned by that account, not by him. Deleting
the old project in the console removes both.

## R16 · Moving Almanac's state somewhere else

Everything Almanac keeps lives under one root, and one setting moves it:

```
ALMANAC_STATE_DIR=/appdata/almanac
```

That yields `/appdata/almanac/profiles` and `/appdata/almanac/data`,
with the journal, the sealed tokens, the update state and the exclusive
lock inside the latter. Unset, the root is the working directory, which
is what almanac did before this existed.

**The binary is not state.** It belongs to a version, not to the data,
and stays where the unit expects it. Moving the state root does not move
it and should not.

**There is no cache to leave behind.** Almanac keeps nothing regenerable
on disk, so the root is exactly what a backup needs and a backup of it
carries no ballast.

**The four older settings still work and still win.**
`ALMANAC_PROFILES_DIR`, `ALMANAC_DATA_DIR`, `ALMANAC_JOURNAL` and
`ALMANAC_TOKEN_STORE` override the derived paths individually. CT 112
sets all four absolutely and is therefore unaffected by this feature
until someone removes them — which is the point: adopting the release
changes nothing, and the move is a separate, deliberate act.

To actually migrate a deployment: stop the service, copy the tree,
replace the four settings with one `ALMANAC_STATE_DIR`, make sure
`ReadWritePaths` in the unit covers the new root (`ProtectSystem=strict`
makes everything else read-only, and the failure is
`Read-only file system (os error 30)`), then start it.

One trap that is not almanac's but bites here: the unit runs the binary
under `latch run`, and latch resolves its project link by absolute path.
Moving the working directory without telling latch gives
`'<dir>' is not linked to a latch project`. Relink latch, or leave the
working directory where it is and move only the state root.

**If you relink, relink in the service user's HOME.** Latch reads
`~/.latch/config.toml`, not a `.latch` beside the working directory.
The almanac user's home is `/opt/almanac`, so that is the file that
counts — and this is easy to get wrong in a way that looks fixed: the
homelab's migration on 2026-08-31 ran perfectly with an identical
`.latch` copy sitting in the new working directory, then failed the
moment the old `/opt/almanac/.latch` was renamed, because latch had
never been reading the copy. Recorded on their side as F128.

**What a healthy backup of the state root looks like.** After that
migration, and after deleting the redundant latch clone that had been
riding along, the backup went from 217 files / 5 345 657 bytes to
**7 files / 8 571 bytes** — the journal, the sealed tokens, and the
mapping profiles. If a snapshot of this root is megabytes, something
that is not almanac's state is in it.

## R17 · The secrets live in an environment called `dev`

Almanac has exactly one latch environment, and it is named `dev`:
`almanac/dev/.env.enc`. There is no `prod`. That name is a leftover from
latch's default, not a statement about what the file is — it holds the
credentials the live service on CT 112 runs on, and losing it means
minting a new Google service account.

Read that name as **production**, whatever it says.

This is not hypothetical. On 2026-09-02 a system upgrade emptied the
workstation's keyring and a recovery survey across all latch projects
listed almanac's single unreadable file as "a `dev` environment,
nothing operational depends on it". That reading was reasonable from the
outside and wrong.

**Where the key actually is.** Two copies, and only one of them was ever
in the keyring:

| Copy | Where | Survives |
|---|---|---|
| the workstation's | the kernel keyring, slot `key:almanac` | a logout or a keyring wipe: **no** |
| CT 112's | `LATCH_KEY_ALMANAC` in `/appdata/almanac/almanac-config/latch.env` | anything the restic backup survives |

The container's copy is what made 2026-09-02 a non-event. It is a
side effect of how the service is run, not a designed escrow, so it is
not a substitute for `latch key backup`.

**To restore the workstation's copy from the running service:**

```bash
# read the key out of the container and put it where latch looks;
# it is 34 bytes (2-byte generation + 32-byte key), carried as hex
PERS=$(keyctl get_persistent @s)
ID=$(ssh proxmox "pct exec 112 -- grep '^LATCH_KEY_ALMANAC=' \
       /appdata/almanac/almanac-config/latch.env | cut -d= -f2-" \
     | tr -d '\r\n' | xxd -r -p \
     | keyctl padd user "keyring-rs:key:almanac@latch" "$PERS")
keyctl link "$ID" @s
latch state    # expect: project almanac … key gen 1 (Keyring)
latch verify   # expect: ok  almanac/dev/.env.enc
```

**The trap in that recipe:** the keyring holds the key as raw bytes, not
as the hex text the environment variable carries. Store the hex string
verbatim and latch reports the key as `MISSING` — not as corrupt — which
looks exactly like having stored nothing at all. Hence `xxd -r -p`.

**Do not re-mint while the service is running.** `latch commit` mints a
new project key and re-encrypts everything with it; the key inside CT
112 then no longer opens the archive. The running process keeps working,
because it read its secrets at startup — and fails to start the next
time anything restarts it. If a re-mint is ever genuinely needed, replace
`latch.env` in the container and restart the service in the same
sitting.



## R18 · Almanac always starts; a profile it cannot use is listed instead

Loading profiles cannot fail. Whatever is wrong with a file — malformed
TOML, `schema_version = 1`, unreadable, a `source_id` another file
already claims — almanac reports it, does not serve that source, and
starts.

```
ERROR almanac: a profile could not be used; this source is not being served
      path=/appdata/almanac/almanac-config/profiles/uptime-kuma.toml
      reason=written for schema_version 1; this build reads 2 — reduce it to …
```

The count rides on the startup line next to the sources it did load, and
every unusable file is listed on `/sources` under **Not being
served**, with the reason and a **Delete** button.

**Why it works this way** (Kenny, 2026-09-03): the dashboard is where a
bad profile gets deleted, so a bad profile that stops the service takes
away the means of fixing itself. Nothing outside the program decides
whether the program runs.

**A directory that does not exist is fine.** So is one with nothing
usable in it: almanac serves zero sources, says so at warn level, and
waits for one to be added from the dashboard — which creates the
directory if it has to. That is what a fresh machine looks like.

**What a skipped source experiences:** its posts answer 401, the same as
an unknown source. It is not registered as far as this build is
concerned, and the sender sees that immediately rather than as silence.

That is the right answer — an unserved source must be indistinguishable
from an unknown one, or probing maps which sources exist — but it is
also a confusing one from the other end: the sender reads
"unauthorized" and checks its token, which is fine. So when someone
reports a 401 they cannot explain, look here first. The dashboard and
the startup log both name the real reason.

**Fixing a v1 profile:** reduce it to `schema_version = 2`, `source_id`
and `target_calendar_id`, then press *Reload profiles from disk*. No
restart. The source must also send Almanac's event shape — if it
cannot, put HTTPSwitchboard in front of it, which is what that tool is
for.
