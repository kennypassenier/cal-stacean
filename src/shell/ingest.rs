//! The inbound HTTP surface (K6, K7, K8, M7). One ingest endpoint per
//! source, addressed by the profile's immutable `source_id` (AR15):
//!
//!   POST /v1/ingest/{source_id}        → 202, durably journalled (K7)
//!   POST /v1/ingest/{source_id}/sync   → 200 + the event id (K8)
//!
//! Both authenticate with that source's own bearer token (K6): the
//! presented token is hashed and compared, in constant time, against
//! the `token_hash` in its profile. A source only ever holds a token
//! for itself, so one can be revoked without touching the others.
//!
//! Both journal the payload and fsync it *before* answering, so an
//! accepted request survives a crash or power cut (AR16). The
//! asynchronous form returns as soon as that is durable; the
//! synchronous form additionally waits for delivery, because its
//! caller wants the Google event id back.

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::{Json, Router};
use serde_json::{Value, json};

use tokio::sync::Mutex;

use crate::core::journal::Entry;
use crate::core::metrics::Metrics;
use crate::core::observability::{CaptureRecord, RingBuffer, RouteRecord};
use chassis::Caller;

use crate::core::profile::Profile;
use crate::shell::calendar_client::GoogleCalendarClient;
use crate::shell::delivery::{KeyLocks, deliver};
use crate::shell::journal::Journal;

/// Header a source may send to make a redelivery converge instead of
/// duplicating, when it has no natural per-payload id (M7).
const IDEMPOTENCY_HEADER: &str = "idempotency-key";

pub struct AppState {
    /// The loaded mapping profiles, swappable as a whole.
    ///
    /// Behind a lock since K21: the dashboard can add a profile while
    /// the service runs, and the alternative — telling Kenny to restart
    /// almanac after every change — is the friction that made him ask
    /// for the feature. Readers take the `Arc` and drop the guard
    /// immediately (`profiles()`), so a reload never waits on a request
    /// and no guard is ever held across an await.
    profiles: std::sync::RwLock<Arc<HashMap<String, Profile>>>,
    /// Where those profiles live, so a reload reads the same directory
    /// startup did (K20's resolved path, not a second guess at it).
    pub profiles_dir: std::path::PathBuf,
    /// Calendars deleted from the dashboard, until Google stops
    /// listing them (K24).
    ///
    /// Google's calendar list is eventually consistent: a calendar
    /// deleted a second ago can still come back in the very next list
    /// call, so the page that renders straight after the delete shows
    /// the thing that was just removed. Measured on 2026-09-03 —
    /// Kenny deleted one, it stayed on the page, and asking Google
    /// minutes later showed it genuinely gone.
    ///
    /// Almanac knows what it deleted, so it can say so rather than
    /// re-asking a source that has not caught up. Self-clearing: an id
    /// is dropped from here as soon as Google's own list no longer
    /// carries it, so this never grows and never outlives the truth.
    pub deleted_calendars: std::sync::Mutex<std::collections::HashSet<String>>,
    /// Calendars almanac made, by name, until Google lists them (K24).
    ///
    /// The mirror image of `deleted_calendars`, and it exists for the
    /// same lag. `ensure_calendar` refuses to make a second calendar
    /// with a name it can already see — but it looks in the very list
    /// that has not caught up, so a second click within those seconds
    /// finds nothing and creates a duplicate. Measured on CT 112: two
    /// `deleted a calendar` lines at 19:56 for one request.
    ///
    /// A duplicate calendar is close to invisible — events land,
    /// nothing errors, and half of them are on a calendar nobody has
    /// open — so the guard against it has to hold when Google's answer
    /// is still on its way.
    ///
    /// Self-clearing on the same rule as the tombstones: an entry is
    /// dropped as soon as Google's own list carries that id.
    pub created_calendars: std::sync::Mutex<std::collections::HashMap<String, String>>,
    /// Who a calendar created from the dashboard is shared with (K21).
    /// `None` disables creating calendars rather than creating one
    /// nobody can see: a calendar the service account makes is owned by
    /// the service account and invisible to every human until it is
    /// shared, and that mistake has already been made here twice.
    pub calendar_owner: Option<String>,
    pub journal: Journal,
    pub client: GoogleCalendarClient,
    pub locks: KeyLocks,
    /// Supplies the acceptance timestamp; injected rather than read
    /// ambiently so tests can pin it.
    pub now: Box<dyn Fn() -> String + Send + Sync>,
    /// Unix seconds, for the capture surface's expiry arithmetic (M11).
    /// Separate from `now` so retention never has to parse a timestamp
    /// back out of a formatted string.
    pub now_unix: Box<dyn Fn() -> u64 + Send + Sync>,
    /// Recent delivery routes, for the K11 debug surface.
    pub routes: Mutex<RingBuffer<RouteRecord>>,
    /// Verbatim captured requests, for the M11 capture surface.
    pub captures: Mutex<RingBuffer<CaptureRecord>>,
    /// M13 counters. Shared with the token manager, which is built
    /// before this state exists, so it is an `Arc` rather than owned.
    pub metrics: Arc<Metrics>,
}

