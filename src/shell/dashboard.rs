//! Almanac's own dashboard pages on the kit (M12 as amended 2026-09-06,
//! step 2 of the chassis migration): **Sources** — the mapping profiles,
//! the calendars they write to and the files that could not be loaded —
//! only. The kit brings the layout, the login and session,
//! the clients page (labelled "Sources" here: the per-source tokens),
//! CSRF and CSP; this module fills `content` with minijinja templates.
//!
//! The UI is English per standing rule 1 (Dutch is for conversation and
//! for dashboards meant for Kenny's parents; this is an operator tool).

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Extension, Form, Router};
use chassis::Dashboard;
use serde::Deserialize;
use serde_json::json;

use crate::shell::ingest::AppState;

const CALENDARS_HTML: &str = include_str!("../../templates/calendars.html");

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        // 4.0.2: sources live on the kit's Sources page (profile + token in
        // one issue, S1); Almanac's own page keeps the calendars and the
        // profile files on disk.
        .route("/calendars", get(calendars_page).post(create_calendar))
        .route("/calendars/{calendar_id}/delete", post(delete_calendar))
        .route("/sources/reload", post(reload_profiles))
        .route("/profiles/{file_name}/delete", post(delete_unusable))
        // Older addresses keep working: bookmarks and runbooks point here.
        .route("/sources", get(|| async { Redirect::to("/calendars") }))
        .route("/dashboard", get(|| async { Redirect::to("/") }))
        .route(
            "/dashboard/sources",
            get(|| async { Redirect::to("/calendars") }),
        )
        .route(
            "/dashboard/captures",
            get(|| async { Redirect::to("/clients") }),
        )
        .route("/captures", get(|| async { Redirect::to("/clients") }))
}

async fn calendars_page(
    Extension(dash): Extension<Dashboard>,
    State(state): State<Arc<AppState>>,
) -> Response {
    render_calendars(&dash, &state, None).await
}

/// Renders the Sources page, optionally with an error and the values
/// that were typed, so a refusal keeps what the person entered.
async fn render_calendars(dash: &Dashboard, state: &AppState, error: Option<&str>) -> Response {
    let loaded = state.profiles();
    let mut profiles: Vec<_> = loaded.values().collect();
    profiles.sort_by(|a, b| a.source_id.cmp(&b.source_id));
    let unusable: Vec<serde_json::Value> = crate::shell::profiles::load_all(&state.profiles_dir)
        .unusable
        .iter()
        .map(|u| json!({ "file_name": u.file_name(), "reason": u.reason }))
        .collect();
    let can_create = state.calendar_owner.is_some();
    let (calendars, calendar_error) = match state.client.list_calendars().await {
        Ok(calendars) => {
            let calendars =
                state.without_deleted_calendars(state.with_created_calendars(calendars));
            state.remember_calendars(&calendars);
            (calendars, None)
        }
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
    match dash.render_project(
        "/calendars",
        CALENDARS_HTML,
        json!({
            "error": error,
            "profiles_dir": state.profiles_dir.display().to_string(),
            "unusable": unusable,
            "calendars": calendar_rows,
            "calendar_error": calendar_error,
            "can_create": can_create,
        }),
    ) {
        Ok(html) => html.into_response(),
        Err(e) => e.into_response(),
    }
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
        return render_calendars(&dash, &state, Some("Name the calendar.")).await;
    }
    let Some(owner) = state.calendar_owner.as_deref() else {
        return render_calendars(
            &dash,
            &state,
            Some(
                "ALMANAC_CALENDAR_OWNER is not set — without an owner to share it with, a \
                 calendar Almanac creates belongs to the service account and is visible to \
                 nobody.",
            ),
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
        return Redirect::to("/calendars").into_response();
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
            Redirect::to("/calendars").into_response()
        }
        Err(e) => render_calendars(&dash, &state, Some(&e.to_string())).await,
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
        return render_calendars(
            &dash,
            &state,
            Some(&format!(
                "{} still writes to that calendar, so it cannot be deleted yet. Delete the \
                 source first — its events stay on the calendar either way.",
                users.join(", ")
            )),
        )
        .await;
    }
    match state.client.delete_calendar(&calendar_id).await {
        Ok(()) => {
            state.remember_deleted_calendar(&calendar_id);
            state.forget_created_calendar(&calendar_id);
            tracing::info!(calendar_id = %calendar_id, "deleted a calendar from the dashboard");
            Redirect::to("/calendars").into_response()
        }
        Err(e) => render_calendars(&dash, &state, Some(&e.to_string())).await,
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
            Redirect::to("/calendars").into_response()
        }
        Err(e) => render_calendars(&dash, &state, Some(&e.to_string())).await,
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
    Redirect::to("/calendars").into_response()
}
