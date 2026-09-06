//! The dashboard on the kit (M12 as amended 2026-09-06): the kit owns the
//! door, the layout, the login and the Sources (tokens) page; Almanac keeps
//! its profiles, calendars and captures pages. Ported from
//! `dashboard_http.rs`; the login, session, cookie and theme assertions
//! are the kit's own suite now.

mod common;

use common::{KitHub, spawn_kit, spawn_kit_with, urlencode};

async fn sources(hub: &KitHub) -> String {
    hub.page("/sources").await
}

#[tokio::test]
async fn the_kit_owns_the_door_and_the_old_addresses_keep_working() {
    let hub = spawn_kit().await;
    for path in [
        "/",
        "/sources",
        "/captures",
        "/clients",
        "/dashboard",
        "/dashboard/sources",
    ] {
        let response = hub.get_anon(path).await;
        assert_eq!(response.status(), 303, "{path} needs a login");
    }
    for (old, new) in [
        ("/dashboard", "/"),
        ("/dashboard/sources", "/sources"),
        ("/dashboard/captures", "/captures"),
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
async fn the_sources_page_lists_profiles_and_their_token_state() {
    let hub = spawn_kit().await;
    let page = sources(&hub).await;
    assert!(
        page.contains("class=\"explain\"") && page.contains("kp-nav"),
        "inside the kit's layout"
    );
    assert!(
        page.contains("home-assistant") && page.contains("no token"),
        "{page}"
    );
    let token = hub.issue_client("home-assistant").await;
    let page = sources(&hub).await;
    assert!(page.contains("issued"), "{page}");
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
    let page = sources(&hub).await;
    assert!(
        page.contains("Huishouden"),
        "the new calendar is in the list: {page}"
    );
    let id = page
        .split("<option value=\"")
        .skip(1)
        .filter_map(|part| part.split('"').next())
        .find(|v| !v.is_empty())
        .expect("a calendar option")
        .to_string();
    let added = hub
        .form(
            "/sources",
            &format!("source_id=job-tracker&calendar={}", urlencode(&id)),
        )
        .await;
    let added_status = added.status();
    let added_body = added.text().await.unwrap();
    assert_eq!(added_status, 303, "{added_body}");
    assert!(
        hub.dir.path().join("profiles/job-tracker.toml").exists(),
        "the profile is on disk"
    );
    let page = sources(&hub).await;
    assert!(
        page.contains("job-tracker") && page.contains("no token"),
        "served, waiting for its token"
    );
    assert!(
        hub.state.profiles().contains_key("job-tracker"),
        "loaded without a restart"
    );
    // A rejected name keeps what was typed and writes nothing.
    let rejected = hub
        .form(
            "/sources",
            &format!(
                "source_id={}&calendar={}",
                urlencode(".bad name"),
                urlencode(&id)
            ),
        )
        .await;
    assert_eq!(rejected.status(), 200);
    let body = rejected.text().await.unwrap();
    assert!(
        body.contains("cannot be a source name") && body.contains(".bad name"),
        "{body}"
    );
    assert!(!hub.dir.path().join("profiles/.bad name.toml").exists());
    hub.shutdown().await;
}

#[tokio::test]
async fn k24_without_an_owner_no_calendar_is_created_and_the_page_says_so() {
    let hub = spawn_kit().await;
    let page = sources(&hub).await;
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
    let page = sources(&hub).await;
    let id = page
        .split("<option value=\"")
        .skip(1)
        .filter_map(|part| part.split('"').next())
        .find(|v| !v.is_empty())
        .expect("a calendar option")
        .to_string();
    assert_eq!(
        hub.form(
            "/sources",
            &format!("source_id=werk&calendar={}", urlencode(&id))
        )
        .await
        .status(),
        303
    );
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
    assert_eq!(
        hub.form("/sources/werk/delete", "").await.status(),
        303,
        "the source goes first"
    );
    let deleted = hub
        .form(&format!("/calendars/{}/delete", urlencode(&id)), "")
        .await;
    assert_eq!(deleted.status(), 303, "then the calendar can go");
    let page = sources(&hub).await;
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
    // Refused while the journal still holds its events.
    let refused = hub.form("/sources/printer/delete", "").await;
    assert_eq!(refused.status(), 200);
    assert!(
        refused
            .text()
            .await
            .unwrap()
            .contains("waiting to be delivered")
    );
    // Drain the journal by hand (the worker is not running in the harness).
    let pending = hub.state.journal.pending().unwrap();
    for entry in pending {
        hub.state.journal.mark_done(&entry.id).await.unwrap();
    }
    let deleted = hub.form("/sources/printer/delete", "").await;
    assert_eq!(deleted.status(), 303);
    assert!(!hub.dir.path().join("profiles/printer.toml").exists());
    assert_eq!(
        hub.post_json("/v1/ingest/printer", Some(&token), "{}")
            .await
            .status(),
        401,
        "its token is gone with it"
    );
    assert!(!hub.page("/clients").await.contains("printer"));
    // Deleting what is not there is a 404.
    assert_eq!(hub.form("/sources/printer/delete", "").await.status(), 404);
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
    let page = sources(&hub).await;
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
async fn a_captured_script_tag_renders_inert_and_the_test_button_posts_a_capture() {
    let hub = spawn_kit().await;
    let token = hub.issue_client("probe").await;
    let posted = hub
        .bearer(reqwest::Method::POST, "/v1/debug/capture/xss", &token)
        .header("content-type", "text/plain")
        .body("<script>alert('pwned')</script>")
        .send()
        .await
        .unwrap();
    assert_eq!(posted.status(), 200);
    let page = hub.page("/captures").await;
    assert!(!page.contains("<script>alert"), "escaped: {page}");
    assert!(page.contains("&lt;script&gt;"));
    hub.shutdown().await;
}