/// How many recent routes and captures to keep. Enough to debug what
/// just happened; small enough that neither can grow into a memory
/// problem on a long-running process.
pub const HISTORY_CAPACITY: usize = 100;

impl AppState {
    /// Assembles the shared state with real clocks and empty history.
    /// A constructor rather than a struct literal at each call site:
    /// the observability fields are the same everywhere, and every
    /// future field would otherwise have to be added to five places.
    pub fn new(
        profiles: HashMap<String, Profile>,
        journal: Journal,
        client: GoogleCalendarClient,
    ) -> Self {
        Self {
            profiles: std::sync::RwLock::new(Arc::new(profiles)),
            profiles_dir: std::path::PathBuf::from("profiles"),
            calendar_owner: None,
            deleted_calendars: std::sync::Mutex::new(std::collections::HashSet::new()),
            created_calendars: std::sync::Mutex::new(std::collections::HashMap::new()),
            journal,
            client,
            locks: KeyLocks::new(),
            now: Box::new(|| chrono::Utc::now().to_rfc3339()),
            now_unix: Box::new(|| {
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0)
            }),
            routes: Mutex::new(RingBuffer::new(HISTORY_CAPACITY)),
            captures: Mutex::new(RingBuffer::new(HISTORY_CAPACITY)),
            metrics: Arc::new(Metrics::default()),
        }
    }

    /// The current profile set. Cheap: an `Arc` clone under a read
    /// lock held for the length of that clone.
    pub fn profiles(&self) -> Arc<HashMap<String, Profile>> {
        self.profiles
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    /// Replaces the whole set (K21). Whole-set rather than per-entry
    /// because `validate_unique_source_ids` is a property of the set —
    /// inserting one profile at a time cannot check it.
    pub fn set_profiles(&self, profiles: HashMap<String, Profile>) {
        *self
            .profiles
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Arc::new(profiles);
    }

    /// Where profiles are read from and written to. A builder rather
    /// than a constructor argument: only `main` knows the resolved
    /// path, and every test would otherwise have to invent one.
    pub fn with_profiles_dir(mut self, dir: std::path::PathBuf) -> Self {
        self.profiles_dir = dir;
        self
    }

    /// Remembers that a calendar was deleted, so the page rendered
    /// straight afterwards does not show it (K24).
    pub fn remember_deleted_calendar(&self, calendar_id: &str) {
        self.deleted_calendars
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(calendar_id.to_string());
    }

    /// Filters Google's list through what almanac knows it deleted, and
    /// forgets the ids Google has caught up on.
    pub fn without_deleted_calendars(
        &self,
        calendars: Vec<(String, String)>,
    ) -> Vec<(String, String)> {
        let mut deleted = self
            .deleted_calendars
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if deleted.is_empty() {
            return calendars;
        }
        let listed: std::collections::HashSet<&str> =
            calendars.iter().map(|(id, _)| id.as_str()).collect();
        // Anything Google no longer lists has caught up; drop it here
        // so this set stays as small as the disagreement is.
        deleted.retain(|id| listed.contains(id.as_str()));
        calendars
            .into_iter()
            .filter(|(id, _)| !deleted.contains(id))
            .collect()
    }

    /// Remembers a calendar almanac just made, so the next few seconds
    /// do not make a second one with the same name (K24).
    pub fn remember_created_calendar(&self, summary: &str, calendar_id: &str) {
        self.created_calendars
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(summary.to_string(), calendar_id.to_string());
    }

    /// The id of a calendar almanac made under this name and Google has
    /// not listed yet, if there is one.
    pub fn remembered_calendar(&self, summary: &str) -> Option<String> {
        self.created_calendars
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(summary)
            .cloned()
    }

    /// Drops the memory of a created calendar by id.
    ///
    /// Called when one is deleted: without this, a calendar made and
    /// removed inside the same lag window would keep being added back
    /// to the rendered list by the very memory that was meant to make
    /// it appear.
    pub fn forget_created_calendar(&self, calendar_id: &str) {
        self.created_calendars
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .retain(|_, id| id != calendar_id);
    }

    /// Adds what almanac made and Google has not listed yet, and
    /// forgets the ones it now lists.
    ///
    /// The absence is as misleading as the stale presence was: a
    /// calendar created a second ago is missing from the page that
    /// renders next, which reads as "it did not work".
    pub fn with_created_calendars(
        &self,
        mut calendars: Vec<(String, String)>,
    ) -> Vec<(String, String)> {
        let mut created = self
            .created_calendars
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if created.is_empty() {
            return calendars;
        }
        let listed: std::collections::HashSet<&str> =
            calendars.iter().map(|(id, _)| id.as_str()).collect();
        let missing: Vec<(String, String)> = created
            .iter()
            .filter(|(_, id)| !listed.contains(id.as_str()))
            .map(|(summary, id)| (id.clone(), summary.clone()))
            .collect();
        // Google has caught up on the rest; keeping them would mean
        // holding a name that could since have been renamed there.
        created.retain(|_, id| !listed.contains(id.as_str()));
        calendars.extend(missing);
        calendars.sort_by(|a, b| a.1.cmp(&b.1));
        calendars
    }

    /// Sets who a dashboard-created calendar is shared with (K21).
    pub fn with_calendar_owner(mut self, owner: Option<String>) -> Self {
        self.calendar_owner = owner;
        self
    }

    /// Shares one set of counters with the token manager, which is
    /// constructed earlier in startup. Without this the two would each
    /// count into their own instance and `almanac_token_refreshes_total`
    /// would sit at zero forever.
    pub fn with_metrics(mut self, metrics: Arc<Metrics>) -> Self {
        self.metrics = metrics;
        self
    }

    /// The capture buffer with everything past its TTL already
    /// dropped.
    ///
    /// Every reader must go through this rather than locking
    /// `captures` directly. Expiry used to be repeated at each call
    /// site, and the self-updater's "are captures still retained?"
    /// check was written without it — so a single capture that nobody
    /// ever looked at again suppressed every update for the life of
    /// the process, because the TTL only ever ran while someone was
    /// reading the page.
    pub async fn captures_after_expiry(
        &self,
    ) -> tokio::sync::MutexGuard<'_, RingBuffer<CaptureRecord>> {
        let mut captures = self.captures.lock().await;
        crate::core::observability::expire_captures(
            &mut captures,
            (self.now_unix)(),
            crate::shell::admin::CAPTURE_TTL_SECS,
        );
        captures
    }

    /// How many captures are still within their TTL right now, without
    /// waiting on the lock (3.0.0: the kit's update gate asks this from a
    /// synchronous closure). `None` while a reader holds the buffer — the
    /// caller treats that as "busy", which is also a reason not to restart.
    pub fn captures_retained_now(&self) -> Option<usize> {
        let mut captures = self.captures.try_lock().ok()?;
        crate::core::observability::expire_captures(
            &mut captures,
            (self.now_unix)(),
            crate::shell::admin::CAPTURE_TTL_SECS,
        );
        Some(captures.len())
    }

    /// Same, but with both clocks pinned — for tests that assert on
    /// timestamps or drive expiry without waiting.
    #[cfg(test)]
    pub fn new_for_test(
        profiles: HashMap<String, Profile>,
        journal: Journal,
        client: GoogleCalendarClient,
    ) -> Self {
        Self {
            now: Box::new(|| "2026-08-28T09:00:00+00:00".to_string()),
            now_unix: Box::new(|| 1_787_000_000),
            ..Self::new(profiles, journal, client)
        }
    }
}

