//! Almanac's own dashboard pages on the kit (M12 as amended 2026-09-06,
//! step 2 of the chassis migration): **Sources** — the mapping profiles,
//! the calendars they write to and the files that could not be loaded —
//! and **Captures**. The kit brings the layout, the login and session,
//! the clients page (labelled "Sources" here: the per-source tokens),
//! CSRF and CSP; this module fills `content` with minijinja templates.
//!
//! The UI is English per standing rule 1 (Dutch is for conversation and
//! for dashboards meant for Kenny's parents; this is an operator tool).

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Extension, Form, Router};
use chassis::Dashboard;
use serde::Deserialize;
use serde_json::json;

use crate::shell::admin::CAPTURE_TTL_SECS;
use crate::shell::ingest::AppState;

const SOURCES_HTML: &str = include_str!("../../templates/sources.html");
const CAPTURES_HTML: &str = include_str!("../../templates/captures.html");

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/sources", get(sources_page).post(create_source))
        .route("/sources/reload", post(reload_profiles))
        .route("/sources/{source_id}/delete", post(delete_source))
        .route("/profiles/{file_name}/delete", post(delete_unusable))
        .route("/calendars", post(create_calendar))
        .route("/calendars/{calendar_id}/delete", post(delete_calendar))
        .route("/captures", get(captures_page))
        // 3.x addresses keep working: bookmarks and runbooks point here.
        .route("/dashboard", get(|| async { Redirect::to("/") }))
        .route(
            "/dashboard/sources",
            get(|| async { Redirect::to("/sources") }),
        )
        .route(
            "/dashboard/captures",
            get(|| async { Redirect::to("/captures") }),
        )
}

async fn sources_page(
    Extension(dash): Extension<Dashboard>,
    State(state): State<Arc<AppState>>,
) -> Response {
    render_sources(&dash, &state, None, None).await
}

/// Renders the Sources page, optionally with an error and the values
/// that were typed, so a refusal keeps what the person entered.
async fn render_sources(
    dash: &Dashboard,
    state: &AppState,
    error: Option<&str>,
    draft: Option<(&str, &str)>,
) -> Response {
    // Rendered for a person: "3 Sep 2026, 03:47" with "2 hours ago"
    // beside it, in Kenny's own zone. The stored value stays as issued.
    let zone: chrono_tz::Tz = "Europe/Brussels".parse().unwrap_or(chrono_tz::UTC);
    let now = chrono::Utc::now();
    // The tokens are the kit's clients now (A2-1): a source has one when
    // an active client under its name exists on the kit's Sources page.
    let issued: HashMap<String, String> = dash
        .clients
        .snapshot()
        .clients
        .iter()
        .filter(|c| c.token.is_some())
        .map(|c| {
            let when = crate::core::humanise::timestamp(&c.issued_at, zone);
            let rendered = match crate::core::humanise::how_long_ago(&c.issued_at, now) {
                Some(ago) => format!("{when} ({ago})"),
                None => when,
            };
            (c.name.clone(), rendered)
        })
        .collect();
    let loaded = state.profiles();
    let mut profiles: Vec<_> = loaded.values().collect();
    profiles.sort_by(|a, b| a.source_id.cmp(&b.source_id));
    let rows: Vec<serde_json::Value> = profiles
        .iter()
        .map(|p| {
            json!({
                "source_id": p.source_id,
                "target_calendar_id": p.target_calendar_id,
                "schema_version": p.schema_version,
                "token_issued": issued.get(&p.source_id),
            })
        })
        .collect();
    // Files on disk that are not being served. Shown rather than only
    // logged: a source that stopped working is invisible otherwise, and
    // the fix — delete it — belongs on the same page (K23).
    let unusable: Vec<serde_json::Value> = crate::shell::profiles::load_all(&state.profiles_dir)
        .unusable
        .iter()
        .map(|u| json!({ "file_name": u.file_name(), "reason": u.reason }))
        .collect();
    // Without an owner a created calendar would belong to the service
    // account and be visible to nobody, so the form says so (K24).
    let can_create = state.calendar_owner.is_some();
    // Fetched on render so the dropdown shows what exists. A failure here
    // must not take the page down: the profiles are what someone came for
    // when Google is unreachable.
    let (calendars, calendar_error) = match state.client.list_calendars().await {
        Ok(calendars) => (
            state.without_deleted_calendars(state.with_created_calendars(calendars)),
            None,
        ),
        Err(e) => (Vec::new(), Some(e.to_string())),
    };
    let calendar_rows: Vec<serde_json::Value> = calendars
        .iter()
        .map(|(id, name)| {
            let in_use_by: Vec<&str> = profiles
                .iter()
                .filter(|p| &p.target_calendar_id == id)
                .map(|p| p.source_id.as_str())
                .collect();
            json!({ "id": id, "name": name, "in_use_by": in_use_by })
        })
        .collect();
    let (draft_source, draft_calendar) = draft.unwrap_or(("", ""));
    match dash.render_project(
        "/sources",
        SOURCES_HTML,
        json!({
            "error": error,
            "profiles": rows,
            "profiles_dir": state.profiles_dir.display().to_string(),
            "unusable": unusable,
            "calendars": calendar_rows,
            "calendar_error": calendar_error,
            "can_create": can_create,
            "draft_source": draft_source,
            "draft_calendar": draft_calendar,
        }),
    ) {
        Ok(html) => html.into_response(),
        Err(e) => e.into_response(),
    }
}

