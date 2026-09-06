//! The operator surface as real HTTP: health (M1), debug status (K11),
//! raw capture (M11) and dry-run (M9), driven in-process as the admin.
//! None of these need Google, so they run in CI on every push. The door
//! (who may call what) is the kit's since 4.0.0: tests/kit_door.rs.

use std::collections::HashMap;
use std::sync::Arc;

use almanac::core::profile::Profile;
use almanac::shell::auth::TokenManager;
use almanac::shell::calendar_client::GoogleCalendarClient;
use almanac::shell::ingest::AppState;
use almanac::shell::journal::{DEFAULT_MAX_BYTES, Journal};
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use tower::ServiceExt;

const ADMIN_TOKEN: &str = "bootstrap-admin-token";

fn scratch_dir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "almanac-admin-{}-{}-{name}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn state(dir: &std::path::Path, _admin: Option<&str>) -> Arc<AppState> {
    let journal_path = dir.join("journal.jsonl");
    let toml = r#"
schema_version = 2
source_id = "home-assistant"
target_calendar_id = "primary"

"#;
    let mut profiles = HashMap::new();
    profiles.insert(
        "home-assistant".to_string(),
        Profile::parse(toml, "test.toml").unwrap(),
    );

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

/// State with a capture-only token as well (S2).
fn get(uri: &str, token: Option<&str>) -> Request<Body> {
    let mut builder = Request::builder().method("GET").uri(uri);
    if let Some(token) = token {
        builder = builder.header("authorization", format!("Bearer {token}"));
    }
    builder.body(Body::empty()).unwrap()
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

async fn body_json(response: axum::response::Response) -> serde_json::Value {
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn health_answers_without_a_token() {
    // M1: a monitoring stack that fails closed lies to you during an
    // outage, so this must never require credentials.
    let dir = scratch_dir("health");
    let app = almanac::shell::build_router_with_probes(state(&dir, Some(ADMIN_TOKEN)));

    let response = app.oneshot(get("/healthz", None)).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(body_json(response).await["status"], "ok");

    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn the_debug_status_reports_profiles_and_the_journal() {
    let dir = scratch_dir("status");
    let app = almanac::shell::build_router_with_probes(state(&dir, Some(ADMIN_TOKEN)));

    let response = app
        .oneshot(get("/v1/debug/status", Some(ADMIN_TOKEN)))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body = body_json(response).await;
    assert_eq!(body["profiles"][0]["source_id"], "home-assistant");
    assert_eq!(body["profiles"][0]["target_calendar_id"], "primary");
    assert_eq!(body["journal"]["count"], 0);

    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn a_captured_request_reads_back_verbatim() {
    // M11's whole purpose: learn an undocumented webhook's real shape.
    // If the body came back altered, the profile written from it would
    // be wrong.
    let dir = scratch_dir("capture");
    let st = state(&dir, Some(ADMIN_TOKEN));
    let payload = r#"{"weird":{"nested":[1,2,3]},"unicode":"héllo"}"#;

    let posted = almanac::shell::build_router_with_probes(Arc::clone(&st))
        .oneshot(post(
            "/v1/debug/capture/unknown-app",
            Some(ADMIN_TOKEN),
            payload,
        ))
        .await
        .unwrap();
    assert_eq!(posted.status(), StatusCode::OK);

    let listed = almanac::shell::build_router_with_probes(Arc::clone(&st))
        .oneshot(get("/v1/debug/capture", Some(ADMIN_TOKEN)))
        .await
        .unwrap();
    let body = body_json(listed).await;

    assert_eq!(body["captures"][0]["label"], "unknown-app");
    assert_eq!(
        body["captures"][0]["body"].as_str().unwrap(),
        payload,
        "the body must come back byte-identical"
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn a_captured_authorization_header_is_redacted() {
    // Capturing an unknown webhook means capturing whatever it sends,
    // including its own credentials. Those must not become readable
    // afterwards just because someone pointed it here.
    let dir = scratch_dir("capture-redact");
    let st = state(&dir, Some(ADMIN_TOKEN));

    let posted = almanac::shell::build_router_with_probes(Arc::clone(&st))
        .oneshot(post("/v1/debug/capture/x", Some(ADMIN_TOKEN), "{}"))
        .await
        .unwrap();
    assert_eq!(posted.status(), StatusCode::OK);

    let listed = almanac::shell::build_router_with_probes(Arc::clone(&st))
        .oneshot(get("/v1/debug/capture", Some(ADMIN_TOKEN)))
        .await
        .unwrap();
    let raw = serde_json::to_string(&body_json(listed).await).unwrap();

    assert!(
        !raw.contains(ADMIN_TOKEN),
        "no captured header may echo a bearer token back:\n{raw}"
    );
    assert!(
        raw.contains("<redacted>"),
        "and it must say it redacted one"
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn dry_run_shows_the_event_without_writing_it() {
    // M9: check a profile against a real payload before letting it
    // near a calendar.
    let dir = scratch_dir("dryrun");
    let app = almanac::shell::build_router_with_probes(state(&dir, Some(ADMIN_TOKEN)));

    let response = app
        .oneshot(post(
            "/v1/debug/dry-run/home-assistant",
            Some(ADMIN_TOKEN),
            r#"{"external_id":"switch.wasmachine","title":"Wasmachine klaar","start":"2026-08-28T09:00:00+00:00"}"#,
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    assert_eq!(body["would_write_to_calendar"], "primary");
    assert_eq!(body["event"]["summary"], "Wasmachine klaar");
    assert_eq!(
        body["event"]["end"]["dateTime"],
        "2026-08-28T10:00:00+00:00"
    );
    assert_eq!(
        body["event"]["extendedProperties"]["private"]["almanac_source_id"],
        "home-assistant:switch.wasmachine"
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn dry_run_explains_a_payload_the_profile_cannot_map() {
    let dir = scratch_dir("dryrun-bad");
    let app = almanac::shell::build_router_with_probes(state(&dir, Some(ADMIN_TOKEN)));

    let response = app
        .oneshot(post(
            "/v1/debug/dry-run/home-assistant",
            Some(ADMIN_TOKEN),
            r#"{"title":"no start field here"}"#,
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body = body_json(response).await;
    assert!(body["message"].as_str().unwrap().contains("start"));
    assert!(!body["remedy"].as_str().unwrap().is_empty());

    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn dry_run_on_an_unknown_source_says_where_to_look() {
    let dir = scratch_dir("dryrun-unknown");
    let app = almanac::shell::build_router_with_probes(state(&dir, Some(ADMIN_TOKEN)));

    let response = app
        .oneshot(post("/v1/debug/dry-run/nope", Some(ADMIN_TOKEN), "{}"))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert!(
        body_json(response).await["remedy"]
            .as_str()
            .unwrap()
            .contains("/v1/debug/status")
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn captures_past_the_capacity_drop_the_oldest_through_the_real_endpoints() {
    // T13: the cap was tested as a pure ring buffer, never through the
    // wiring. That is exactly the class of bug that let a forgotten
    // capture disable self-update for months — the function was right,
    // the place it was called from was not.
    let dir = scratch_dir("capture-cap");
    let state = state(&dir, Some(ADMIN_TOKEN));

    for i in 0..105 {
        almanac::shell::build_router_with_probes(Arc::clone(&state))
            .oneshot(post(
                &format!("/v1/debug/capture/label-{i}"),
                Some(ADMIN_TOKEN),
                &format!(r#"{{"n":{i}}}"#),
            ))
            .await
            .unwrap();
    }

    let response = almanac::shell::build_router_with_probes(Arc::clone(&state))
        .oneshot(get("/v1/debug/capture", Some(ADMIN_TOKEN)))
        .await
        .unwrap();
    let body = body_json(response).await;
    let captures = body["captures"].as_array().unwrap();

    assert_eq!(
        captures.len(),
        100,
        "the cap must hold through the endpoints"
    );
    assert_eq!(
        captures[0]["label"], "label-104",
        "newest first, and the oldest five are gone"
    );
    assert!(
        !captures.iter().any(|c| c["label"] == "label-0"),
        "the oldest must actually have been dropped"
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn every_credential_header_a_webhook_might_send_is_redacted() {
    // Capturing an unknown webhook means capturing whatever it sends,
    // including its own credentials. The only test here asserted the
    // redaction list was lowercase; nothing proved a real credential
    // header actually gets redacted, or that the check is
    // case-insensitive against what a real sender writes.
    let dir = scratch_dir("capture-redact-all");
    let state = state(&dir, Some(ADMIN_TOKEN));

    let request = Request::builder()
        .method("POST")
        .uri("/v1/debug/capture/x")
        .header("authorization", format!("Bearer {ADMIN_TOKEN}"))
        .header("Cookie", "session=super-secret")
        .header("X-Api-Key", "vendor-api-key-value")
        .header("Proxy-Authorization", "Basic abc123")
        .body(Body::from("{}"))
        .unwrap();

    almanac::shell::build_router_with_probes(Arc::clone(&state))
        .oneshot(request)
        .await
        .unwrap();

    let response = almanac::shell::build_router_with_probes(Arc::clone(&state))
        .oneshot(get("/v1/debug/capture", Some(ADMIN_TOKEN)))
        .await
        .unwrap();
    let rendered = serde_json::to_string(&body_json(response).await).unwrap();

    for secret in [
        "super-secret",
        "vendor-api-key-value",
        "abc123",
        ADMIN_TOKEN,
    ] {
        assert!(
            !rendered.contains(secret),
            "a credential header was stored verbatim: {secret} in {rendered}"
        );
    }

    std::fs::remove_dir_all(&dir).ok();
}

// ---------------------------------------------------------------
// M13 — the Prometheus scrape target.
// ---------------------------------------------------------------

async fn body_text(response: axum::response::Response) -> String {
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    String::from_utf8(bytes.to_vec()).unwrap()
}

#[tokio::test]
async fn metrics_answer_without_a_token_so_monitoring_cannot_fail_closed() {
    // M12's rule, and the practical reason for it: a scraper that
    // cannot authenticate reports the service as down. An admin token
    // *is* configured here, so this proves the endpoint is deliberately
    // open rather than merely open by accident.
    let dir = scratch_dir("metrics-open");
    let app = almanac::shell::build_router_with_probes(state(&dir, Some(ADMIN_TOKEN)));

    let response = app.oneshot(get("/metrics", None)).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    assert!(
        content_type.starts_with("text/plain"),
        "the exposition format must not be served as JSON: {content_type}"
    );

    let body = body_text(response).await;
    assert!(body.contains("almanac_events_accepted_total"));
    assert!(body.contains("almanac_journal_pending 0\n"));

    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn a_scrape_never_carries_a_token_a_calendar_id_or_payload_content() {
    // The M13 acceptance criterion, asserted against a state that
    // holds all three: a source token, a calendar id, and a payload
    // with a household detail in it. A metrics database keeps what it
    // scrapes for years and renders it on a dashboard, so a leak here
    // is not a leak that can be taken back.
    let dir = scratch_dir("metrics-no-secrets");
    let state = state(&dir, Some(ADMIN_TOKEN));
    // 4.0.0: the tokens are the kit's and never touch this state; the
    // bearer below is decoration for the in-process router.
    let token = "a-source-token-that-must-never-be-scraped";

    // Accept a real payload so the counters are non-zero and the
    // journal has something in it.
    let app = almanac::shell::build_router_with_probes(Arc::clone(&state));
    let accepted = app
        .oneshot(post(
            "/v1/ingest/home-assistant",
            Some(token),
            r#"{"title":"Sarah's dentist appointment","external_id":"binary_sensor.hallway",
                "start":"2026-08-29T10:00:00Z"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(accepted.status(), StatusCode::ACCEPTED);

    let app = almanac::shell::build_router_with_probes(Arc::clone(&state));
    let body = body_text(app.oneshot(get("/metrics", None)).await.unwrap()).await;

    assert!(
        body.contains("almanac_events_accepted_total 1\n"),
        "the acceptance was not counted, so this test would pass vacuously:\n{body}"
    );
    assert!(body.contains("almanac_journal_pending 1\n"));

    for secret in [
        token,
        "primary",
        "Sarah's dentist appointment",
        "binary_sensor.hallway",
        "home-assistant",
    ] {
        assert!(
            !body.contains(secret),
            "a scrape carried {secret:?}:\n{body}"
        );
    }

    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn an_unreadable_journal_is_reported_as_unreadable_not_as_empty() {
    // A directory where the journal file should be: reading it fails.
    // The dangerous rendering is "0 pending" — a flat, green backlog
    // graph on a hub that has quietly stopped being able to tell.
    let dir = scratch_dir("metrics-broken-journal");
    let state = state(&dir, Some(ADMIN_TOKEN));
    std::fs::create_dir_all(dir.join("journal.jsonl")).unwrap();

    let app = almanac::shell::build_router_with_probes(state);
    let body = body_text(app.oneshot(get("/metrics", None)).await.unwrap()).await;

    assert!(body.contains("almanac_journal_readable 0\n"));
    assert!(
        !body.contains("almanac_journal_pending 0"),
        "an unreadable journal must not look like an empty one:\n{body}"
    );

    std::fs::remove_dir_all(&dir).ok();
}
