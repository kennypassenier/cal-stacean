//! The door and the debug surfaces on the kit (4.0.0, A2-1/A2-3): a
//! source posts with the client token issued under its own name, the
//! admin's login token opens the debug views, a capture can be posted with
//! any client token and read back only by the admin. Ported from
//! `ingest_http.rs` and `admin_http.rs`, which mounted Almanac's own door.

mod common;

use common::{KEY, TOKEN, body_json, spawn_kit, spawn_kit_in, spawn_kit_with};

fn ha_payload() -> &'static str {
    r#"{"external_id":"switch.wasmachine","title":"Wasmachine klaar","start":"2026-08-28T09:00:00+00:00"}"#
}

#[tokio::test]
async fn a_source_posts_with_its_own_client_token_and_nothing_else() {
    let hub = spawn_kit_with(&["home-assistant", "uptime-kuma"], None).await;
    let ha = hub.issue_client("home-assistant").await;
    let kuma = hub.issue_client("uptime-kuma").await;
    // No token: the kit refuses before Almanac sees the request.
    let refused = hub
        .post_json("/v1/ingest/home-assistant", None, ha_payload())
        .await;
    assert_eq!(refused.status(), 401);
    assert!(
        hub.state.journal.pending().unwrap().is_empty(),
        "nothing journalled"
    );
    // A wrong token: the same.
    let wrong = hub
        .post_json(
            "/v1/ingest/home-assistant",
            Some("not-a-token"),
            ha_payload(),
        )
        .await;
    assert_eq!(wrong.status(), 401);
    // Another source's token: the kit lets it in, Almanac says no (K6).
    let foreign = hub
        .post_json("/v1/ingest/home-assistant", Some(&kuma), ha_payload())
        .await;
    assert_eq!(foreign.status(), 401);
    let json = body_json(foreign).await;
    assert!(
        json["remedy"]
            .as_str()
            .unwrap_or_default()
            .contains("Sources"),
        "{json}"
    );
    // An unknown source with a good token answers the same 401.
    let unknown = hub
        .post_json("/v1/ingest/nope", Some(&ha), ha_payload())
        .await;
    assert_eq!(unknown.status(), 401);
    // The right token: 202, and the 202 means durably journalled.
    let accepted = hub
        .post_json("/v1/ingest/home-assistant", Some(&ha), ha_payload())
        .await;
    assert_eq!(accepted.status(), 202);
    let pending = hub.state.journal.pending().unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].source_id, "home-assistant");
    // The admin's login token may post as any source (Kenny's scripts).
    let admin = hub
        .post_json("/v1/ingest/uptime-kuma", Some(TOKEN), ha_payload())
        .await;
    assert_eq!(admin.status(), 202);
    hub.shutdown().await;
}

#[tokio::test]
async fn the_3x_source_tokens_are_imported_once_and_keep_working() {
    // A2-1: JobTracker and every other source keep the token 3.x issued.
    let dir = tempfile::tempdir().unwrap();
    let profiles_dir = dir.path().join("profiles");
    std::fs::create_dir_all(&profiles_dir).unwrap();
    std::fs::write(
        profiles_dir.join("job-tracker.toml"),
        common::profile_toml("job-tracker"),
    )
    .unwrap();
    let old_token = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    {
        let key: [u8; 32] = hex::decode(KEY).unwrap().try_into().unwrap();
        let store =
            almanac::shell::token_store::TokenStore::with_key(dir.path().join("tokens.json"), key);
        store
            .issue("job-tracker", old_token, "2026-09-01T10:00:00+00:00")
            .await
            .unwrap();
    }
    let hub = spawn_kit_in(dir, None).await;
    let accepted = hub
        .post_json("/v1/ingest/job-tracker", Some(old_token), ha_payload())
        .await;
    assert_eq!(
        accepted.status(),
        202,
        "the 3.x token posts on 4.0.0 unchanged"
    );
    let sources = hub.page("/clients").await;
    assert!(
        sources.contains("job-tracker"),
        "the imported source is on the kit's Sources page"
    );
    assert!(
        !sources.contains(old_token),
        "the token never appears in the page HTML"
    );
    let page = hub.page("/sources").await;
    assert!(
        page.contains("issued"),
        "the profile shows its token as issued: {page}"
    );
    assert!(hub.dir.path().join("clients.json.enc").exists());
    hub.shutdown().await;
}

#[tokio::test]
async fn the_debug_views_need_the_admin_and_captures_take_any_client() {
    let hub = spawn_kit().await;
    let client = hub.issue_client("probe").await;
    // Status and dry-run: the admin only.
    assert_eq!(hub.get_anon("/v1/debug/status").await.status(), 401);
    let as_client = hub
        .bearer(reqwest::Method::GET, "/v1/debug/status", &client)
        .send()
        .await
        .unwrap();
    assert_eq!(as_client.status(), 403, "a client token is not the admin");
    let as_admin = hub
        .bearer(reqwest::Method::GET, "/v1/debug/status", TOKEN)
        .send()
        .await
        .unwrap();
    assert_eq!(as_admin.status(), 200);
    let status = body_json(as_admin).await;
    assert_eq!(status["profiles"][0]["source_id"], "home-assistant");
    // A capture: any client may post one, credentials are redacted.
    let posted = hub
        .bearer(
            reqwest::Method::POST,
            "/v1/debug/capture/webhook-x",
            &client,
        )
        .header("content-type", "application/json")
        .header("x-api-key", "very-secret")
        .body(r#"{"raw":true}"#)
        .send()
        .await
        .unwrap();
    assert_eq!(posted.status(), 200, "{}", posted.text().await.unwrap());
    // Reading them back: the admin only.
    let list_as_client = hub
        .bearer(reqwest::Method::GET, "/v1/debug/capture", &client)
        .send()
        .await
        .unwrap();
    assert_eq!(list_as_client.status(), 403);
    let list = hub
        .bearer(reqwest::Method::GET, "/v1/debug/capture", TOKEN)
        .send()
        .await
        .unwrap();
    assert_eq!(list.status(), 200);
    let captures = body_json(list).await;
    assert_eq!(captures["captures"][0]["label"], "webhook-x");
    assert_eq!(captures["captures"][0]["body"], r#"{"raw":true}"#);
    let headers = captures["captures"][0]["headers"].to_string();
    assert!(
        headers.contains("<redacted>") && !headers.contains("very-secret"),
        "{headers}"
    );
    // And the page shows it.
    let page = hub.page("/captures").await;
    assert!(page.contains("webhook-x") && !page.contains("very-secret"));
    // Health and metrics stay open for monitoring.
    assert_eq!(hub.get_anon("/healthz").await.status(), 200);
    assert_eq!(hub.get_anon("/metrics").await.status(), 200);
    hub.shutdown().await;
}