/// What the add-a-source form sends (K21): a name and a calendar id from
/// the dropdown.
#[derive(Deserialize)]
struct NewSource {
    source_id: String,
    calendar: String,
}

/// `POST /sources` — resolve the calendar, write the profile, reload
/// (K21). The token is issued on the kit's Sources page afterwards.
async fn create_source(
    Extension(dash): Extension<Dashboard>,
    State(state): State<Arc<AppState>>,
    Form(form): Form<NewSource>,
) -> Response {
    let source_id = form.source_id.trim();
    let chosen = form.calendar.trim();
    let draft = Some((source_id, chosen));
    // Checked here as well as in the parser: this one names the file AND
    // becomes a URL segment, and the message should point at the field
    // the person just typed rather than at a TOML they never saw.
    if !crate::core::profile::source_id_is_safe(source_id) {
        return render_sources(
            &dash,
            &state,
            Some(&format!(
                "\"{source_id}\" cannot be a source name — use letters, digits, '.', '-' and '_', and do not start with a dot."
            )),
            draft,
        )
        .await;
    }
    if chosen.is_empty() {
        return render_sources(
            &dash,
            &state,
            Some("Choose a calendar for this source."),
            draft,
        )
        .await;
    }
    let toml = crate::core::profile::default_profile_toml(source_id, chosen);
    if let Err(e) = crate::shell::profiles::save_new(&state.profiles_dir, &toml) {
        return render_sources(&dash, &state, Some(&e.to_string()), draft).await;
    }
    // Reload from disk rather than inserting the parsed profile: reading
    // it back is the only proof that what was written can be read again.
    state.set_profiles(crate::shell::profiles::load_map(&state.profiles_dir));
    tracing::info!(source_id = %source_id, "added a source from the dashboard");
    Redirect::to("/sources").into_response()
}

/// `POST /sources/{source_id}/delete` — remove a source entirely (K21):
/// its profile and, if one exists, its client on the kit's Sources page.
///
/// The events it already put on the calendar are left alone (Kenny,
/// 2026-09-03): deleting a source says something about the source, not
/// about what already happened.
async fn delete_source(
    Extension(dash): Extension<Dashboard>,
    State(state): State<Arc<AppState>>,
    Path(source_id): Path<String>,
) -> Response {
    if !state.profiles().contains_key(&source_id) {
        return (StatusCode::NOT_FOUND, "no such source").into_response();
    }
    // Refused while anything of this source's is still waiting: the
    // worker needs the profile to know which calendar an entry belongs to.
    let waiting = match state.journal.pending() {
        Ok(pending) => pending
            .iter()
            .filter(|entry| entry.source_id == source_id)
            .count(),
        Err(e) => return render_sources(&dash, &state, Some(&e.to_string()), None).await,
    };
    if waiting > 0 {
        let message = format!(
            "{source_id} still has {waiting} event(s) waiting to be delivered. \
             Deleting it now would leave them in the journal with no profile to deliver them by. \
             Wait for the queue to drain, or fix whatever is blocking delivery, and delete it then."
        );
        return render_sources(&dash, &state, Some(&message), None).await;
    }
    // The source's token goes with it, so a deleted source cannot keep
    // posting; a source without a client is simply a profile.
    let client = dash
        .clients
        .snapshot()
        .clients
        .into_iter()
        .find(|c| c.name == source_id);
    if let Some(client) = client {
        let id = client.id.clone();
        if let Err(e) = dash.clients.update(&mut |file| {
            file.clients.retain(|c| c.id != id);
            Ok(client.clone())
        }) {
            return render_sources(&dash, &state, Some(&e.to_string()), None).await;
        }
    }
    let removed = match crate::shell::profiles::delete(&state.profiles_dir, &source_id) {
        Ok(path) => path,
        Err(e) => return render_sources(&dash, &state, Some(&e.to_string()), None).await,
    };
    state.set_profiles(crate::shell::profiles::load_map(&state.profiles_dir));
    tracing::info!(
        source_id = %source_id,
        removed = %removed.display(),
        "deleted a source from the dashboard"
    );
    Redirect::to("/sources").into_response()
}

/// What the make-a-calendar form sends (K24).
#[derive(Deserialize)]
struct NewCalendar {
    name: String,
}

