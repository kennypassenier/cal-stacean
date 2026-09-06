//! The operator-facing surface: health (M1), debug introspection
//! (K11), the ping for the test button (4.0.1, replacing M11's capture
//! surface) and the dry-run mapper (M9).
//!
//! Everything except health sits behind the bootstrap token from the
//! environment (AR17 as amended) — the same token that will log into
//! the L4b dashboard. Health stays open on purpose: a monitoring stack
//! that fails closed lies to you during an outage, which is exactly
//! when you believe it. It carries no secret, so there is nothing to
//! protect.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::{Json, Router};
use serde_json::{Value, json};

use crate::core::mapping::map_payload;
use crate::shell::ingest::AppState;
use chassis::Caller;

/// Environment variable holding the bootstrap token. Absent means the
/// admin surface refuses every request rather than opening up — a
/// forgotten variable must not silently expose the debug views
/// (fail-closed, standing rule 12).
pub const BOOTSTRAP_TOKEN_ENV: &str = "ALMANAC_BOOTSTRAP_TOKEN";

/// The 3.x capture-only token's variable name. No longer read: a start
/// with it still set warns once (4.0.0), so a forgotten line in the env
/// file is noticed rather than silently ignored.
pub const CAPTURE_TOKEN_ENV: &str = "ALMANAC_CAPTURE_TOKEN";

type Reply = (StatusCode, Json<Value>);

fn error(status: StatusCode, message: &str, remedy: &str) -> Reply {
    (
        status,
        Json(json!({"status": "error", "message": message, "remedy": remedy})),
    )
}

/// The admin surfaces need the admin (the login token as bearer, or a
/// session): a client token that opened the kit's door is not enough.
fn require_admin(caller: &Caller) -> Result<(), Reply> {
    match caller {
        Caller::Admin => Ok(()),
        Caller::Client { .. } => Err(error(
            StatusCode::FORBIDDEN,
            "this needs the admin, not a client token",
            "send the service's login token (ALMANAC_TOKEN) as `Authorization: Bearer <token>`",
        )),
    }
}

/// `GET /v1/debug/status` (K11) — what is loaded, what is waiting, and
/// how the recent events were routed.
async fn debug_status(State(state): State<Arc<AppState>>, caller: Caller) -> Reply {
    if let Err(reply) = require_admin(&caller) {
        return reply;
    }

    let loaded = state.profiles();
    let mut profiles: Vec<_> = loaded
        .values()
        .map(|p| {
            json!({
                "source_id": p.source_id,
                "target_calendar_id": p.target_calendar_id,
                "schema_version": p.schema_version,
            })
        })
        .collect();
    profiles.sort_by_key(|p| p["source_id"].as_str().unwrap_or_default().to_string());

    let pending = match state.journal.pending() {
        Ok(pending) => json!({
            "count": pending.len(),
            "oldest": pending.first().map(|e| json!({
                "entry_id": e.id,
                "source_id": e.source_id,
                "received_at": e.received_at,
            })),
        }),
        Err(e) => json!({"error": e.to_string(), "remedy": e.remedy()}),
    };

    let routes: Vec<_> = state.routes.lock().await.iter().cloned().collect();

    (
        StatusCode::OK,
        Json(json!({
            "status": "ok",
            "profiles": profiles,
            "journal": pending,
            "recent_routes": routes,
        })),
    )
}

/// `POST /v1/debug/dry-run/{source_id}` (M9) — shows the calendar event
/// a payload would produce, without writing anything to Google. The
/// point is to check a new or changed profile against a real payload
/// before letting it near a calendar.
async fn dry_run(
    State(state): State<Arc<AppState>>,
    Path(source_id): Path<String>,
    caller: Caller,
    Json(payload): Json<Value>,
) -> Reply {
    if let Err(reply) = require_admin(&caller) {
        return reply;
    }

    let profiles = state.profiles();
    let Some(profile) = profiles.get(&source_id) else {
        return error(
            StatusCode::NOT_FOUND,
            &format!("no profile with source_id \"{source_id}\""),
            "check the loaded profiles at /v1/debug/status",
        );
    };

    match map_payload(&payload, profile, &format!("profile {source_id}")) {
        Ok(event) => (
            StatusCode::OK,
            Json(json!({
                "status": "ok",
                "would_write_to_calendar": profile.target_calendar_id,
                "event": event,
            })),
        ),
        Err(e) => error(StatusCode::UNPROCESSABLE_ENTITY, &e.to_string(), e.remedy()),
    }
}

/// `POST /v1/ping` — "does my token work?" for any caller the kit let in:
/// the kit's **Send test** button on the Sources page posts here, and the
/// round trip lands under that source's *Last requests* (K13). Almanac's
/// own capture surface (M11) went in 4.0.1: the kit keeps the last
/// requests per client token, headers masked, body cut — the same
/// evidence, on the row of the source that sent it.
async fn ping(caller: Caller) -> Reply {
    let who = match caller {
        Caller::Admin => "admin".to_string(),
        Caller::Client { name, .. } => name,
    };
    (StatusCode::OK, Json(json!({"status": "ok", "caller": who})))
}

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/v1/debug/status", axum::routing::get(debug_status))
        .route("/v1/ping", axum::routing::post(ping))
        .route(
            "/v1/debug/dry-run/{source_id}",
            axum::routing::post(dry_run),
        )
}
