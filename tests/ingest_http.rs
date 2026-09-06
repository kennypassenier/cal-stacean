//! The ingest surface as real HTTP (K7, M7): status codes, journalling,
//! and the guarantee that a 202 is only
//! ever returned after the payload is durably journalled.
//!
//! Drives the router directly rather than binding a port — the
//! request/response path is the real one, but nothing here needs
//! Google, so these run in CI on every push. The parts that genuinely
//! need Google live in tests/power_loss_drill.rs and
//! tests/calendar_e2e.rs. The door (K6) is the kit's since 4.0.0 and is
//! proven in tests/kit_door.rs through the real chassis::App.

use std::collections::HashMap;
use std::sync::Arc;

use almanac::core::profile::Profile;
use almanac::shell::auth::TokenManager;
use almanac::shell::calendar_client::GoogleCalendarClient;
use almanac::shell::ingest::AppState;
use almanac::shell::journal::{DEFAULT_MAX_BYTES, Journal};
use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

const HA_TOKEN: &str = "home-assistant-token";
const KUMA_TOKEN: &str = "uptime-kuma-token";

fn scratch_dir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "almanac-http-{}-{}-{name}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn profile(source_id: &str) -> Profile {
    let toml = format!(
        r#"
schema_version = 2
source_id = "{source_id}"
target_calendar_id = "primary"

"#
    );
    Profile::parse(&toml, "test.toml").unwrap()
}

async fn state(dir: &std::path::Path) -> Arc<AppState> {
    let journal_path = dir.join("journal.jsonl");
    let mut profiles = HashMap::new();
    profiles.insert("home-assistant".to_string(), profile("home-assistant"));
    profiles.insert("uptime-kuma".to_string(), profile("uptime-kuma"));

    // 4.0.0: the door is the kit's; the in-process router runs every
    // request as the admin, so the tokens below are decoration.

    // Points at an unreachable host: these tests exercise the ingest
    // surface only, and the asynchronous path never calls Google.
    let http = reqwest::Client::new();
    let tokens = TokenManager::new(
        http.clone(),
        almanac::core::auth::ServiceAccountCredentials {
            client_email: "unused".to_string(),
            private_key: "unused".to_string(),
            token_url: "https://example.invalid/token".to_string(),
        },
    );

    Arc::new(AppState::new(
        profiles,
        Journal::new(journal_path, DEFAULT_MAX_BYTES),
        GoogleCalendarClient::new(http, tokens),
    ))
}

fn ha_payload() -> &'static str {
    r#"{"external_id":"switch.wasmachine","title":"Wasmachine klaar","start":"2026-08-28T09:00:00+00:00"}"#
}

fn post(uri: &str, token: Option<&str>, body: &str) -> Request<Body> {
    let mut builder = Request::builder()
        .method("POST")
        .uri(uri)
        .header("content-type", "application/json");
    if let Some(token) = token {
        builder = builder.header("authorization", format!("Bearer {token}"));
    }
    builder.body(Body::from(body.to_string())).unwrap()
}

#[tokio::test]
async fn a_valid_home_assistant_payload_is_accepted_and_journalled() {
    let dir = scratch_dir("accept");
    let state = state(&dir).await;
    let app = almanac::shell::build_router_with_probes(Arc::clone(&state));

    let response = app
        .oneshot(post(
            "/v1/ingest/home-assistant",
            Some(HA_TOKEN),
            ha_payload(),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::ACCEPTED);

    // The 202 must mean "durably stored", not "we'll get to it".
    let pending = state.journal.pending().unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].source_id, "home-assistant");
    assert_eq!(pending[0].payload["title"], "Wasmachine klaar");

    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn an_idempotency_key_header_is_recorded_on_the_journal_entry() {
    // M7: the header is what lets a source without a natural external
    // id have its retries converge instead of duplicating.
    let dir = scratch_dir("idempotency");
    let state = state(&dir).await;
    let app = almanac::shell::build_router_with_probes(Arc::clone(&state));

    let request = Request::builder()
        .method("POST")
        .uri("/v1/ingest/home-assistant")
        .header("content-type", "application/json")
        .header("authorization", format!("Bearer {HA_TOKEN}"))
        .header("idempotency-key", "run-42")
        .body(Body::from(ha_payload().to_string()))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::ACCEPTED);

    let pending = state.journal.pending().unwrap();
    assert_eq!(pending[0].idempotency_key.as_deref(), Some("run-42"));

    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn two_accepted_payloads_both_survive_in_the_journal() {
    let dir = scratch_dir("two");
    let state = state(&dir).await;

    for _ in 0..2 {
        let response = almanac::shell::build_router_with_probes(Arc::clone(&state))
            .oneshot(post(
                "/v1/ingest/home-assistant",
                Some(HA_TOKEN),
                ha_payload(),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::ACCEPTED);
    }

    assert_eq!(state.journal.pending().unwrap().len(), 2);

    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn a_journal_that_cannot_be_written_answers_500_so_the_sender_retries() {
    // AR16's one silent-data-loss path. The rule is: if the write
    // fails, say so, because Home Assistant's retry script only tries
    // again on a failure. A regression to "log it and answer 202
    // anyway" means a full disk quietly eats events while every sender
    // believes it succeeded — and nothing on the dashboard would show
    // it either.
    let dir = scratch_dir("journal-readonly");
    let readonly = dir.join("readonly");
    std::fs::create_dir_all(&readonly).unwrap();

    // A state whose journal points inside a directory nothing may
    // write to; the door is the kit's, so the request reaches the write.

    let mut profiles = HashMap::new();
    profiles.insert("home-assistant".to_string(), profile("home-assistant"));

    let http = reqwest::Client::new();
    let state = Arc::new(AppState::new(
        profiles,
        Journal::new(readonly.join("journal.jsonl"), DEFAULT_MAX_BYTES),
        GoogleCalendarClient::new(
            http.clone(),
            TokenManager::new(
                http,
                almanac::core::auth::ServiceAccountCredentials {
                    client_email: "unused".to_string(),
                    private_key: "unused".to_string(),
                    token_url: "https://example.invalid/token".to_string(),
                },
            ),
        ),
    ));

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&readonly, std::fs::Permissions::from_mode(0o500)).unwrap();
    }

    let response = almanac::shell::build_router_with_probes(Arc::clone(&state))
        .oneshot(post(
            "/v1/ingest/home-assistant",
            Some(HA_TOKEN),
            ha_payload(),
        ))
        .await
        .unwrap();

    // Restore permissions before asserting, so a failure still cleans up.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&readonly, std::fs::Permissions::from_mode(0o700)).ok();
    }
    let status = response.status();
    std::fs::remove_dir_all(&dir).ok();

    assert_eq!(
        status,
        StatusCode::INTERNAL_SERVER_ERROR,
        "an unwritable journal must be reported, never answered with 202"
    );
}

