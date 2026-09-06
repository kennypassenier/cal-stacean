//! The dashboard on the kit (M12 as amended 2026-09-06, S1 4.0.2): the kit
//! owns the door, the layout, the login and the Sources page — profile,
//! token and last requests in one row, made in one issue; Almanac keeps
//! its Calendars page. Ported from `dashboard_http.rs`; the login, session,
//! cookie and theme assertions are the kit's own suite now.

mod common;

use common::{KitHub, TOKEN, body_json, spawn_kit, spawn_kit_with, urlencode};

async fn sources(hub: &KitHub) -> String {
    hub.page("/clients").await
}

async fn calendars(hub: &KitHub) -> String {
    hub.page("/calendars").await
}

/// The first calendar id the Calendars page reveals (under the `id` toggle).
fn first_calendar_id(page: &str) -> String {
    page.split("<code style=\"font-size: 0.8125rem; word-break: break-all\">")
        .nth(1)
        .and_then(|rest| rest.split('<').next())
        .expect("a calendar id on the page")
        .to_string()
}

/// Add a source the 4.0.2 way: one issue on the Sources page with the
/// calendar, as the admin.
async fn add_source(hub: &KitHub, name: &str, calendar: &str) -> reqwest::Response {
    hub.bearer(reqwest::Method::POST, "/api/clients", TOKEN)
        .header("content-type", "application/json")
        .body(serde_json::json!({ "name": name, "calendar": calendar }).to_string())
        .send()
        .await
        .unwrap()
}

async fn client_id(hub: &KitHub, name: &str) -> String {
    let list = body_json(hub.get("/api/clients").await).await;
    list.as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == name)
        .and_then(|c| c["id"].as_str())
        .expect("the client is listed")
        .to_string()
}

#[tokio::test]
async fn the_kit_owns_the_door_and_the_old_addresses_keep_working() {
    let hub = spawn_kit().await;
    for path in [
        "/",
        "/calendars",
        "/clients",
        "/dashboard",
        "/dashboard/sources",
    ] {
        let response = hub.get_anon(path).await;
        assert_eq!(response.status(), 303, "{path} needs a login");
    }
    for (old, new) in [
        ("/dashboard", "/"),
        ("/dashboard/sources", "/calendars"),
        ("/sources", "/calendars"),
        ("/dashboard/captures", "/clients"),
        ("/captures", "/clients"),
    ] {
        let response = hub.get(old).await;
        assert_eq!(response.status(), 303, "{old} redirects");
        assert!(
            response.headers()["location"]
                .to_str()
                .unwrap()
                .ends_with(new),
            "{old} → {new}"
        );
    }
    let status = hub.page("/").await;
    assert!(
        status.contains("Journal")
            && status.contains("Sources")
            && status.contains("home-assistant"),
        "{status}"
    );
    let clients = hub.page("/clients").await;
    assert!(
        clients.contains("<h1>Sources</h1>"),
        "the clients page keeps Almanac's word for them"
    );
    hub.shutdown().await;
}

#[tokio::test]
async fn the_sources_page_is_the_kits_with_a_calendar_field_and_column() {
    let hub = spawn_kit().await;
    let page = sources(&hub).await;
    assert!(
        page.contains("class=\"explain\"") && page.contains("kp-nav"),
        "inside the kit's layout"
    );
    assert!(
        page.contains("id=\"field-calendar\" name=\"calendar\""),
        "the issue form asks for the calendar: {page}"
    );
    assert!(
        page.contains("href=\"&#x2f;calendars\""),
        "a Calendars page beside the Sources page (hrefs are escaped): {page}"
    );
    assert!(
        !page.contains("href=\"&#x2f;sources\""),
        "no second Sources page any more: {page}"
    );
    let token = hub.issue_client("home-assistant").await;
    let page = sources(&hub).await;
    assert!(
        page.contains("home-assistant") && page.contains("<th>Calendar</th>"),
        "the row and the calendar column: {page}"
    );
    assert!(
        !page.contains(&token),
        "the token never appears in the page"
    );
    hub.shutdown().await;
}

