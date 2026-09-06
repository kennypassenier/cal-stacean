//! The I/O-performing shell — HTTP handlers, the Google Calendar
//! client, config/profile file loading (AR13). The only place allowed
//! to import reqwest or perform file/network I/O directly; whatever it
//! learns from the outside world reaches `core` through explicit
//! function calls and trait implementations, never the reverse.
//!
//! The ingest routes land in L3; the debug and health
//! surfaces (K11, M11, M1) follow in L4.

pub mod admin;
pub mod auth;
pub mod calendar_client;
pub mod dashboard;
pub mod datadir;
pub mod delivery;
pub mod durability;
pub mod heartbeat;
pub mod ingest;
pub mod journal;
pub mod kit;
pub mod notify;
pub mod profiles;
/// Stubs for Google and the token endpoint.
///
/// Not `#[cfg(test)]`: integration tests in `tests/` link the library
/// as an ordinary dependency and would not see it, and K21's dashboard
/// tests need to create a calendar without reaching Google. The
/// alternative was a second hand-rolled stub under `tests/` — the same
/// fixture maintained twice, which is the shape of every drift this
/// project has had to fix.
pub mod testing;
pub mod token_store;
pub mod worker;

use std::sync::Arc;

use axum::Router;

use crate::shell::ingest::AppState;

/// Builds the application's Axum router.
pub fn build_router(state: Arc<AppState>) -> Router {
    ingest::routes().merge(admin::routes()).with_state(state)
}

/// Almanac's own dashboard pages (Sources), handed to
/// `App::dashboard_routes`: the kit renders them inside its layout behind
/// the admin login and refuses cross-origin form posts (4.0.0).
pub fn pages(state: Arc<AppState>) -> Router {
    dashboard::routes().with_state(state)
}

/// [`build_router`] plus `/healthz` and `/metrics` as the kit serves them
/// (3.0.0), for in-process tests and embedders that run Almanac without
/// `chassis::App`. The binary must NOT use this: the kit mounts the same two
/// routes itself and axum refuses a second handler on a path.
pub fn build_router_with_probes(state: Arc<AppState>) -> Router {
    use axum::response::IntoResponse;
    use chassis::ScrapeSource;
    use chassis::shell::health::{Health, healthz};

    let health = Health::new(
        env!("CARGO_PKG_VERSION"),
        std::time::Duration::from_secs(2),
        vec![Arc::new(kit::JournalSubsystem(Arc::clone(&state)))],
    );
    let metrics = Arc::new(kit::AlmanacMetrics(Arc::clone(&state)));
    let probes = Router::new()
        .route("/healthz", axum::routing::get(healthz).with_state(health))
        .route(
            "/metrics",
            axum::routing::get(move || {
                let metrics = Arc::clone(&metrics);
                async move {
                    (
                        [(
                            axum::http::header::CONTENT_TYPE,
                            "text/plain; version=0.0.4; charset=utf-8",
                        )],
                        metrics.scrape(),
                    )
                        .into_response()
                }
            }),
        );
    // In-process tests have no kit door in front: every request arrives as
    // the admin, so the API handlers' `Caller` extractor finds one.
    build_router(state)
        .layer(axum::Extension(chassis::Caller::Admin))
        .merge(probes)
}