type Reply = (StatusCode, Json<Value>);

fn error(status: StatusCode, message: &str, remedy: &str) -> Reply {
    (
        status,
        Json(json!({"status": "error", "message": message, "remedy": remedy})),
    )
}

/// Resolves the profile for `source_id` and decides whether the caller
/// may post as that source.
///
/// 4.0.0: the kit's door already checked the bearer token and named the
/// caller; what is left is whether THIS token belongs to THIS source — a
/// client under the source's own name, or the admin (K6). An unknown
/// source and a foreign token both answer 401 with the same body:
/// distinguishing them would tell a caller which source ids exist.
async fn authenticate(
    state: &AppState,
    source_id: &str,
    caller: &Caller,
) -> Result<Profile, Reply> {
    let unauthorized = || {
        error(
            StatusCode::UNAUTHORIZED,
            "unknown source or invalid token",
            "post with the token issued for this source on the Sources page (the client's name is the source id)",
        )
    };
    let profiles = state.profiles();
    let Some(profile) = profiles.get(source_id) else {
        return Err(unauthorized());
    };
    match caller {
        Caller::Admin => Ok(profile.clone()),
        Caller::Client { name, .. } if name == source_id => Ok(profile.clone()),
        Caller::Client { .. } => Err(unauthorized()),
    }
}

