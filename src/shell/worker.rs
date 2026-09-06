//! The background delivery loop (AR16). Drains the journal's pending
//! entries into Google and marks each done. Runs on startup — which is
//! what makes replay-after-a-crash automatic rather than a manual
//! recovery step — and then on an interval for everything the
//! asynchronous ingest path accepted.
//!
//! A delivery that fails is left pending deliberately: the next pass
//! retries it. That is the whole reason the journal exists, and it is
//! safe because upsert (K2/AR15) and idempotency keys (M7) make a
//! redelivery converge on the same event.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::watch;

use crate::core::observability::{RouteOutcome, RouteRecord};
use crate::core::pacing::{self, DrainSummary};
use crate::shell::ingest::AppState;
use crate::shell::notify::{Event, Notifier, ops};

/// How many times an entry may fail *permanently* before it is set
/// aside (T1).
///
/// Not one: a permanent classification can be wrong at the edges — a
/// calendar whose permissions were being changed at that moment, a
/// 403 whose reason string Google has not documented. Three passes
/// spread over the backoff ladder is long enough that a genuinely
/// transient misclassification has recovered, and short enough that a
/// dead payload does not hold the hub back for hours.
const PERMANENT_FAILURES_BEFORE_DEAD: usize = 3;

/// Compact once the log has grown past this many delivered records,
/// so a long-running process does not accumulate an unbounded file of
/// done-markers.
const COMPACT_AFTER_DELIVERIES: usize = 100;

/// Records one delivery attempt on the K11 debug surface, so the
/// operator can see how an event was routed — including a failure with
/// its remedy, which is the case where looking at all is most likely.
pub async fn record_route(
    state: &AppState,
    entry: &crate::core::journal::Entry,
    result: &Result<crate::shell::delivery::Delivered, crate::core::error::AlmanacError>,
) {
    let outcome = match result {
        Ok(d) if d.created => RouteOutcome::Created {
            event_id: d.event_id.clone(),
        },
        Ok(d) => RouteOutcome::Updated {
            event_id: d.event_id.clone(),
        },
        Err(e) => RouteOutcome::Failed {
            message: e.to_string(),
            remedy: e.remedy().to_string(),
        },
    };

    state.routes.lock().await.push(RouteRecord {
        at: (state.now)(),
        source_id: entry.source_id.clone(),
        entry_id: entry.id.clone(),
        // Whatever the delivery actually deduplicated against. A
        // failure has no key to report, since it never got that far.
        upsert_key: match &result {
            Ok(d) => d.upsert_key.clone(),
            Err(_) => None,
        },
        outcome,
    });
}

/// What one drain pass did, so the loop can decide how long to wait
/// before the next one.
pub struct DrainOutcome {
    pub delivered: usize,
    pub failed: usize,
}

/// Delivers every currently-pending entry once. Returns how many were
/// delivered. Never returns an error: one entry's failure must not
/// stop the others, and a failed entry stays pending for the next
/// pass.
pub async fn drain_once(state: &AppState) -> usize {
    drain_once_detailed(state).await.delivered
}

/// As [`drain_once`], but also reports failures so the caller can back
/// off while an outage lasts.
/// Per-entry count of consecutive permanent failures, so an entry that
/// can never succeed is eventually set aside rather than retried
/// forever. In memory: a restart gives every entry a fresh chance,
/// which is the right default because a restart is also how a fixed
/// profile gets loaded.
pub type PermanentFailures = std::collections::HashMap<String, usize>;

pub async fn drain_once_detailed(state: &AppState) -> DrainOutcome {
    drain_once_tracking(state, &mut PermanentFailures::new(), &Notifier::disabled()).await
}

