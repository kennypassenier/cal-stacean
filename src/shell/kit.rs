//! Almanac's glue to chassis (3.0.0): what `/healthz` and `/metrics` say,
//! in the kit's shape, with Almanac's own words and metric names. The kit
//! answers the routes; these types answer the kit.

use std::sync::Arc;

use chassis::{App, ScrapeSource, Subsystem, SubsystemStatus};

use crate::shell::ingest::AppState;

/// `journal`: readable, with the number of undelivered entries. This is
/// deliberately NOT Google's reachability (M1): the health check going red
/// during an outage Almanac rides out via the journal would be a lie about
/// Almanac's own state. The only failing answer is a journal that cannot
/// be read — the one thing that stops deliveries for good.
pub struct JournalSubsystem(pub Arc<AppState>);

impl Subsystem for JournalSubsystem {
    fn name(&self) -> &str {
        "journal"
    }

    fn check(&self) -> SubsystemStatus {
        match self.0.journal.pending() {
            Ok(pending) if pending.is_empty() => SubsystemStatus::ok("readable; nothing pending"),
            Ok(pending) => SubsystemStatus::ok(format!(
                "readable; {} undelivered entr{} waiting for the worker",
                pending.len(),
                if pending.len() == 1 { "y" } else { "ies" }
            )),
            Err(e) => {
                SubsystemStatus::failing(format!("cannot be read: {e}. What now: {}", e.remedy()))
            }
        }
    }
}

/// The `almanac_*` series (M13), appended verbatim to the kit's `/metrics`
/// so every Grafana panel keeps its query. `almanac_build_info` is the
/// kit's since 3.0.0 (same name, same label).
pub struct AlmanacMetrics(pub Arc<AppState>);

impl ScrapeSource for AlmanacMetrics {
    fn scrape(&self) -> String {
        // The journal depth is read per scrape rather than tracked, because
        // a counter of "how many are pending" drifts from the file the
        // moment a replay, a compaction or a restart touches it.
        let pending = match self.0.journal.pending() {
            Ok(entries) => Some(entries.len() as u64),
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "could not read the journal for a metrics scrape; reporting it as unreadable rather than as empty"
                );
                None
            }
        };
        self.0.metrics.render(pending, env!("CARGO_PKG_VERSION"))
    }
}

/// The Journal section on the kit's status page (A2-2): what is waiting
/// and what was just delivered.
pub struct JournalSection(pub Arc<AppState>);

impl chassis::StatusSection for JournalSection {
    fn render(&self) -> chassis::Section {
        let state = &self.0;
        let (waiting, explain) = match state.journal.pending() {
            Ok(pending) => (pending.len().to_string(), String::new()),
            Err(e) => ("unreadable".to_string(), format!(" {e} — {}", e.remedy())),
        };
        // The route ring is behind an async mutex; the status page renders
        // on the runtime, so a busy lock is reported rather than awaited.
        let recent: Vec<String> = match state.routes.try_lock() {
            Ok(routes) => routes
                .iter()
                .map(|r| {
                    let outcome = match &r.outcome {
                        crate::core::observability::RouteOutcome::Created { event_id } => {
                            format!("created {event_id}")
                        }
                        crate::core::observability::RouteOutcome::Updated { event_id } => {
                            format!("updated {event_id}")
                        }
                        crate::core::observability::RouteOutcome::Failed { message, .. } => {
                            format!("failed: {message}")
                        }
                    };
                    format!(
                        "<tr><td>{}</td><td><code>{}</code></td><td>{}</td></tr>",
                        crate::core::html::escape(&r.at),
                        crate::core::html::escape(&r.source_id),
                        crate::core::html::escape(&outcome)
                    )
                })
                .collect(),
            Err(_) => Vec::new(),
        };
        let html = if recent.is_empty() {
            "<p class=\"text-secondary\">Nothing delivered yet this run.</p>".to_string()
        } else {
            format!(
                "<table class=\"kp-table\"><thead><tr><th>When</th><th>Source</th><th>Result</th></tr></thead><tbody>{}</tbody></table>",
                recent.join("")
            )
        };
        chassis::Section {
            title: "Journal".into(),
            explain: format!(
                "Events are journalled on arrival and delivered to Google by the worker; \
                 what is waiting has not reached the calendar yet.{explain}"
            ),
            rows: vec![("Waiting to be delivered".into(), waiting)],
            html: Some(html),
        }
    }
}

/// The Profiles section on the kit's status page (A2-2): which sources
/// are served, and where their events go.
pub struct ProfilesSection(pub Arc<AppState>);

