//! Almanac — event-to-calendar hub. On chassis since 3.0.0: the kit owns
//! the command line, the transport knobs, logging, `/healthz`, `/metrics`,
//! readiness, the graceful stop and signed self-update. This file assembles
//! Almanac on top of it.
//!
//! Startup order still matters: everything that can be checked without
//! side effects — profiles, the credentials Latch injects, the key that
//! opens the token store, the journal — is checked before the kit binds, so
//! a misconfigured process fails immediately and visibly. What needs the
//! network (authenticating against Google, AR21) runs AFTER the bind, so a
//! power cut that starts Almanac before the network settles never parks
//! the unit: it serves, the journal accepts events, and delivery starts
//! the moment Google answers.

use std::collections::{BTreeMap, HashMap};
use std::process::ExitCode;
use std::sync::{Arc, Mutex};

use almanac::core::paths::{self, Paths};
use almanac::shell;
use almanac::shell::admin::{BOOTSTRAP_TOKEN_ENV, CAPTURE_TOKEN_ENV};
use almanac::shell::auth::{TokenManager, load_credentials};
use almanac::shell::calendar_client::GoogleCalendarClient;
use almanac::shell::datadir::DataDirLock;
use almanac::shell::ingest::AppState;
use almanac::shell::journal::{DEFAULT_MAX_BYTES, Journal};
use almanac::shell::kit::{import_source_tokens, mount};
use almanac::shell::notify::Notifier;
use axum::Router;
use chassis::{App, AppSpec, Control};
use tokio::sync::watch;

/// Backoff between startup authentication attempts, in seconds; the
/// last value repeats. Never gives up: a wedged unit that nobody
/// restarts is worse than one that keeps trying quietly (AR21).
const STARTUP_RETRY_WAITS: [u64; 5] = [2, 5, 15, 60, 300];

/// What `--help` says beyond the kit's knobs: Almanac's own environment.
const HELP_EXTRA: &str = "Almanac's own environment (read next to the knobs above):
  ALMANAC_SECRET_KEY            64 hex chars; seals the token store (mandatory)
  ALMANAC_TOKEN                 the login token (kit knob): dashboard login and admin bearer; required
                                (was ALMANAC_BOOTSTRAP_TOKEN; ALMANAC_CAPTURE_TOKEN is gone — use a client token)
  ALMANAC_NOTIFY_WEBHOOK        Home Assistant webhook for almanac's own notifications
  ALMANAC_HEARTBEAT_INTERVAL_SECS  one heartbeat line per interval (default 3600, 0 = off)
  ALMANAC_CALENDAR_OWNER        who new calendars are shared with
  ALMANAC_PROFILES_DIR, _DATA_DIR, _JOURNAL, _TOKEN_STORE  2.x per-path overrides of the
                                state root (deprecated; derived from ALMANAC_STATE_DIR)
  CLIENT_EMAIL, PRIVATE_KEY, TOKEN_URI  the Google service account, injected by `latch run --`
  ALMANAC_BIND, ALMANAC_SELF_UPDATE, RUST_LOG  2.x names of ALMANAC_LISTEN, ALMANAC_UPDATE_MODE
                                and ALMANAC_LOG; still honoured, with a warning";

fn die(e: impl std::fmt::Display) -> ExitCode {
    eprintln!("{e}");
    ExitCode::FAILURE
}