/// As [`drain_once_detailed`], but remembering which entries keep
/// failing permanently so they can be set aside (T1).
pub async fn drain_once_tracking(
    state: &AppState,
    permanent: &mut PermanentFailures,
    notifier: &Notifier,
) -> DrainOutcome {
    let pending = match state.journal.pending() {
        Ok(pending) => pending,
        Err(e) => {
            tracing::error!(error = %e, remedy = %e.remedy(), "cannot read the journal");
            return DrainOutcome {
                delivered: 0,
                failed: 1,
            };
        }
    };

    if pending.is_empty() {
        return DrainOutcome {
            delivered: 0,
            failed: 0,
        };
    }

    tracing::info!(count = pending.len(), "delivering pending journal entries");

    let mut delivered = 0;
    let mut failed = 0;
    let profiles = state.profiles();
    for entry in pending {
        let Some(profile) = profiles.get(&entry.source_id) else {
            // The profile that accepted this payload is gone. Leaving
            // it pending forever would silently wedge the journal, so
            // say so loudly on every pass rather than dropping it.
            tracing::error!(
                entry_id = %entry.id,
                source_id = %entry.source_id,
                "journal entry names a source with no profile — restore the profile or move the \
                 journal aside; this entry cannot be delivered and will be retried indefinitely"
            );
            failed += 1;
            continue;
        };

        let result =
            crate::shell::delivery::deliver(&entry, profile, &state.client, &state.locks).await;
        record_route(state, &entry, &result).await;

        match result {
            Ok(result) => {
                if let Err(e) = state.journal.mark_done(&entry.id).await {
                    tracing::warn!(
                        entry_id = %entry.id, error = %e,
                        "delivered but failed to mark done; replay will converge"
                    );
                }
                tracing::info!(
                    entry_id = %entry.id,
                    event_id = %result.event_id,
                    created = result.created,
                    "delivered"
                );
                delivered += 1;
            }
            Err(e) if e.is_transient() => {
                permanent.remove(&entry.id);
                tracing::warn!(
                    entry_id = %entry.id, error = %e, remedy = %e.remedy(),
                    "delivery failed; entry stays pending for the next pass"
                );
                failed += 1;
            }
            Err(e) => {
                // A permanent failure will not fix itself by being
                // retried. Count it, and after a few passes set the
                // entry aside so one unmappable payload does not hold
                // the whole hub in its slowest backoff forever (T1).
                let seen = permanent.entry(entry.id.clone()).or_insert(0);
                *seen += 1;

                if *seen >= PERMANENT_FAILURES_BEFORE_DEAD {
                    let reason = format!("{e} — {}", e.remedy());
                    match state
                        .journal
                        .mark_dead(&entry.id, &reason, &(state.now)())
                        .await
                    {
                        Ok(()) => {
                            permanent.remove(&entry.id);
                            state.metrics.dead(1);
                            tracing::error!(
                                entry_id = %entry.id, source_id = %entry.source_id, reason = %reason,
                                "set aside as undeliverable after {PERMANENT_FAILURES_BEFORE_DEAD} \
                                 permanent failures; it is kept in the journal and shown on the \
                                 debug surface, but no longer retried"
                            );
                            notifier
                                .send(Event {
                                    op: ops::ENTRY_SET_ASIDE,
                                    ok: false,
                                    version: env!("CARGO_PKG_VERSION").to_string(),
                                    error: Some(format!(
                                        "an event from {} can never be delivered and was set \
                                         aside: {reason}",
                                        entry.source_id
                                    )),
                                })
                                .await;
                        }
                        Err(write_error) => tracing::error!(
                            entry_id = %entry.id, error = %write_error,
                            "could not set the entry aside; it stays pending"
                        ),
                    }
                } else {
                    tracing::warn!(
                        entry_id = %entry.id, error = %e, remedy = %e.remedy(),
                        permanent_failures = *seen,
                        "delivery failed permanently; entry stays pending for now"
                    );
                }
                failed += 1;
            }
        }
    }

    state.metrics.delivered(delivered as u64);
    state.metrics.failed(failed as u64);
    DrainOutcome { delivered, failed }
}