/// `POST /calendars` — make one and share it (K24).
async fn create_calendar(
    Extension(dash): Extension<Dashboard>,
    State(state): State<Arc<AppState>>,
    Form(form): Form<NewCalendar>,
) -> Response {
    let name = form.name.trim();
    if name.is_empty() {
        return render_sources(&dash, &state, Some("Name the calendar."), None).await;
    }
    let Some(owner) = state.calendar_owner.as_deref() else {
        return render_sources(
            &dash,
            &state,
            Some(
                "ALMANAC_CALENDAR_OWNER is not set — without an owner to share it with, a \
                 calendar Almanac creates belongs to the service account and is visible to \
                 nobody.",
            ),
            None,
        )
        .await;
    };
    // Find-or-create, serialized per name and consulted against what
    // almanac made moments ago: Google's list lags a create by seconds,
    // and two clicks inside that window would both create.
    let lock = state.locks.for_key(&format!("calendar:{name}")).await;
    let _guard = lock.lock().await;
    if let Some(id) = state.remembered_calendar(name) {
        tracing::info!(
            calendar = %name,
            id = %id,
            "a calendar with that name was just created; reusing it rather than making a second"
        );
        return Redirect::to("/sources").into_response();
    }
    match state.client.ensure_calendar(name, owner).await {
        Ok((id, created)) => {
            if created {
                state.remember_created_calendar(name, &id);
                tracing::info!(
                    calendar = %name,
                    id = %id,
                    shared_with = %owner,
                    role = "owner",
                    "created a calendar from the dashboard and shared it"
                );
            }
            Redirect::to("/sources").into_response()
        }
        Err(e) => render_sources(&dash, &state, Some(&e.to_string()), None).await,
    }
}

/// `POST /calendars/{calendar_id}/delete` — remove a calendar and
/// everything on it (K24). Guarded on arrival as well as in the page: a
/// source can be added between the render and the click.
async fn delete_calendar(
    Extension(dash): Extension<Dashboard>,
    State(state): State<Arc<AppState>>,
    Path(calendar_id): Path<String>,
) -> Response {
    let users: Vec<String> = state
        .profiles()
        .values()
        .filter(|p| p.target_calendar_id == calendar_id)
        .map(|p| p.source_id.clone())
        .collect();
    if !users.is_empty() {
        return render_sources(
            &dash,
            &state,
            Some(&format!(
                "{} still writes to that calendar, so it cannot be deleted yet. Delete the \
                 source first — its events stay on the calendar either way.",
                users.join(", ")
            )),
            None,
        )
        .await;
    }
    match state.client.delete_calendar(&calendar_id).await {
        Ok(()) => {
            state.remember_deleted_calendar(&calendar_id);
            state.forget_created_calendar(&calendar_id);
            tracing::info!(calendar_id = %calendar_id, "deleted a calendar from the dashboard");
            Redirect::to("/sources").into_response()
        }
        Err(e) => render_sources(&dash, &state, Some(&e.to_string()), None).await,
    }
}

/// `POST /profiles/{file_name}/delete` — remove a file the service cannot
/// use (K23). Addressed by file name: a broken profile often has no
/// readable source id, which is frequently the thing wrong with it.
async fn delete_unusable(
    Extension(dash): Extension<Dashboard>,
    State(state): State<Arc<AppState>>,
    Path(file_name): Path<String>,
) -> Response {
    let unusable = crate::shell::profiles::load_all(&state.profiles_dir).unusable;
    if !unusable.iter().any(|u| u.file_name() == file_name) {
        return (StatusCode::NOT_FOUND, "no such unusable profile").into_response();
    }
    match crate::shell::profiles::delete_file(&state.profiles_dir, &file_name) {
        Ok(removed) => {
            state.set_profiles(crate::shell::profiles::load_map(&state.profiles_dir));
            tracing::info!(removed = %removed.display(), "deleted an unusable profile from the dashboard");
            Redirect::to("/sources").into_response()
        }
        Err(e) => render_sources(&dash, &state, Some(&e.to_string()), None).await,
    }
}

/// `POST /sources/reload` — re-read the profiles directory (K21), which
/// is what makes a profile placed by hand usable without a restart.
async fn reload_profiles(State(state): State<Arc<AppState>>) -> Response {
    let profiles = crate::shell::profiles::load_map(&state.profiles_dir);
    tracing::info!(
        count = profiles.len(),
        "reloaded profiles from the dashboard"
    );
    state.set_profiles(profiles);
    Redirect::to("/sources").into_response()
}

/// `GET /captures` — what the capture surface (M11) holds right now.
async fn captures_page(
    Extension(dash): Extension<Dashboard>,
    State(state): State<Arc<AppState>>,
) -> Response {
    let captures = state.captures_after_expiry().await;
    let cards: Vec<serde_json::Value> = captures
        .iter()
        .map(|c| {
            json!({
                "label": c.label,
                "at": c.at,
                "headers": c.headers,
                "body": c.body,
                "truncated_from_bytes": c.truncated_from_bytes,
            })
        })
        .collect();
    match dash.render_project(
        "/captures",
        CAPTURES_HTML,
        json!({ "captures": cards, "ttl_minutes": CAPTURE_TTL_SECS / 60 }),
    ) {
        Ok(html) => html.into_response(),
        Err(e) => e.into_response(),
    }
}