/// Refuses a call Almanac would not be able to find again.
///
/// Every event Almanac creates carries a marker built from the source's
/// own id, and that marker is the only handle the upsert and the delete
/// endpoint have. A call with neither an `external_id` in the payload
/// nor an `Idempotency-Key` header produces an event Almanac can never
/// update or remove — it duplicates on every resend and answers 404 on
/// every delete.
///
/// Refused at the door rather than left to a profile default. The
/// JobTracker session hit exactly this on 2026-09-03 against the live
/// service, hours after the dashboard started writing profiles: two
/// identical posts, two events, and a delete answering 404. A default
/// in a template fixes the next source; a refusal here fixes all of
/// them.
fn requires_an_id(payload: &Value, headers: &HeaderMap) -> Result<(), Reply> {
    let has_external_id = payload
        .get("external_id")
        .and_then(Value::as_str)
        .is_some_and(|id| !id.trim().is_empty());
    let has_header = headers
        .get(IDEMPOTENCY_HEADER)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|s| !s.trim().is_empty());

    if has_external_id || has_header {
        return Ok(());
    }
    Err(error(
        StatusCode::UNPROCESSABLE_ENTITY,
        "the payload has no \"external_id\" and no Idempotency-Key header",
        "send \"external_id\" with the source's own id for this thing, or an \
         Idempotency-Key header — without one of the two, almanac cannot find the event \
         again to update or delete it, and every resend creates another",
    ))
}

/// Refuses a payload Almanac could never turn into an event.
///
/// Checked at the door, before the journal, so a body that can only
/// ever fail is answered with 422 instead of being stored and retried
/// until it dead-letters. Two things were wrong without this, and they
/// were the same thing seen from two sides (Kenny, 2026-09-03):
///
/// - the asynchronous post answered a reassuring 202 to a payload with
///   a misspelled field, and the mistake only surfaced later, in a list
///   nobody watches;
/// - the synchronous post answered 502 both when Google had hiccuped
///   and when the body was unusable, so a caller could not tell "wait,
///   almanac is retrying" from "retrying will never help". The
///   JobTracker session was showing "almanac will try again" for a date
///   sent without `all_day`, which is a sentence nobody could disprove.
///
/// With this, 502 means Google and only Google, and 422 means the
/// source has to send something else. Neither needs a new field:
/// HTTP already has this distinction and we were not using it.
///
/// The cost, accepted: a payload almanac cannot map is refused rather
/// than kept. It was lost either way — this way the sender hears it
/// while it can still act.
fn must_be_mappable(payload: &Value, profile: &Profile) -> Result<(), Reply> {
    match crate::core::mapping::map_payload(
        payload,
        profile,
        &format!("profile {}", profile.source_id),
    ) {
        Ok(_) => Ok(()),
        Err(e) => Err(error(
            StatusCode::UNPROCESSABLE_ENTITY,
            &e.to_string(),
            e.remedy(),
        )),
    }
}