/// Whether the journal is filling up. The I/O half of AR26's warning;
/// what to do about it lives in `core::pacing`.
fn journal_is_filling(state: &AppState) -> bool {
    let Ok(size) = std::fs::metadata(state.journal.path()).map(|m| m.len()) else {
        return false;
    };
    let filling = pacing::journal_is_filling(size, state.journal.max_bytes());
    if filling {
        tracing::warn!(
            bytes = size,
            cap = state.journal.max_bytes(),
            "the journal is over half its cap — deliveries have been failing long enough to build \
             a backlog; check the delivery errors before it fills and ingest starts refusing events"
        );
    }
    filling
}

/// Runs the loop until `shutdown` flips. On exit it drains once more,
/// so a graceful stop (M2) hands over a journal with as little
/// outstanding work as possible.
pub async fn run(state: Arc<AppState>, mut shutdown: watch::Receiver<bool>, notifier: Notifier) {
    // Startup replay: whatever a crash or power cut left behind goes
    // out before anything new is accepted for delivery.
    let replayed = drain_once(&state).await;
    if replayed > 0 {
        tracing::info!(count = replayed, "replayed entries left by a previous run");
    }

    let mut since_compaction = replayed;
    let mut permanent = PermanentFailures::new();
    let mut pace = pacing::Worker::new();
    let mut ticker = tokio::time::interval(Duration::from_secs(pacing::POLL_INTERVAL_SECS));

    loop {
        tokio::select! {
            _ = ticker.tick() => {
                let outcome = drain_once_tracking(&state, &mut permanent, &notifier).await;
                since_compaction += outcome.delivered;

                // AR26: slow down while an outage lasts, speed back up
                // the moment anything gets through. The decision is in
                // `core::pacing`, which is where its tests are.
                let summary = DrainSummary {
                    delivered: outcome.delivered,
                    failed: outcome.failed,
                };
                let stalled = summary.failed > 0 && summary.delivered == 0;
                let next = pace.after(summary, stalled && journal_is_filling(&state));

                if next.report_backlog {
                    notifier
                        .send(Event {
                            op: ops::JOURNAL_BACKLOG,
                            ok: false,
                            version: env!("CARGO_PKG_VERSION").to_string(),
                            error: Some(
                                "deliveries keep failing and the journal is over half its cap; \
                                 once it fills, ingest starts refusing events and the sources' \
                                 own retries will eventually give up"
                                    .to_string(),
                            ),
                        })
                        .await;
                }
                if next.recovered {
                    tracing::info!("deliveries recovered; returning to the normal poll interval");
                }
                if stalled {
                    tracing::warn!(
                        consecutive_failures = pace.consecutive_failures(),
                        wait_seconds = next.wait_secs,
                        "deliveries are failing; backing off before the next attempt"
                    );
                }

                let wanted = Duration::from_secs(next.wait_secs);
                if ticker.period() != wanted {
                    ticker = tokio::time::interval(wanted);
                    ticker.tick().await; // the first tick fires immediately
                }

                if since_compaction >= COMPACT_AFTER_DELIVERIES {
                    match state.journal.compact().await {
                        Ok(kept) => {
                            tracing::info!(pending = kept, "compacted the journal");
                            since_compaction = 0;
                        }
                        Err(e) => tracing::warn!(error = %e, "journal compaction failed"),
                    }
                }
            }
            _ = shutdown.changed() => {
                if *shutdown.borrow() {
                    tracing::info!("worker shutting down — draining once more");
                    drain_once(&state).await;
                    return;
                }
            }
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::collections::HashMap;

    use crate::shell::calendar_client::GoogleCalendarClient;
    use crate::shell::journal::{DEFAULT_MAX_BYTES, Journal};

    /// A unique scratch directory.
    ///
    /// The counter matters: naming these by process id and nanoseconds
    /// alone lets two tests running on different threads land on the
    /// same directory and share a journal, which shows up as a rare,
    /// confusing failure in one of them and nowhere else.
    fn scratch(name: &str) -> std::path::PathBuf {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "almanac-{name}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A minimal state for driving the self-update loop, which only
    /// reads the capture buffer.
    pub(crate) async fn state_for_update_loop() -> (Arc<AppState>, std::path::PathBuf) {
        let (state, dir) = state_with_empty_journal();
        (state, dir)
    }

    fn state_with_empty_journal() -> (Arc<AppState>, std::path::PathBuf) {
        let dir = scratch("worker-test");

        let http = reqwest::Client::new();
        let tokens = crate::shell::auth::TokenManager::new(
            http.clone(),
            crate::core::auth::ServiceAccountCredentials {
                client_email: "unused".to_string(),
                private_key: "unused".to_string(),
                token_url: "https://example.invalid/token".to_string(),
            },
        );

        let state = Arc::new(AppState::new_for_test(
            HashMap::new(),
            Journal::new(dir.join("journal.jsonl"), DEFAULT_MAX_BYTES),
            GoogleCalendarClient::new(http, tokens),
        ));

        (state, dir)
    }

    #[tokio::test]
    async fn the_worker_returns_promptly_when_shutdown_is_signalled() {
        // M2: without this the process would hang on the worker handle
        // after the HTTP server stopped, and systemd would eventually
        // SIGKILL it — losing exactly the graceful drain the shutdown
        // path exists to perform.
        let (state, dir) = state_with_empty_journal();
        let (tx, rx) = watch::channel(false);

        let worker = tokio::spawn(run(state, rx, crate::shell::notify::Notifier::disabled()));
        tx.send(true).unwrap();

        let result = tokio::time::timeout(Duration::from_secs(5), worker).await;
        assert!(
            result.is_ok(),
            "the worker must return on the shutdown signal, not wait out its poll interval"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn draining_an_empty_journal_delivers_nothing_and_does_not_error() {
        let (state, dir) = state_with_empty_journal();
        assert_eq!(drain_once(&state).await, 0);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn a_drain_reports_failures_so_the_loop_can_back_off() {
        // AR26: without a failure count the loop would keep hammering
        // every five seconds through a multi-hour Google outage.
        let (state, dir) = state_with_empty_journal();
        state
            .journal
            .accept(&crate::core::journal::Entry {
                id: "orphan".to_string(),
                source_id: "profile-that-no-longer-exists".to_string(),
                received_at: "2026-08-28T09:00:00+00:00".to_string(),
                payload: serde_json::json!({"title": "t"}),
                idempotency_key: None,
            })
            .await
            .unwrap();

        let outcome = drain_once_detailed(&state).await;
        assert_eq!(outcome.delivered, 0);
        assert_eq!(
            outcome.failed, 1,
            "the loop must be able to see the failure"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn an_empty_journal_reports_neither_delivery_nor_failure() {
        // An idle hub must not look like an outage, or it would back
        // off to half-hour polls for no reason.
        let (state, dir) = state_with_empty_journal();
        let outcome = drain_once_detailed(&state).await;
        assert_eq!(outcome.delivered, 0);
        assert_eq!(outcome.failed, 0);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn an_entry_whose_profile_is_gone_is_left_pending_rather_than_dropped() {
        // Losing a payload because someone renamed a profile would be
        // exactly the silent data loss the journal exists to prevent.
        let (state, dir) = state_with_empty_journal();
        state
            .journal
            .accept(&crate::core::journal::Entry {
                id: "orphan".to_string(),
                source_id: "profile-that-no-longer-exists".to_string(),
                received_at: "2026-08-28T09:00:00+00:00".to_string(),
                payload: serde_json::json!({"title": "t"}),
                idempotency_key: None,
            })
            .await
            .unwrap();

        assert_eq!(drain_once(&state).await, 0, "nothing can be delivered");
        assert_eq!(
            state.journal.pending().unwrap().len(),
            1,
            "but the payload must still be there"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    /// A state whose deliveries always fail permanently.
    async fn state_rejecting_everything() -> (Arc<AppState>, std::path::PathBuf) {
        let dir = scratch("deadletter");

        let calendar = crate::shell::testing::CalendarStub::start().await;
        calendar.reject_next(9_999);
        let tokens = crate::shell::testing::TokenStub::start(3600).await;

        let toml = r#"
schema_version = 2
source_id = "home-assistant"
target_calendar_id = "household"

"#;
        let mut profiles = HashMap::new();
        profiles.insert(
            "home-assistant".to_string(),
            crate::core::profile::Profile::parse(toml, "t.toml").unwrap(),
        );

        let state = Arc::new(AppState::new_for_test(
            profiles,
            Journal::new(dir.join("journal.jsonl"), DEFAULT_MAX_BYTES),
            GoogleCalendarClient::with_base_url(
                reqwest::Client::new(),
                crate::shell::auth::TokenManager::new(
                    reqwest::Client::new(),
                    crate::shell::testing::stub_credentials(&tokens.url),
                ),
                &calendar.base_url,
            ),
        ));

        state
            .journal
            .accept(&crate::core::journal::Entry {
                id: "doomed".to_string(),
                source_id: "home-assistant".to_string(),
                received_at: "2026-08-29T09:00:00+00:00".to_string(),
                payload: serde_json::json!({
                    "title": "t",
                    "start": "2026-08-29T09:00:00+00:00"
                }),
                idempotency_key: None,
            })
            .await
            .unwrap();

        (state, dir)
    }

    #[tokio::test]
    async fn an_entry_that_can_never_be_delivered_is_eventually_set_aside() {
        // T1. Left pending, one unmappable payload is retried forever,
        // drives the worker to half-hourly polling for every other
        // source, and eventually raises a backlog alert about a queue
        // of exactly one dead event.
        let (state, dir) = state_rejecting_everything().await;
        let notifier = Notifier::disabled();
        let mut permanent = PermanentFailures::new();

        for pass in 1..PERMANENT_FAILURES_BEFORE_DEAD {
            drain_once_tracking(&state, &mut permanent, &notifier).await;
            assert_eq!(
                state.journal.pending().unwrap().len(),
                1,
                "still retrying on pass {pass} — one bad classification must not be enough"
            );
        }

        drain_once_tracking(&state, &mut permanent, &notifier).await;

        assert!(
            state.journal.pending().unwrap().is_empty(),
            "after {PERMANENT_FAILURES_BEFORE_DEAD} permanent failures it must stop being retried"
        );

        let dead = state.journal.dead().unwrap();
        assert_eq!(dead.len(), 1, "and must still be readable");
        assert_eq!(dead[0].0.id, "doomed");
        assert!(
            !dead[0].1.is_empty(),
            "with the reason, which is what makes it fixable"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn setting_an_entry_aside_is_reported() {
        // The source was told 202. Quietly giving up on that payload
        // would make that a lie nobody ever hears about.
        let (state, dir) = state_rejecting_everything().await;
        let notify = crate::shell::testing::NotifyStub::start().await;
        let notifier = Notifier::to(reqwest::Client::new(), &notify.url);
        let mut permanent = PermanentFailures::new();

        for _ in 0..PERMANENT_FAILURES_BEFORE_DEAD {
            drain_once_tracking(&state, &mut permanent, &notifier).await;
        }

        assert_eq!(notify.ops().await, vec![ops::ENTRY_SET_ASIDE.to_string()]);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn a_dead_entry_survives_compaction_with_its_reason() {
        // Compaction rewrites the log; dropping the dead entry there
        // would discard a payload the source was told had been
        // accepted, and lose the reason it failed.
        let (state, dir) = state_rejecting_everything().await;
        let notifier = Notifier::disabled();
        let mut permanent = PermanentFailures::new();

        for _ in 0..PERMANENT_FAILURES_BEFORE_DEAD {
            drain_once_tracking(&state, &mut permanent, &notifier).await;
        }
        state.journal.compact().await.unwrap();

        let dead = state.journal.dead().unwrap();
        assert_eq!(dead.len(), 1, "the dead entry must survive compaction");
        assert!(state.journal.pending().unwrap().is_empty());

        std::fs::remove_dir_all(&dir).ok();
    }
}