impl chassis::StatusSection for ProfilesSection {
    fn render(&self) -> chassis::Section {
        let loaded = self.0.profiles();
        let mut profiles: Vec<_> = loaded.values().collect();
        profiles.sort_by(|a, b| a.source_id.cmp(&b.source_id));
        let rows = profiles
            .iter()
            .map(|p| (p.source_id.clone(), p.target_calendar_id.clone()))
            .collect();
        chassis::Section {
            title: "Sources".into(),
            explain: "Every loaded mapping profile and the calendar it writes to; manage them on the Sources pages.".into(),
            rows,
            html: Some("<p><a class=\"kp-button\" href=\"/sources\">Profiles and calendars</a> <a class=\"kp-button\" href=\"/clients\">Tokens</a></p>".into()),
        }
    }
}

/// A2-1 · one-time import of the 3.x per-source tokens into the kit's
/// client store, so JobTracker and every other source keep their tokens.
///
/// Runs before the kit opens the store; does nothing once
/// `clients.json.enc` exists or when there is no 3.x store to read, so
/// it is idempotent across restarts. The tokens are copied unchanged.
pub async fn import_source_tokens(
    state_dir: &std::path::Path,
    token_store: &std::path::Path,
    key_hex: &str,
) -> Result<usize, String> {
    use chassis::core::clients::{Client, ClientsFile};
    use chassis::shell::store::{ClientStore, EncryptedFile, FileClientStore};

    let file = state_dir.join("clients.json.enc");
    if file.exists() || !token_store.exists() {
        return Ok(0);
    }
    let key_bytes: [u8; 32] = hex::decode(key_hex)
        .ok()
        .and_then(|bytes| bytes.try_into().ok())
        .ok_or_else(|| "ALMANAC_SECRET_KEY must be 64 hex characters".to_string())?;
    let store = crate::shell::token_store::TokenStore::with_key_loading(
        token_store.to_path_buf(),
        key_bytes,
    )
    .map_err(|e| format!("{e}: {}", e.remedy()))?;
    let mut clients = Vec::new();
    for (source_id, issued_at) in store.list().await {
        if let Some(token) = store.reveal(&source_id).await.map_err(|e| e.to_string())? {
            clients.push(Client {
                id: format!("source-{source_id}"),
                name: source_id,
                token: Some(token),
                issued_at,
                revoked_at: None,
                last_used_at: None,
                uses: 0,
            });
        }
    }
    if clients.is_empty() {
        return Ok(0);
    }
    let kit_key = chassis::core::crypto::Key::parse_hex("ALMANAC_SECRET_KEY", key_hex, key_hex)
        .map_err(|e| e.to_string())?;
    let kit_store = FileClientStore::open(EncryptedFile::new(file, kit_key, "clients"))
        .map_err(|e| e.to_string())?;
    let count = clients.len();
    kit_store
        .update(&mut |clients_file: &mut ClientsFile| {
            clients_file.clients.extend(clients.iter().cloned());
            Ok(clients.last().cloned().expect("at least one source"))
        })
        .map_err(|e| e.to_string())?;
    Ok(count)
}

/// Everything Almanac hangs on the kit, in one place, so the binary and the
/// in-process test harness assemble the same service (4.0.0).
pub fn mount(app: &mut App, state: Arc<AppState>) {
    app.subsystem(JournalSubsystem(Arc::clone(&state)));
    app.metrics_source(AlmanacMetrics(Arc::clone(&state)));
    // The machine API behind the kit's door (per-source client tokens, the
    // admin's login token), the pages behind the admin login (A2-2): `/`
    // is the kit's status page with a Journal and a Sources section, the
    // profiles and calendars live on /sources, and the tokens — with each
    // source's last requests (K13) — on the kit's clients page, labelled
    // Sources. Almanac's own captures page went in 4.0.1 (A2-2 revisited).
    app.api_routes(crate::shell::build_router(Arc::clone(&state)));
    app.dashboard_routes(crate::shell::pages(Arc::clone(&state)));
    app.nav_entry("Sources", "/sources");
    app.clients_label("Sources");
    app.status_section(JournalSection(Arc::clone(&state)));
    app.status_section(ProfilesSection(state));
    // "Send test" on the Sources page posts a ping with that client's token;
    // the round trip shows under the row's Last requests (K13): "does my
    // token work?" has a button.
    app.test_route(
        "POST",
        "/v1/ping",
        "application/json",
        r#"{"hello":"from the dashboard"}"#,
    );
}