fn build_entry(state: &AppState, source_id: &str, headers: &HeaderMap, payload: Value) -> Entry {
    let idempotency_key = headers
        .get(IDEMPOTENCY_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    Entry {
        id: uuid::Uuid::new_v4().to_string(),
        source_id: source_id.to_string(),
        received_at: (state.now)(),
        payload,
        idempotency_key,
    }
}

/// `POST /v1/ingest/{source_id}` — accept, journal, answer 202 (K7).
async fn ingest(
    State(state): State<Arc<AppState>>,
    Path(source_id): Path<String>,
    caller: Caller,
    headers: HeaderMap,
    Json(payload): Json<Value>,
) -> Reply {
    let profile = match authenticate(&state, &source_id, &caller).await {
        Ok(profile) => profile,
        Err(reply) => return reply,
    };
    let source_id = profile.source_id.clone();

    if let Err(reply) = requires_an_id(&payload, &headers) {
        return reply;
    }
    if let Err(reply) = must_be_mappable(&payload, &profile) {
        return reply;
    }

    let entry = build_entry(&state, &source_id, &headers, payload);

    if let Err(e) = state.journal.accept(&entry).await {
        tracing::error!(source_id = %source_id, error = %e, "failed to journal an accepted payload");
        return error(
            StatusCode::INTERNAL_SERVER_ERROR,
            &e.to_string(),
            e.remedy(),
        );
    }

    state.metrics.accepted();
    tracing::info!(source_id = %source_id, entry_id = %entry.id, "payload accepted and journalled");

    (
        StatusCode::ACCEPTED,
        Json(json!({"status": "accepted", "entry_id": entry.id})),
    )
}

/// `POST /v1/ingest/{source_id}/sync` — accept, journal, deliver, and
/// answer with the Google event id (K8).
async fn ingest_sync(
    State(state): State<Arc<AppState>>,
    Path(source_id): Path<String>,
    caller: Caller,
    headers: HeaderMap,
    Json(payload): Json<Value>,
) -> Reply {
    let profile = match authenticate(&state, &source_id, &caller).await {
        Ok(profile) => profile,
        Err(reply) => return reply,
    };
    let profile = profile.clone();

    if let Err(reply) = requires_an_id(&payload, &headers) {
        return reply;
    }
    // Mapped twice on this path — once here, once inside `deliver`.
    // It is a pure function over the payload and the profile, so the
    // second run is cheap, and the alternative is a journalled entry
    // that exists only to fail.
    if let Err(reply) = must_be_mappable(&payload, &profile) {
        return reply;
    }

    let entry = build_entry(&state, &profile.source_id, &headers, payload);

    if let Err(e) = state.journal.accept(&entry).await {
        tracing::error!(source_id = %profile.source_id, error = %e, "failed to journal an accepted payload");
        return error(
            StatusCode::INTERNAL_SERVER_ERROR,
            &e.to_string(),
            e.remedy(),
        );
    }

    state.metrics.accepted();

    match deliver(&entry, &profile, &state.client, &state.locks).await {
        Ok(delivered) => {
            if let Err(e) = state.journal.mark_done(&entry.id).await {
                // The event IS on the calendar; only the bookkeeping
                // failed. Replay would redeliver it, which upsert makes
                // harmless, so this is a warning rather than an error
                // to the caller.
                tracing::warn!(
                    entry_id = %entry.id, error = %e,
                    "delivered but failed to mark the journal entry done; replay will converge"
                );
            }
            (
                StatusCode::OK,
                Json(json!({
                    "status": "delivered",
                    "event_id": delivered.event_id,
                    "created": delivered.created,
                })),
            )
        }
        Err(e) => {
            // The entry stays pending in the journal, so the worker
            // retries it later — the caller's payload is not lost even
            // though this response reports the failure.
            tracing::error!(source_id = %profile.source_id, error = %e, "synchronous delivery failed; left pending for retry");
            error(StatusCode::BAD_GATEWAY, &e.to_string(), e.remedy())
        }
    }
}

/// `DELETE /v1/ingest/{source_id}/events/{external_id}` (K8) — removes
/// the event a source previously created.
///
/// The caller addresses the event by the id *it* used, not by Google's:
/// a Claude session that created an event with `external_id = "task-7"`
/// deletes it with the same name, and never has to have kept the
/// Google event id. That works because the upsert key is pinned to
/// `<source_id>:<external-id>` (AR15) and stored on the event, which
/// is the same lookup a redelivery uses to update instead of
/// duplicating.
///
/// Synchronous and not journalled, unlike ingest. There is no payload
/// to lose here: if Google is unreachable the caller is told so and
/// retries, whereas an accepted-but-undelivered *deletion* would be a
/// promise Almanac cannot keep — the event would stay on the calendar
/// while the caller believed it was gone.
///
/// A source can only delete under its own prefix, so one source can
/// never remove another's events even if it guesses the external id.
async fn delete_event(
    State(state): State<Arc<AppState>>,
    Path((source_id, external_id)): Path<(String, String)>,
    caller: Caller,
) -> Reply {
    let profile = match authenticate(&state, &source_id, &caller).await {
        Ok(profile) => profile,
        Err(reply) => return reply,
    };

    let key = format!("{}:{external_id}", profile.source_id);
    let calendar = profile.target_calendar_id.clone();

    // Serialize on the same key the delivery path uses, so a delete
    // cannot interleave with an upsert of the same event and leave a
    // recreated copy behind.
    let lock = state.locks.for_key(&key).await;
    let _guard = lock.lock().await;

    let found = match state
        .client
        .find_event_by_property(&calendar, crate::shell::delivery::UPSERT_PROPERTY, &key)
        .await
    {
        Ok(found) => found,
        Err(e) => {
            tracing::warn!(source_id = %profile.source_id, error = %e, "delete lookup failed");
            return error(StatusCode::BAD_GATEWAY, &e.to_string(), e.remedy());
        }
    };

    let Some(event) = found else {
        // Deliberately distinct from success: a caller retrying a
        // delete needs to be able to tell "already gone" from "just
        // removed", and silently answering 200 would hide a wrong
        // external id forever.
        return (
            StatusCode::NOT_FOUND,
            Json(json!({
                "status": "not_found",
                "message": format!("no event on {calendar} carries {key}"),
                "remedy": "check the external id; it must be the one this source used when the \
                           event was created"
            })),
        );
    };

    let Some(event_id) = event.id.clone() else {
        return error(
            StatusCode::BAD_GATEWAY,
            "the Calendar API returned a matching event with no id",
            "this is unexpected; nothing was deleted",
        );
    };

    match state.client.delete_event(&calendar, &event_id).await {
        Ok(()) => {
            tracing::info!(
                source_id = %profile.source_id, %event_id, %key,
                "deleted an event on its source's request"
            );
            (
                StatusCode::OK,
                Json(json!({"status": "deleted", "event_id": event_id})),
            )
        }
        Err(e) => {
            tracing::warn!(source_id = %profile.source_id, error = %e, "delete failed");
            error(StatusCode::BAD_GATEWAY, &e.to_string(), e.remedy())
        }
    }
}

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/v1/ingest/{source_id}", axum::routing::post(ingest))
        .route(
            "/v1/ingest/{source_id}/sync",
            axum::routing::post(ingest_sync),
        )
        .route(
            "/v1/ingest/{source_id}/events/{external_id}",
            axum::routing::delete(delete_event),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

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

    fn scratch_dir() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "almanac-ingest-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// State holding one source whose token is already issued in the
    /// encrypted store — the only place ingest auth consults since the
    /// AR17 amendment.
    async fn state_with(source_id: &str, _token: &str) -> AppState {
        let dir = scratch_dir();

        let mut profiles = HashMap::new();
        profiles.insert(source_id.to_string(), profile(source_id));

        AppState::new_for_test(
            profiles,
            Journal::new(
                dir.join("journal.jsonl"),
                crate::shell::journal::DEFAULT_MAX_BYTES,
            ),
            GoogleCalendarClient::new(
                reqwest::Client::new(),
                crate::shell::auth::TokenManager::new(
                    reqwest::Client::new(),
                    crate::core::auth::ServiceAccountCredentials {
                        client_email: "unused".to_string(),
                        private_key: "unused".to_string(),
                        token_url: "https://example.invalid/token".to_string(),
                    },
                ),
            ),
        )
    }

    fn headers_with_token(token: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::AUTHORIZATION,
            format!("Bearer {token}").parse().unwrap(),
        );
        headers
    }

    fn client(name: &str) -> Caller {
        Caller::Client {
            id: format!("source-{name}"),
            name: name.to_string(),
        }
    }

    #[tokio::test]
    async fn the_sources_own_client_authenticates() {
        let state = state_with("home-assistant", "correct-token").await;
        assert!(
            authenticate(&state, "home-assistant", &client("home-assistant"))
                .await
                .is_ok()
        );
        // The admin's login token posts as any source (Kenny's scripts).
        assert!(
            authenticate(&state, "home-assistant", &Caller::Admin)
                .await
                .is_ok()
        );
    }

    #[tokio::test]
    async fn a_foreign_client_and_an_unknown_source_answer_the_same_401() {
        // K6's actual promise: one source's token must not open another,
        // and the answer must not reveal which source ids exist.
        let state = state_with("home-assistant", "ha-token").await;
        let mut profiles = (*state.profiles()).clone();
        profiles.insert("uptime-kuma".to_string(), profile("uptime-kuma"));
        state.set_profiles(profiles);
        assert!(
            authenticate(&state, "uptime-kuma", &client("uptime-kuma"))
                .await
                .is_ok()
        );
        let foreign = authenticate(&state, "uptime-kuma", &client("home-assistant"))
            .await
            .unwrap_err();
        assert_eq!(foreign.0, StatusCode::UNAUTHORIZED);
        let unknown = authenticate(&state, "nope", &client("nope"))
            .await
            .unwrap_err();
        assert_eq!(unknown.0, StatusCode::UNAUTHORIZED);
        assert_eq!(foreign.1.0["message"], unknown.1.0["message"]);
    }

    #[tokio::test]
    async fn an_idempotency_key_header_lands_on_the_entry() {
        let state = state_with("home-assistant", "t").await;
        let mut headers = headers_with_token("t");
        headers.insert(IDEMPOTENCY_HEADER, "abc123".parse().unwrap());
        let entry = build_entry(&state, "home-assistant", &headers, json!({}));
        assert_eq!(entry.idempotency_key.as_deref(), Some("abc123"));
    }

    #[tokio::test]
    async fn an_absent_or_blank_idempotency_key_is_none_not_an_empty_string() {
        let state = state_with("home-assistant", "t").await;
        let entry = build_entry(
            &state,
            "home-assistant",
            &headers_with_token("t"),
            json!({}),
        );
        assert_eq!(entry.idempotency_key, None);

        let mut headers = headers_with_token("t");
        headers.insert(IDEMPOTENCY_HEADER, "   ".parse().unwrap());
        let entry = build_entry(&state, "home-assistant", &headers, json!({}));
        assert_eq!(entry.idempotency_key, None);
    }

    #[tokio::test]
    async fn each_accepted_payload_gets_its_own_entry_id() {
        let state = state_with("home-assistant", "t").await;
        let a = build_entry(
            &state,
            "home-assistant",
            &headers_with_token("t"),
            json!({}),
        );
        let b = build_entry(
            &state,
            "home-assistant",
            &headers_with_token("t"),
            json!({}),
        );
        assert_ne!(a.id, b.id);
    }

    #[tokio::test]
    async fn a_capture_that_aged_out_no_longer_counts_as_retained() {
        // AR25 suppresses self-update while captures are retained. The
        // suppression check used to read the buffer without expiring
        // it, and expiry only ran while somebody had a capture page
        // open — so one capture that Kenny looked at once and forgot
        // stopped every update for the life of the process. Months of
        // releases, including security fixes, silently never installed.
        let state = state_with("home-assistant", "tok").await;

        // The pinned clock is 1_787_000_000 and the TTL is an hour, so
        // this record is two hours old.
        state.captures.lock().await.push(CaptureRecord {
            at: "2026-08-28T07:00:00+00:00".to_string(),
            at_unix: 1_787_000_000 - 7_200,
            label: "unknown-webhook".to_string(),
            method: "POST".to_string(),
            headers: Vec::new(),
            body: "{}".to_string(),
            truncated_from_bytes: None,
        });

        assert!(
            state.captures_after_expiry().await.is_empty(),
            "an expired capture must not keep suppressing self-update"
        );
    }

    #[tokio::test]
    async fn a_fresh_capture_does_still_count_as_retained() {
        // The other half: expiry must not throw away a capture Kenny
        // is actually looking at, or a restart would discard exactly
        // the requests he is reverse-engineering.
        let state = state_with("home-assistant", "tok").await;

        state.captures.lock().await.push(CaptureRecord {
            at: "2026-08-28T08:55:00+00:00".to_string(),
            at_unix: 1_787_000_000 - 300,
            label: "unknown-webhook".to_string(),
            method: "POST".to_string(),
            headers: Vec::new(),
            body: "{}".to_string(),
            truncated_from_bytes: None,
        });

        assert_eq!(state.captures_after_expiry().await.len(), 1);
    }

    /// State whose calendar client points at a stub, so the
    /// synchronous path can actually deliver.
    async fn state_with_calendar(
        source_id: &str,
        _token: &str,
        calendar: &crate::shell::testing::CalendarStub,
    ) -> AppState {
        let dir = scratch_dir();

        let mut profiles = HashMap::new();
        profiles.insert(source_id.to_string(), profile(source_id));

        let tokens = crate::shell::testing::TokenStub::start(3600).await;
        AppState::new_for_test(
            profiles,
            Journal::new(
                dir.join("journal.jsonl"),
                crate::shell::journal::DEFAULT_MAX_BYTES,
            ),
            GoogleCalendarClient::with_base_url(
                reqwest::Client::new(),
                crate::shell::auth::TokenManager::new(
                    reqwest::Client::new(),
                    crate::shell::testing::stub_credentials(&tokens.url),
                ),
                &calendar.base_url,
            ),
        )
    }

    fn bearer(token: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::AUTHORIZATION,
            format!("Bearer {token}").parse().unwrap(),
        );
        headers
    }

    fn sync_payload() -> Value {
        serde_json::json!({
            "title": "meeting",
            "start": "2026-08-28T09:00:00+00:00",
            "external_id": "claude-session-1"
        })
    }

    #[tokio::test]
    async fn the_synchronous_endpoint_delivers_and_returns_the_event_id() {
        // K8: a Claude session posts and wants the Google event id
        // back. Nothing tested this endpoint at all — not the happy
        // path, not the response shape.
        let calendar = crate::shell::testing::CalendarStub::start().await;
        let state = Arc::new(state_with_calendar("home-assistant", "tok", &calendar).await);

        let (status, Json(body)) = ingest_sync(
            State(Arc::clone(&state)),
            Path("home-assistant".to_string()),
            Caller::Admin,
            bearer("tok"),
            Json(sync_payload()),
        )
        .await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["status"], "delivered");
        assert!(
            body["event_id"].as_str().is_some_and(|id| !id.is_empty()),
            "the caller needs a real event id back, got {body}"
        );
        assert_eq!(body["created"], true);

        assert!(
            state.journal.pending().unwrap().is_empty(),
            "a delivered entry must be marked done"
        );
    }

    #[tokio::test]
    async fn the_synchronous_endpoint_rejects_a_wrong_token() {
        // The auth guard on this route was covered by nothing.
        let calendar = crate::shell::testing::CalendarStub::start().await;
        let state = Arc::new(state_with_calendar("home-assistant", "tok", &calendar).await);

        let (status, _) = ingest_sync(
            State(Arc::clone(&state)),
            Path("home-assistant".to_string()),
            client("someone-else"),
            bearer("wrong-token"),
            Json(sync_payload()),
        )
        .await;

        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert!(
            state.journal.pending().unwrap().is_empty(),
            "an unauthenticated request must journal nothing"
        );
        assert_eq!(
            calendar.state.request_count().await,
            0,
            "and must never reach Google"
        );
    }

    #[tokio::test]
    async fn a_failed_synchronous_delivery_reports_502_but_keeps_the_payload() {
        // The promise in the handler's own comment: the caller is told
        // it failed, and the entry stays pending so the worker retries
        // it. Losing the payload here would make the synchronous
        // endpoint strictly worse than the asynchronous one.
        let calendar = crate::shell::testing::CalendarStub::start().await;
        calendar.reject_next(99);
        let state = Arc::new(state_with_calendar("home-assistant", "tok", &calendar).await);

        let (status, _) = ingest_sync(
            State(Arc::clone(&state)),
            Path("home-assistant".to_string()),
            Caller::Admin,
            bearer("tok"),
            Json(sync_payload()),
        )
        .await;

        assert_eq!(status, StatusCode::BAD_GATEWAY);
        assert_eq!(
            state.journal.pending().unwrap().len(),
            1,
            "the payload must survive for the worker to retry"
        );
    }

    #[tokio::test]
    async fn a_source_can_delete_the_event_it_created() {
        // K8's criterion says create, update *and* delete. The verb was
        // never built; Kenny asked for it when the gap was reported.
        let calendar = crate::shell::testing::CalendarStub::start().await;
        calendar
            .seed(
                "primary",
                serde_json::json!({
                    "id": "google-event-1",
                    "summary": "meeting",
                    "start": {"dateTime": "2026-08-29T09:00:00+00:00", "timeZone": "UTC"},
                    "end": {"dateTime": "2026-08-29T10:00:00+00:00", "timeZone": "UTC"},
                    "extendedProperties": {
                        "private": {"almanac_source_id": "home-assistant:task-7"}
                    }
                }),
            )
            .await;
        let state = Arc::new(state_with_calendar("home-assistant", "tok", &calendar).await);

        let (status, Json(body)) = delete_event(
            State(Arc::clone(&state)),
            Path(("home-assistant".to_string(), "task-7".to_string())),
            Caller::Admin,
        )
        .await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["status"], "deleted");
        assert_eq!(body["event_id"], "google-event-1");
        assert!(
            calendar
                .state
                .requests
                .lock()
                .await
                .iter()
                .any(|(method, _)| method == "DELETE"),
            "it must actually have asked Google to delete it"
        );
    }

    #[tokio::test]
    async fn deleting_something_that_is_not_there_says_so_rather_than_pretending() {
        // A caller retrying a delete needs to tell "already gone" from
        // "just removed", and answering 200 for a wrong external id
        // would hide the mistake forever.
        let calendar = crate::shell::testing::CalendarStub::start().await;
        let state = Arc::new(state_with_calendar("home-assistant", "tok", &calendar).await);

        let (status, Json(body)) = delete_event(
            State(Arc::clone(&state)),
            Path(("home-assistant".to_string(), "never-existed".to_string())),
            Caller::Admin,
        )
        .await;

        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(body["status"], "not_found");
        assert!(
            body["remedy"].as_str().unwrap().contains("external id"),
            "and point at the likely cause"
        );
    }

    #[tokio::test]
    async fn one_source_cannot_delete_another_sources_event() {
        // The upsert key is prefixed with the source id, so even a
        // correct guess of someone else's external id addresses a key
        // this source can never name.
        let calendar = crate::shell::testing::CalendarStub::start().await;
        calendar
            .seed(
                "primary",
                serde_json::json!({
                    "id": "google-event-1",
                    "summary": "someone else's event",
                    "start": {"dateTime": "2026-08-29T09:00:00+00:00", "timeZone": "UTC"},
                    "end": {"dateTime": "2026-08-29T10:00:00+00:00", "timeZone": "UTC"},
                    "extendedProperties": {
                        "private": {"almanac_source_id": "uptime-kuma:task-7"}
                    }
                }),
            )
            .await;
        let state = Arc::new(state_with_calendar("home-assistant", "tok", &calendar).await);

        let (status, _) = delete_event(
            State(Arc::clone(&state)),
            Path(("home-assistant".to_string(), "task-7".to_string())),
            Caller::Admin,
        )
        .await;

        assert_eq!(
            status,
            StatusCode::NOT_FOUND,
            "another source's event must be invisible, not deletable"
        );
        assert!(
            !calendar
                .state
                .requests
                .lock()
                .await
                .iter()
                .any(|(method, _)| method == "DELETE"),
            "and nothing may be deleted"
        );
    }

    #[tokio::test]
    async fn deleting_needs_this_sources_own_token() {
        let calendar = crate::shell::testing::CalendarStub::start().await;
        let state = Arc::new(state_with_calendar("home-assistant", "tok", &calendar).await);

        let (status, _) = delete_event(
            State(Arc::clone(&state)),
            Path(("home-assistant".to_string(), "task-7".to_string())),
            client("someone-else"),
        )
        .await;

        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(
            calendar.state.request_count().await,
            0,
            "an unauthenticated delete must never reach Google"
        );
    }

    #[tokio::test]
    async fn a_delete_that_google_refuses_is_reported_rather_than_claimed() {
        let calendar = crate::shell::testing::CalendarStub::start().await;
        calendar.reject_next(99);
        let state = Arc::new(state_with_calendar("home-assistant", "tok", &calendar).await);

        let (status, _) = delete_event(
            State(Arc::clone(&state)),
            Path(("home-assistant".to_string(), "task-7".to_string())),
            Caller::Admin,
        )
        .await;

        assert_eq!(status, StatusCode::BAD_GATEWAY);
    }
}
