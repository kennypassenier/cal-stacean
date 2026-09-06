//! The in-process kit harness (4.0.0): Almanac assembled exactly as the
//! binary assembles it — `almanac::shell::kit::mount` on a real
//! `chassis::App` — started on a free port with the kit's door in front,
//! Google and its token endpoint stubbed. Tests about the dashboard, the
//! door and the debug surfaces go through here.
#![allow(dead_code)]

use std::collections::{BTreeMap, HashMap};
use std::net::SocketAddr;
use std::sync::Arc;

use almanac::core::profile::Profile;
use almanac::shell::auth::TokenManager;
use almanac::shell::calendar_client::GoogleCalendarClient;
use almanac::shell::ingest::AppState;
use almanac::shell::journal::{DEFAULT_MAX_BYTES, Journal};
use almanac::shell::kit::{import_source_tokens, mount};
use almanac::shell::testing::{CalendarStub, TokenStub, stub_credentials};
use axum::Router;
use chassis::{App, AppSpec, Running};

pub const TOKEN: &str = "a-login-token-that-is-long-enough";
pub const KEY: &str = "abababababababababababababababababababababababababababababababab";

pub fn profile_toml(source_id: &str) -> String {
    format!(
        r#"
schema_version = 2
source_id = "{source_id}"
target_calendar_id = "primary"
"#
    )
}

pub struct KitHub {
    pub addr: SocketAddr,
    pub state: Arc<AppState>,
    pub calendar: CalendarStub,
    pub dir: tempfile::TempDir,
    /// The admin session cookie, `name=value`, from one login.
    pub cookie: String,
    running: Option<Running>,
    _tokens: TokenStub,
}

impl KitHub {
    pub fn url(&self, path: &str) -> String {
        format!("http://{}{path}", self.addr)
    }

    fn http() -> reqwest::Client {
        reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("a client")
    }

    /// A browser with the admin session.
    pub async fn get(&self, path: &str) -> reqwest::Response {
        Self::http()
            .get(self.url(path))
            .header("cookie", &self.cookie)
            .send()
            .await
            .expect("a response")
    }

    pub async fn get_anon(&self, path: &str) -> reqwest::Response {
        Self::http()
            .get(self.url(path))
            .send()
            .await
            .expect("a response")
    }

    pub async fn page(&self, path: &str) -> String {
        let response = self.get(path).await;
        assert_eq!(response.status(), 200, "{path} must render");
        response.text().await.expect("a body")
    }

    /// One of the dashboard's own forms, posted by the admin's browser.
    pub async fn form(&self, path: &str, body: &str) -> reqwest::Response {
        Self::http()
            .post(self.url(path))
            .header("cookie", &self.cookie)
            .header("content-type", "application/x-www-form-urlencoded")
            .body(body.to_string())
            .send()
            .await
            .expect("a response")
    }

    /// A script with a bearer token.
    pub fn bearer(
        &self,
        method: reqwest::Method,
        path: &str,
        token: &str,
    ) -> reqwest::RequestBuilder {
        Self::http()
            .request(method, self.url(path))
            .header("authorization", format!("Bearer {token}"))
    }

    pub async fn post_json(
        &self,
        path: &str,
        token: Option<&str>,
        body: &str,
    ) -> reqwest::Response {
        let mut request = Self::http()
            .post(self.url(path))
            .header("content-type", "application/json")
            .body(body.to_string());
        if let Some(token) = token {
            request = request.header("authorization", format!("Bearer {token}"));
        }
        request.send().await.expect("a response")
    }

    /// Issue a client token on the kit's Sources page (as the admin) and
    /// return the token, the way a person would with Reveal.
    pub async fn issue_client(&self, name: &str) -> String {
        let issued = Self::http()
            .post(self.url("/api/clients"))
            .header("cookie", &self.cookie)
            .header("content-type", "application/json")
            // 4.0.2: the issue form carries the calendar; a source with a
            // profile on disk keeps it, a new one gets this test calendar.
            .body(format!(r#"{{"name":"{name}","calendar":"cal-test"}}"#))
            .send()
            .await
            .expect("a response");
        assert_eq!(issued.status(), 201, "issuing {name}");
        let view: serde_json::Value = issued.json().await.expect("a client view");
        let id = view["id"].as_str().expect("an id").to_string();
        let revealed = self.get(&format!("/api/clients/{id}/token")).await;
        assert_eq!(revealed.status(), 200);
        let body: serde_json::Value = revealed.json().await.expect("json");
        body["token"].as_str().expect("the token").to_string()
    }

    pub async fn shutdown(mut self) {
        if let Some(running) = self.running.take() {
            running.stop().await;
        }
    }
}

pub async fn body_json(response: reqwest::Response) -> serde_json::Value {
    let text = response.text().await.expect("a body");
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("expected JSON, got {text:?}: {e}"))
}

pub fn urlencode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}

