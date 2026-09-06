//! L3's hardest exit criterion, against a REAL calendar (standing
//! rules 9 and 15): a crash between "delivered to Google" and
//! "recorded as delivered" must lose nothing and duplicate nothing.
//!
//! That window is the dangerous one. Crashing *before* delivery is
//! easy — the entry is still pending and simply goes out later.
//! Crashing *after* delivery but *before* the journal records it is
//! the case where a naive design creates the event twice: replay sees
//! an undelivered entry that was, in fact, already delivered.
//!
//! The drill reproduces exactly that by delivering an entry and then
//! deliberately not marking it done — which is precisely the on-disk
//! state a `kill -9` in that window leaves behind — and then replaying
//! the way startup does.
//!
//! Requires the scratch calendar and credentials (see
//! tests/calendar_e2e.rs); run with:
//!   latch run -- cargo test --test power_loss_drill -- --ignored

use std::collections::HashMap;
use std::sync::Arc;

use almanac::core::journal::Entry;
use almanac::core::profile::Profile;
use almanac::shell::auth::{TokenManager, load_credentials};
use almanac::shell::calendar_client::GoogleCalendarClient;
use almanac::shell::delivery::UPSERT_PROPERTY;
use almanac::shell::ingest::AppState;
use almanac::shell::journal::{DEFAULT_MAX_BYTES, Journal};
use serde_json::json;

fn scratch_dir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "almanac-drill-{}-{}-{name}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn profile_for(calendar_id: &str) -> Profile {
    let toml = format!(
        r#"
schema_version = 2
source_id = "drill-source"
target_calendar_id = "{calendar_id}"

"#
    );
    Profile::parse(&toml, "drill.toml").unwrap()
}

fn state_for(calendar_id: &str, journal_path: std::path::PathBuf) -> Arc<AppState> {
    let credentials = load_credentials().expect("service-account credentials via latch run");
    let http = reqwest::Client::new();
    let tokens = TokenManager::new(http.clone(), credentials);

    let mut profiles = HashMap::new();
    profiles.insert("drill-source".to_string(), profile_for(calendar_id));

    Arc::new(AppState::new(
        profiles,
        Journal::new(journal_path, DEFAULT_MAX_BYTES),
        GoogleCalendarClient::new(http, tokens),
    ))
}

#[tokio::test]
#[ignore = "requires ALMANAC_TEST_CALENDAR_ID and Google service-account credentials via latch run"]
async fn a_crash_between_delivery_and_bookkeeping_loses_nothing_and_duplicates_nothing() {
    let calendar_id = std::env::var("ALMANAC_TEST_CALENDAR_ID")
        .expect("set ALMANAC_TEST_CALENDAR_ID to the almanac-test calendar's id");
    let dir = scratch_dir("crash");
    let journal_path = dir.join("journal.jsonl");

    // A marker unique to this run, so a previous run's leftovers can
    // never make this one pass or fail spuriously.
    let entity_id = format!(
        "drill-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let upsert_key = format!("drill-source:{entity_id}");

    let entry = Entry {
        id: "entry-1".to_string(),
        source_id: "drill-source".to_string(),
        received_at: "2026-08-28T09:00:00+00:00".to_string(),
        payload: json!({
            "external_id": entity_id,
            "title": "power-loss drill",
            "start": "2026-08-28T09:00:00+00:00",
        }),
        idempotency_key: None,
    };

    // ---- First run: accept and deliver, then "crash" ---------------
    let state = state_for(&calendar_id, journal_path.clone());
    state.journal.accept(&entry).await.expect("accept");

    let delivered = almanac::shell::worker::drain_once(&state).await;
    assert_eq!(delivered, 1, "the entry should have been delivered once");

    let after_first = state
        .client
        .list_events_by_property(&calendar_id, UPSERT_PROPERTY, &upsert_key)
        .await
        .expect("list after first delivery");
    assert_eq!(after_first.len(), 1, "exactly one event after delivery");
    let first_event_id = after_first[0].id.clone().expect("event id");

    // The crash: drain_once marked it done, so undo exactly that one
    // record to reproduce the dangerous on-disk state — delivered to
    // Google, not yet recorded as delivered.
    let contents = std::fs::read_to_string(&journal_path).expect("read journal");
    let without_done: String = contents
        .lines()
        .filter(|line| !line.contains("\"done\""))
        .map(|line| format!("{line}\n"))
        .collect();
    std::fs::write(&journal_path, without_done).expect("rewrite journal");

    // ---- Second run: restart replays the "undelivered" entry -------
    let restarted = state_for(&calendar_id, journal_path.clone());
    let pending = restarted.journal.pending().expect("pending after restart");
    assert_eq!(
        pending.len(),
        1,
        "the entry must still look pending after the crash — nothing lost"
    );

    let redelivered = almanac::shell::worker::drain_once(&restarted).await;
    assert_eq!(redelivered, 1, "replay should redeliver the entry");

    // ---- The actual claim ------------------------------------------
    let after_replay = restarted
        .client
        .list_events_by_property(&calendar_id, UPSERT_PROPERTY, &upsert_key)
        .await
        .expect("list after replay");
    assert_eq!(
        after_replay.len(),
        1,
        "replay must converge on the same event, not create a second one"
    );
    assert_eq!(
        after_replay[0].id.as_deref(),
        Some(first_event_id.as_str()),
        "the surviving event must be the one the first run created"
    );

    assert!(
        restarted.journal.pending().expect("pending").is_empty(),
        "after a successful replay nothing should remain pending"
    );

    // ---- Clean up the scratch calendar ------------------------------
    restarted
        .client
        .delete_event(&calendar_id, &first_event_id)
        .await
        .expect("clean up the drill event");
    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
#[ignore = "requires ALMANAC_TEST_CALENDAR_ID and Google service-account credentials via latch run"]
async fn an_entry_accepted_but_never_delivered_goes_out_on_the_next_start() {
    // The easier half of the guarantee, still worth proving live: a
    // crash *before* delivery loses nothing either.
    let calendar_id = std::env::var("ALMANAC_TEST_CALENDAR_ID")
        .expect("set ALMANAC_TEST_CALENDAR_ID to the almanac-test calendar's id");
    let dir = scratch_dir("undelivered");
    let journal_path = dir.join("journal.jsonl");

    let entity_id = format!(
        "drill-undelivered-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let upsert_key = format!("drill-source:{entity_id}");

    // First "process" only accepts, then dies without delivering.
    {
        let state = state_for(&calendar_id, journal_path.clone());
        state
            .journal
            .accept(&Entry {
                id: "entry-1".to_string(),
                source_id: "drill-source".to_string(),
                received_at: "2026-08-28T09:00:00+00:00".to_string(),
                payload: json!({
                    "external_id": entity_id,
                    "title": "accepted but never delivered",
                    "start": "2026-08-28T09:00:00+00:00",
                }),
                idempotency_key: None,
            })
            .await
            .expect("accept");
    }

    let restarted = state_for(&calendar_id, journal_path.clone());
    assert_eq!(
        restarted.journal.pending().expect("pending").len(),
        1,
        "the accepted entry must survive the crash"
    );

    assert_eq!(almanac::shell::worker::drain_once(&restarted).await, 1);

    let events = restarted
        .client
        .list_events_by_property(&calendar_id, UPSERT_PROPERTY, &upsert_key)
        .await
        .expect("list");
    assert_eq!(events.len(), 1, "the event should exist exactly once");

    restarted
        .client
        .delete_event(&calendar_id, events[0].id.as_ref().expect("event id"))
        .await
        .expect("clean up");
    std::fs::remove_dir_all(&dir).ok();
}