/// The 2.x names, mapped onto the kit's knobs on the environment snapshot
/// (never `set_var`, which is unsound once threads exist). Returns what to
/// say about it once logging is up — standing rule 12: no silent
/// substitution.
fn compat(env: &mut BTreeMap<String, String>) -> Vec<String> {
    let mut warnings = Vec::new();
    if !env.contains_key("ALMANAC_LISTEN")
        && let Some(bind) = env.get("ALMANAC_BIND").cloned()
    {
        env.insert("ALMANAC_LISTEN".to_string(), bind);
        warnings.push(
            "ALMANAC_BIND is the 2.x name of ALMANAC_LISTEN; it still works, rename it in the \
             environment file — the alias goes away in 4.0"
                .to_string(),
        );
    }
    if !env.contains_key("ALMANAC_UPDATE_MODE")
        && let Some(raw) = env.get("ALMANAC_SELF_UPDATE").cloned()
    {
        let mode = match raw.trim().to_ascii_lowercase().as_str() {
            "on" | "true" | "1" | "yes" => "autonomous",
            _ => "off",
        };
        env.insert("ALMANAC_UPDATE_MODE".to_string(), mode.to_string());
        warnings.push(format!(
            "ALMANAC_SELF_UPDATE={raw} is the 2.x knob; it now means ALMANAC_UPDATE_MODE={mode} \
             (off | supervised | autonomous) — set that instead, the alias goes away in 4.0"
        ));
    }
    if let Some(url) = env.get("ALMANAC_UPDATE_URL").cloned() {
        let trimmed = url.trim_end_matches('/');
        if trimmed.ends_with("/releases") {
            let fixed = format!("{trimmed}/latest/download");
            env.insert("ALMANAC_UPDATE_URL".to_string(), fixed.clone());
            warnings.push(format!(
                "ALMANAC_UPDATE_URL={url} has the 2.x shape; the kit wants the directory holding \
                 VERSION and SHA256SUMS, so {fixed} is used — set that, or unset it to derive it \
                 from the repository"
            ));
        }
    }
    if !env.contains_key("ALMANAC_LOG")
        && let Some(filter) = env.get("RUST_LOG").cloned()
    {
        // A convention rather than an Almanac knob until 3.0.0; honoured quietly.
        env.insert("ALMANAC_LOG".to_string(), filter);
    }
    warnings
}