#[tokio::test]
async fn k21_adding_a_source_writes_its_profile_on_the_chosen_calendar() {
    let hub = spawn_kit_with(&[], Some("kenny@example.com")).await;
    // The dropdown offers what Google lists; make one first.
    let made = hub
        .form("/calendars", &format!("name={}", urlencode("Huishouden")))
        .await;
    let made_status = made.status();
    let made_body = made.text().await.unwrap();
    assert_eq!(
        made_status, 303,
        "the calendar is made and shared: {made_body}"
    );
    let page = calendars(&hub).await;
    assert!(
        page.contains("Huishouden"),
        "the new calendar is in the list: {page}"
    );
    let id = first_calendar_id(&page);
    // Visiting the Calendars page remembered the names for the Sources page.
    let page = sources(&hub).await;
    assert!(
        page.contains(&format!("<option value=\"{id}\">Huishouden</option>")),
        "the dropdown offers it by name: {page}"
    );
    // One issue makes profile and token (S1).
    let added = add_source(&hub, "job-tracker", &id).await;
    let added_status = added.status();
    let added_body = added.text().await.unwrap();
    assert_eq!(added_status, 201, "{added_body}");
    assert!(
        hub.dir.path().join("profiles/job-tracker.toml").exists(),
        "the profile is on disk"
    );
    assert!(
        hub.state.profiles().contains_key("job-tracker"),
        "loaded without a restart"
    );
    let page = sources(&hub).await;
    assert!(
        page.contains("job-tracker") && page.contains("Huishouden"),
        "the row shows the calendar by name, not its id: {page}"
    );
    assert_eq!(
        page.matches(&id).count(),
        1,
        "the calendar id appears once — as the dropdown's value, never on the row: {page}"
    );
    // A rejected name issues nothing and writes nothing.
    let rejected = add_source(&hub, ".bad name", &id).await;
    assert_eq!(rejected.status(), 400);
    let body = rejected.text().await.unwrap();
    assert!(
        body.contains("not allowed") || body.contains("cannot be a source name"),
        "{body}"
    );
    assert!(!hub.dir.path().join("profiles/.bad name.toml").exists());
    let list = body_json(hub.get("/api/clients").await).await;
    assert_eq!(
        list.as_array().unwrap().len(),
        1,
        "no token for the bad name"
    );
    // No calendar: refused too.
    let refused = add_source(&hub, "no-calendar", "").await;
    assert_eq!(refused.status(), 400);
    assert!(refused.text().await.unwrap().contains("pick the calendar"));
    hub.shutdown().await;
}

#[tokio::test]
async fn k24_without_an_owner_no_calendar_is_created_and_the_page_says_so() {
    let hub = spawn_kit().await;
    let page = calendars(&hub).await;
    assert!(page.contains("ALMANAC_CALENDAR_OWNER"), "{page}");
    let made = hub.form("/calendars", "name=Nope").await;
    assert_eq!(made.status(), 200);
    assert!(
        made.text()
            .await
            .unwrap()
            .contains("ALMANAC_CALENDAR_OWNER")
    );
    hub.shutdown().await;
}

#[tokio::test]
async fn k24_a_calendar_in_use_cannot_be_deleted_and_an_unused_one_can() {
    let hub = spawn_kit_with(&[], Some("kenny@example.com")).await;
    let made = hub.form("/calendars", "name=Werk").await;
    let made_status = made.status();
    assert_eq!(made_status, 303, "{}", made.text().await.unwrap());
    let id = first_calendar_id(&calendars(&hub).await);
    assert_eq!(add_source(&hub, "werk", &id).await.status(), 201);
    let refused = hub
        .form(&format!("/calendars/{}/delete", urlencode(&id)), "")
        .await;
    assert_eq!(refused.status(), 200);
    assert!(
        refused
            .text()
            .await
            .unwrap()
            .contains("still writes to that calendar")
    );
    // The source goes first — on the Sources page, profile and token together.
    let werk = client_id(&hub, "werk").await;
    let gone = hub
        .bearer(
            reqwest::Method::DELETE,
            &format!("/api/clients/{werk}"),
            TOKEN,
        )
        .send()
        .await
        .unwrap();
    assert_eq!(gone.status(), 204);
    assert!(!hub.dir.path().join("profiles/werk.toml").exists());
    let deleted = hub
        .form(&format!("/calendars/{}/delete", urlencode(&id)), "")
        .await;
    assert_eq!(deleted.status(), 303, "then the calendar can go");
    let page = calendars(&hub).await;
    assert!(!page.contains("Werk"), "gone from the list at once: {page}");
    hub.shutdown().await;
}