#[tokio::test]
async fn a_payload_using_every_option_is_accepted_at_the_http_layer() {
    // K9's criterion said "an E2E test per alert system", and until
    // 2.0.0 this ran the grafana and uptime-kuma fixtures through auth,
    // the router and the journal — the only place those payloads ever
    // met anything but the mapping engine.
    //
    // Those fixtures went with the translation layer they existed to
    // prove. What still matters is the same claim about the shape that
    // replaced them: a call using every per-event option must survive
    // the whole HTTP path, not just the mapper. A content type or a
    // field the ingest layer refuses would otherwise surface only when
    // a real event failed to appear.
    let dir = scratch_dir("every-option");
    let state = state(&dir).await;

    let payload = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/fixtures/payloads/everything_sample.json"
    ))
    .unwrap();

    let response = almanac::shell::build_router_with_probes(Arc::clone(&state))
        .oneshot(post("/v1/ingest/uptime-kuma", Some(KUMA_TOKEN), &payload))
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        StatusCode::ACCEPTED,
        "a payload using every option must be accepted as it is actually sent"
    );

    assert_eq!(
        state.journal.pending().unwrap().len(),
        1,
        "and it must be durably journalled"
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn a_payload_almanac_could_never_map_is_refused_at_the_door() {
    // Kenny's decision, 2026-09-03, after the JobTracker session sent a
    // date without `all_day` and got a reassuring answer. A payload that
    // can only ever fail must not be stored: it would be retried until
    // it dead-letters, in a list nobody watches.
    //
    // 422 rather than a new field in the body. HTTP already separates
    // "your request is wrong" from "my upstream broke", and almanac was
    // answering 502 for both.
    let dir = scratch_dir("unmappable");
    let state = state(&dir).await;

    let response = almanac::shell::build_router_with_probes(Arc::clone(&state))
        .oneshot(post(
            "/v1/ingest/home-assistant",
            Some(HA_TOKEN),
            r#"{"title": "Dagmarkering", "start": "2026-09-03", "external_id": "day-1"}"#,
        ))
        .await
        .unwrap();

    assert_eq!(
        response.status(),
        StatusCode::UNPROCESSABLE_ENTITY,
        "a date sent without all_day can never become an event; it must be refused, not accepted"
    );

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(
        body["remedy"]
            .as_str()
            .unwrap_or_default()
            .contains("all_day"),
        "the refusal must name the way out, not only the problem: {body}"
    );

    assert!(
        state.journal.pending().unwrap().is_empty(),
        "nothing that can never be delivered may enter the journal"
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn a_misspelled_field_is_named_instead_of_answered_with_202() {
    // The same fault from the asynchronous side. `deny_unknown_fields`
    // has always caught this, but it caught it in the delivery worker —
    // long after the sender had been told "accepted".
    let dir = scratch_dir("misspelled");
    let state = state(&dir).await;

    let response = almanac::shell::build_router_with_probes(Arc::clone(&state))
        .oneshot(post(
            "/v1/ingest/home-assistant",
            Some(HA_TOKEN),
            r#"{"title": "Tikfout", "start": "2026-09-03T10:00:00+02:00",
                "external_id": "typo-1", "allDay": true}"#,
        ))
        .await
        .unwrap();

    assert_eq!(
        response.status(),
        StatusCode::UNPROCESSABLE_ENTITY,
        "a field almanac does not know is a mistake worth naming while the sender can still act"
    );
    assert!(
        state.journal.pending().unwrap().is_empty(),
        "a payload with a misspelled field must not be journalled"
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn the_synchronous_path_refuses_an_unmappable_payload_before_journalling_it() {
    // The sync path is where the ambiguity actually hurt: it answered
    // 502 both when Google hiccuped and when the body was unusable, so
    // a caller could not tell "wait" from "waiting will never help".
    // After this, 502 on this path means Google and only Google.
    let dir = scratch_dir("sync-unmappable");
    let state = state(&dir).await;

    let response = almanac::shell::build_router_with_probes(Arc::clone(&state))
        .oneshot(post(
            "/v1/ingest/home-assistant/sync",
            Some(HA_TOKEN),
            r#"{"title": "Geen titel is geen probleem", "start": "morgenvroeg",
                "external_id": "sync-1"}"#,
        ))
        .await
        .unwrap();

    assert_eq!(
        response.status(),
        StatusCode::UNPROCESSABLE_ENTITY,
        "an unusable body must not come back as 502; that code now means Google"
    );
    assert!(
        state.journal.pending().unwrap().is_empty(),
        "the sync path must not journal what it just refused"
    );

    std::fs::remove_dir_all(&dir).ok();
}