/// A hub with one profile (`home-assistant` on `primary`), no owner.
pub async fn spawn_kit() -> KitHub {
    spawn_kit_with(&["home-assistant"], None).await
}

/// A hub with the given profiles written to disk and an optional calendar
/// owner (K24 needs one to create calendars).
pub async fn spawn_kit_with(sources: &[&str], owner: Option<&str>) -> KitHub {
    let dir = tempfile::tempdir().expect("a temp dir");
    let profiles_dir = dir.path().join("profiles");
    std::fs::create_dir_all(&profiles_dir).unwrap();
    for source in sources {
        std::fs::write(
            profiles_dir.join(format!("{source}.toml")),
            profile_toml(source),
        )
        .unwrap();
    }
    spawn_kit_in(dir, owner).await
}

pub async fn spawn_kit_in(dir: tempfile::TempDir, owner: Option<&str>) -> KitHub {
    let profiles_dir = dir.path().join("profiles");
    std::fs::create_dir_all(&profiles_dir).unwrap();
    let mut env: BTreeMap<String, String> = BTreeMap::new();
    env.insert("ALMANAC_STATE_DIR".into(), dir.path().display().to_string());
    env.insert("ALMANAC_TOKEN".into(), TOKEN.into());
    env.insert("ALMANAC_SECRET_KEY".into(), KEY.into());
    env.insert("ALMANAC_LISTEN".into(), "127.0.0.1:0".into());
    env.insert("ALMANAC_LOG".into(), "warn".into());
    let spec = AppSpec {
        name: "almanac",
        version: env!("CARGO_PKG_VERSION"),
        repository: Some("kennypassenier/almanac"),
        ..Default::default()
    };
    let mut app = App::from_args_with_env(spec, vec!["almanac".into()], env, Router::new())
        .expect("the kit accepts the test configuration");
    let state_dir = app
        .loaded
        .as_ref()
        .expect("a start loads configuration")
        .state_dir
        .clone();
    let calendar = CalendarStub::start().await;
    let tokens = TokenStub::start(3600).await;
    let http = reqwest::Client::new();
    let profiles: HashMap<String, Profile> = almanac::shell::profiles::load_map(&profiles_dir);
    let state = Arc::new(
        AppState::new(
            profiles,
            Journal::new(dir.path().join("journal.jsonl"), DEFAULT_MAX_BYTES),
            GoogleCalendarClient::with_base_url(
                http.clone(),
                TokenManager::new(http, stub_credentials(&tokens.url)),
                &calendar.base_url,
            ),
        )
        .with_profiles_dir(profiles_dir)
        .with_calendar_owner(owner.map(str::to_string)),
    );
    // The 3.x token store, when a test seeded one, is imported like the
    // binary does it on the first start of 4.0.0.
    import_source_tokens(&state_dir, &dir.path().join("tokens.json"), KEY)
        .await
        .expect("the import runs");
    mount(&mut app, Arc::clone(&state));
    let running = app.start().await.expect("the kit starts");
    let addr = running.addr;
    let login = KitHub::http()
        .post(format!("http://{addr}/login"))
        .header("content-type", "application/x-www-form-urlencoded")
        .body(format!("token={}", urlencode(TOKEN)))
        .send()
        .await
        .expect("a login response");
    assert_eq!(login.status(), 303, "the right token logs in");
    let cookie = login
        .headers()
        .get("set-cookie")
        .expect("a session cookie")
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string();
    KitHub {
        addr,
        state,
        calendar,
        dir,
        cookie,
        running: Some(running),
        _tokens: tokens,
    }
}