#[tokio::test]
async fn k21_deleting_a_source_removes_its_profile_and_its_token() {
    let hub = spawn_kit_with(&["home-assistant", "printer"], None).await;
    let token = hub.issue_client("printer").await;
    assert_eq!(
        hub.post_json(
            "/v1/ingest/printer",
            Some(&token),
            r#"{"external_id":"x","title":"t","start":"2026-08-28T09:00:00+00:00"}"#
        )
        .await
        .status(),
        202
    );
    // Events waiting: the kit asks Almanac first, and Almanac refuses.
    let printer = client_id(&hub, "printer").await;
    let refused = hub
        .bearer(
            reqwest::Method::DELETE,
            &format!("/api/clients/{printer}"),
            TOKEN,
        )
        .send()
        .await
        .unwrap();
    assert_eq!(refused.status(), 400);
    assert!(
        refused
            .text()
            .await
            .unwrap()
            .contains("waiting to be delivered")
    );
    assert!(hub.dir.path().join("profiles/printer.toml").exists());
    assert_eq!(
        hub.post_json("/v1/ingest/printer", Some(&token), "{}")
            .await
            .status(),
        422,
        "its token still works — nothing was deleted"
    );
    let pending = hub.state.journal.pending().unwrap();
    for entry in pending {
        hub.state.journal.mark_done(&entry.id).await.unwrap();
    }
    let deleted = hub
        .bearer(
            reqwest::Method::DELETE,
            &format!("/api/clients/{printer}"),
            TOKEN,
        )
        .send()
        .await
        .unwrap();
    assert_eq!(deleted.status(), 204);
    assert!(!hub.dir.path().join("profiles/printer.toml").exists());
    assert_eq!(
        hub.post_json("/v1/ingest/printer", Some(&token), "{}")
            .await
            .status(),
        401,
        "its token is gone with it"
    );
    assert!(!hub.page("/clients").await.contains("printer"));
    hub.shutdown().await;
}

#[tokio::test]
async fn k23_an_unusable_profile_is_listed_and_can_be_deleted_and_reload_picks_up_a_hand_written_one()
 {
    let hub = spawn_kit().await;
    std::fs::write(
        hub.dir.path().join("profiles/broken.toml"),
        "this is not = toml [",
    )
    .unwrap();
    // Reload notices the broken file; the page lists it.
    assert_eq!(hub.form("/sources/reload", "").await.status(), 303);
    let page = calendars(&hub).await;
    assert!(
        page.contains("Not being served") && page.contains("broken.toml"),
        "{page}"
    );
    assert_eq!(
        hub.form("/profiles/broken.toml/delete", "").await.status(),
        303
    );
    assert!(!hub.dir.path().join("profiles/broken.toml").exists());
    assert_eq!(
        hub.form("/profiles/broken.toml/delete", "").await.status(),
        404
    );
    // A profile placed by hand is served after a reload, no restart.
    std::fs::write(
        hub.dir.path().join("profiles/by-hand.toml"),
        common::profile_toml("by-hand"),
    )
    .unwrap();
    assert!(!hub.state.profiles().contains_key("by-hand"));
    assert_eq!(hub.form("/sources/reload", "").await.status(), 303);
    assert!(hub.state.profiles().contains_key("by-hand"));
    hub.shutdown().await;
}

#[tokio::test]
async fn a_ping_from_a_source_lands_on_its_row_as_a_last_request() {
    // A2-2 revisited (2026-09-06): Almanac's own captures page is gone;
    // the kit keeps each client's last requests on its row (K13). This is
    // what the Sources page's "Send test" button exercises.
    let hub = spawn_kit().await;
    let token = hub.issue_client("probe").await;
    let pinged = hub
        .bearer(reqwest::Method::POST, "/v1/ping", &token)
        .header("content-type", "text/plain")
        .body("<script>alert('pwned')</script>")
        .send()
        .await
        .unwrap();
    assert_eq!(pinged.status(), 200);
    assert_eq!(body_json(pinged).await["caller"], "probe");
    // The row's "Last requests" button fetches the kit's capture list for
    // that client; the page itself never holds the bodies.
    let clients = body_json(hub.get("/api/clients").await).await;
    let id = clients
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "probe")
        .and_then(|c| c["id"].as_str())
        .unwrap()
        .to_string();
    let requests = body_json(hub.get(&format!("/api/clients/{id}/requests")).await).await;
    let listed = requests.to_string();
    assert!(
        listed.contains("/v1/ping"),
        "the row shows the request: {listed}"
    );
    assert!(
        listed.contains("alert('pwned')"),
        "the body is kept verbatim in the JSON (the page escapes it): {listed}"
    );
    hub.shutdown().await;
}