#[tokio::main]
async fn main() -> ExitCode {
    let mut env: BTreeMap<String, String> = std::env::vars().collect();
    let warnings = compat(&mut env);

    let spec = AppSpec {
        name: "almanac",
        version: env!("CARGO_PKG_VERSION"),
        repository: Some("kennypassenier/almanac"),
        help_extra: Some(HELP_EXTRA),
        ..Default::default()
    };
    let args: Vec<String> = std::env::args().collect();
    // The routes need the state, which needs the paths the kit resolves —
    // so the router is attached below, as public routes with Almanac's own
    // door policy (bearer tokens, the session cookie) inside them.
    let mut app = match App::from_args_with_env(spec, args, env.clone(), Router::new()) {
        Ok(app) => app,
        Err(e) => return die(e),
    };
    if !app.needs_project_config() {
        // --version, --help, --healthcheck, --print-config, gen-secret,
        // update, rekey: the kit's alone, and they must work without
        // Latch, a token store or a profiles directory.
        return app.run().await;
    }
    let checking = matches!(app.control, Some(Control::Check));
    // 4.0.0: the login token is the kit's ALMANAC_TOKEN. A 3.x environment
    // file still naming the old variable is refused with the rename spelled
    // out (standing rule 12); the capture token is simply no longer read.
    if env
        .get(BOOTSTRAP_TOKEN_ENV)
        .is_some_and(|v| !v.trim().is_empty())
    {
        return die(format!(
            "{BOOTSTRAP_TOKEN_ENV} is the 3.x name; 4.0.0 reads ALMANAC_TOKEN (the kit's login token)\n  remedy: rename the variable in the environment file (latch.env on CT 112) and restart"
        ));
    }
    let mut warnings = warnings;
    if env
        .get(CAPTURE_TOKEN_ENV)
        .is_some_and(|v| !v.trim().is_empty())
    {
        warnings.push(format!(
            "{CAPTURE_TOKEN_ENV} is no longer read since 4.0.0: a system that posts captures gets a client token from the Sources page instead; remove the variable"
        ));
    }
    let state_dir = app
        .loaded
        .as_ref()
        .expect("a start or --check loads configuration")
        .state_dir
        .clone();
    // K20: every path from one place — the kit's root — with the 2.x
    // per-path overrides still honoured (deprecated, see --help).
    let paths = Paths::resolve(|key| {
        if key == paths::STATE_DIR_ENV {
            Some(state_dir.display().to_string())
        } else {
            env.get(key).cloned()
        }
    });

    // The pre-flight `--check` and a start share (AR22): everything that
    // can differ between versions on one machine, and nothing that needs
    // the network or disturbs a running instance (no lock, no port).
    let loaded = shell::profiles::load_all(&paths.profiles_dir);
    if checking {
        // Reported, not fatal: an unusable profile is a source that will
        // not be served, not a reason the process cannot start.
        for unusable in &loaded.unusable {
            eprintln!(
                "--check: profile not usable: {} — {}",
                unusable.path.display(),
                unusable.reason
            );
        }
    }
    let credentials = match load_credentials() {
        Ok(credentials) => credentials,
        Err(e) => return die(format!("{e}\n  remedy: {}", e.remedy())),
    };
    if checking {
        app.on_check(|| {
            println!("almanac {} --check: ok", env!("CARGO_PKG_VERSION"));
            Ok(())
        });
        return app.run().await;
    }

    // A real start. AR22: take the data-directory lock before anything
    // reads or writes the journal — two processes over one journal is the
    // one thing a supervised update handover must never produce.
    let _data_lock = match DataDirLock::acquire(&paths.data_dir) {
        Ok(lock) => lock,
        Err(e) => return die(e),
    };

    let http = reqwest::Client::new();
    let notifier = Notifier::from_env(http.clone());
    let unusable = loaded.unusable;
    let profile_names: Vec<String> = loaded
        .profiles
        .iter()
        .map(|p| p.source_id.clone())
        .collect();
    let profiles: HashMap<String, _> = loaded
        .profiles
        .into_iter()
        .map(|p| (p.source_id.clone(), p))
        .collect();

    // One set of counters for the whole process (M13).
    let metrics = Arc::new(almanac::core::metrics::Metrics::default());
    let tokens = TokenManager::with_metrics(http.clone(), credentials, Arc::clone(&metrics));
    let tokens = Arc::new(tokens);

    let state = Arc::new(
        AppState::new(
            profiles,
            Journal::new(paths.journal.clone(), DEFAULT_MAX_BYTES),
            GoogleCalendarClient::new(http.clone(), Arc::clone(&tokens)),
        )
        .with_profiles_dir(paths.profiles_dir.clone())
        .with_calendar_owner(
            env.get("ALMANAC_CALENDAR_OWNER")
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty()),
        )
        .with_metrics(metrics),
    );

    // Surface a damaged journal before binding rather than letting the
    // worker discover it a few seconds later.
    let pending = match state.journal.pending() {
        Ok(pending) => pending.len(),
        Err(e) => return die(e),
    };

    // AR25 ("never restart under an investigation") retired in 4.0.1 with
    // Almanac's own capture store: the kit's per-client captures (K13) are
    // in memory and lost on restart by the kit's own design, and nothing
    // durable is at stake between two ticks of the update loop.
    // D-A1 (kit 1.3.0): the update events keep reaching Home Assistant in
    // Almanac's own vocabulary (K22 / AR23 / AR24). The kit still logs each
    // event; this hook runs alongside, on the updater's task, so the send
    // is spawned rather than awaited. `update.ok` and `update.held` are
    // routine and stay out of the phone.
    {
        let notifier = notifier.clone();
        app.on_update_event(move |event| {
            use almanac::shell::notify::{Event as HaEvent, ops};
            let (op, ok) = match event.kind {
                "update.installed" => (ops::UPDATE_APPLIED, true),
                "update.rolled_back" => (ops::UPDATE_REVERTED, false),
                // The kit's `failed` covers a refused signature or checksum as
                // well as an unreachable host; AR24's "three strikes before
                // notifying" is not reproduced here — the kit says it once.
                "update.failed" => (ops::UPDATE_UNVERIFIED, false),
                _ => return,
            };
            let ha = HaEvent {
                op,
                ok,
                version: event.version.clone(),
                error: (!ok).then(|| event.detail.clone()),
            };
            let notifier = notifier.clone();
            tokio::spawn(async move { notifier.send(ha).await });
        });
    }
    // A2-1: the per-source tokens are the kit's clients. The 3.x store is
    // read once, on the first start of 4.0.0, and copied unchanged into
    // the kit's sealed file, so every source keeps its token.
    match import_source_tokens(
        &state_dir,
        &paths.token_store,
        env.get("ALMANAC_SECRET_KEY")
            .map(String::as_str)
            .unwrap_or_default(),
    )
    .await
    {
        Ok(0) => {}
        Ok(count) => eprintln!(
            "almanac: imported {count} source token(s) from the 3.x store into the kit's client store; every source keeps its token"
        ),
        Err(e) => return die(format!("{e:#}")),
    }
    // 4.0.2: the Sources page's calendar dropdown and column render
    // synchronously from a cache; fill it once now, the Calendars page
    // refreshes it on every visit.
    // Off the start path: an unreachable Google must not delay the
    // listener (N1: the port is bound within seconds, then retries happen).
    {
        let state = Arc::clone(&state);
        tokio::spawn(async move {
            match state.client.list_calendars().await {
                Ok(calendars) => {
                    let calendars =
                        state.without_deleted_calendars(state.with_created_calendars(calendars));
                    state.remember_calendars(&calendars);
                }
                Err(e) => tracing::warn!(
                    error = %e,
                    "could not list the calendars at start; the Sources page shows ids until the Calendars page is opened"
                ),
            }
        });
    }
    mount(&mut app, Arc::clone(&state));
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let worker: Arc<Mutex<Option<tokio::task::JoinHandle<()>>>> = Arc::new(Mutex::new(None));
    {
        let state = Arc::clone(&state);
        let worker = Arc::clone(&worker);
        let notifier = notifier.clone();
        let shutdown_rx = shutdown_rx.clone();
        let profiles_dir = paths.profiles_dir.clone();
        let journal_path = paths.journal.clone();
        let heartbeat_env = env.clone();
        app.on_start(move || {
            for warning in warnings {
                tracing::warn!("{warning}");
            }
            // One line per unusable file, at error level: the quietest
            // possible failure would be a count that simply came out lower
            // than expected. They are also listed on the dashboard, where
            // they can be deleted — which is why refusing to start over one
            // would be exactly the wrong move.
            for unusable in &unusable {
                tracing::error!(
                    path = %unusable.path.display(),
                    reason = %unusable.reason,
                    "a profile could not be used; this source is not being served"
                );
            }
            if profile_names.is_empty() {
                tracing::warn!(
                    directory = %profiles_dir.display(),
                    unusable = unusable.len(),
                    "no usable mapping profiles — almanac is serving no sources; add one from /sources"
                );
            } else {
                tracing::info!(
                    count = profile_names.len(),
                    unusable = unusable.len(),
                    sources = ?profile_names,
                    "loaded mapping profiles"
                );
            }
            if pending > 0 {
                tracing::info!(
                    count = pending,
                    journal = %journal_path.display(),
                    "journal holds undelivered entries from a previous run; they go out first"
                );
            }
            // M14: one line per interval, so a silent almanac and a wedged
            // one are distinguishable.
            match shell::heartbeat::interval_from(|key| heartbeat_env.get(key).cloned()) {
                Some(every) => {
                    tokio::spawn(shell::heartbeat::run(
                        Arc::clone(&state),
                        shutdown_rx.clone(),
                        every,
                    ));
                }
                None => tracing::info!(
                    "{} is 0 — no heartbeat line will be written",
                    shell::heartbeat::INTERVAL_ENV
                ),
            }

            // AR21: distinguish a broken key from an unreachable Google. A
            // malformed key never fixes itself, so exit (the supervisor
            // restarts and the log says why). A transient failure — which
            // is exactly what a power cut produces — must not park the
            // unit: keep trying, and start delivering once it works. The
            // listener is already up, so the journal accepts events
            // meanwhile.
            tokio::spawn(async move {
                let mut attempt = 0u32;
                loop {
                    match tokens.token().await {
                        Ok(_) => {
                            tracing::info!("authenticated against Google");
                            break;
                        }
                        Err(e) if !e.is_transient() => {
                            tracing::error!(
                                error = %e,
                                remedy = %e.remedy(),
                                "the Google credentials are unusable; exiting so the supervisor's log shows it"
                            );
                            eprintln!("{e}\n  remedy: {}", e.remedy());
                            std::process::exit(1);
                        }
                        Err(e) => {
                            attempt += 1;
                            let wait = STARTUP_RETRY_WAITS
                                [(attempt as usize - 1).min(STARTUP_RETRY_WAITS.len() - 1)];
                            tracing::warn!(
                                attempt,
                                wait_seconds = wait,
                                error = %e,
                                "could not reach Google yet; retrying — the service stays up and the journal \
                                 accepts events meanwhile"
                            );
                            tokio::time::sleep(std::time::Duration::from_secs(wait)).await;
                        }
                    }
                }
                let handle = tokio::spawn(shell::worker::run(state, shutdown_rx, notifier));
                *worker.lock().expect("worker slot") = Some(handle);
            });
        });
    }
    // M2: the listener has stopped accepting and in-flight requests have
    // finished. Tell the worker to drain what they journalled before the
    // process exits; the kit bounds this by its shutdown budget.
    app.on_flush(move || {
        tracing::info!("http server stopped; draining the worker");
        let _ = shutdown_tx.send(true);
        let handle = worker.lock().expect("worker slot").take();
        if let Some(handle) = handle {
            tokio::runtime::Handle::current().block_on(async {
                if let Err(e) = handle.await {
                    tracing::warn!(error = %e, "worker did not shut down cleanly");
                }
            });
        }
        tracing::info!("almanac stopped");
    });
    app.run().await
}
