//! What the real binary does when it is started, signalled and started
//! twice (T5, T8, T10).
//!
//! Everything here spawns `almanac` as an actual process, because that
//! is the only way to prove any of it. `main.rs` had no tests at all,
//! so SIGTERM handling, the startup retry loop and the data-directory
//! lock across two processes were all unproven — and each of them
//! matters most in exactly the situation nobody is watching: a restart,
//! a power cut, a self-update handover.
//!
//! The service-account key is generated per run rather than committed.
//! A throwaway key in the repository would grant nothing, but it would
//! still be a private key in git, and this costs a fraction of a second
//! to avoid that.

use std::io::Write;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

fn scratch(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "almanac-lifecycle-{}-{}-{name}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();

    let mut file = std::fs::File::create(dir.join("test.toml")).unwrap();
    file.write_all(
        br#"
schema_version = 2
source_id = "test-source"
target_calendar_id = "primary"

"#,
    )
    .unwrap();
    dir
}

/// A fresh RSA key, so the binary can sign a JWT and reach the network
/// step. Never reused, never committed.
fn generate_key(dir: &std::path::Path) -> String {
    let path = dir.join("key.pem");
    let status = Command::new("openssl")
        .args([
            "genpkey",
            "-algorithm",
            "RSA",
            "-pkeyopt",
            "rsa_keygen_bits:2048",
            "-out",
        ])
        .arg(&path)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("openssl is needed to generate a throwaway key for this test");
    assert!(status.success(), "openssl could not generate a key");
    std::fs::read_to_string(&path).unwrap()
}

/// A token endpoint that never answers, so the first token fetch fails
/// the way an unreachable Google does. Port 1 is reserved and refuses
/// immediately, which is a connection failure rather than a hang.
const BLACKHOLED_TOKEN_URL: &str = "http://127.0.0.1:1/token";

fn spawn(dir: &std::path::Path, key: &str, token_url: &str, extra: &[(&str, &str)]) -> Child {
    let mut command = Command::new(env!("CARGO_BIN_EXE_almanac"));
    command
        .env("ALMANAC_STATE_DIR", dir)
        // 3.0.0: the kit binds; `0` picks a free port so parallel tests never collide.
        .env("ALMANAC_LISTEN", "127.0.0.1:0")
        .env("ALMANAC_PROFILES_DIR", dir)
        .env("ALMANAC_DATA_DIR", dir)
        .env("ALMANAC_JOURNAL", dir.join("journal.jsonl"))
        .env("ALMANAC_TOKEN_STORE", dir.join("tokens.json"))
        .env("ALMANAC_SECRET_KEY", "1".repeat(64))
        .env("ALMANAC_TOKEN", "a-login-token-for-the-tests")
        .env("CLIENT_EMAIL", "test@example.iam.gserviceaccount.com")
        .env("PRIVATE_KEY", key)
        .env("TOKEN_URI", token_url)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (k, v) in extra {
        command.env(k, v);
    }
    command.spawn().expect("failed to start almanac")
}

/// Waits until `predicate` holds, or fails after `limit`.
fn wait_until(limit: Duration, mut predicate: impl FnMut() -> bool, what: &str) {
    let deadline = Instant::now() + limit;
    while Instant::now() < deadline {
        if predicate() {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    panic!("timed out waiting for {what}");
}

fn still_running(child: &mut Child) -> bool {
    matches!(child.try_wait(), Ok(None))
}

#[test]
fn an_unreachable_google_makes_it_retry_rather_than_exit() {
    // AR21, and SCOPE criterion 3. After a power cut the LXC can start
    // Almanac before the network settles. If a network failure were
    // treated as permanent the unit would exit, and with no start
    // limit systemd would restart-loop it forever; if a genuinely
    // broken key were treated as transient it would look alive while
    // doing nothing. This is the transient half, which nothing covered.
    let dir = scratch("startup-retry");
    let key = generate_key(&dir);
    let mut child = spawn(&dir, &key, BLACKHOLED_TOKEN_URL, &[]);

    // The first backoff step is 2s; give it room and then some.
    std::thread::sleep(Duration::from_secs(4));

    let alive = still_running(&mut child);
    child.kill().ok();
    let output = child.wait_with_output().unwrap();
    let printed = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    std::fs::remove_dir_all(&dir).ok();

    assert!(
        alive,
        "an unreachable token endpoint must not end the process:\n{printed}"
    );
    assert!(
        printed.contains("retrying"),
        "and it should say what it is doing:\n{printed}"
    );
}

#[test]
fn a_broken_private_key_exits_instead_of_retrying_forever() {
    // The other half of AR21: a malformed key never fixes itself, so
    // retrying it forever would leave Almanac looking alive while
    // delivering nothing.
    let dir = scratch("startup-permanent");
    let mut child = spawn(&dir, "not-a-pem", BLACKHOLED_TOKEN_URL, &[]);

    wait_until(
        Duration::from_secs(10),
        || !still_running(&mut child),
        "a broken key to end the process",
    );

    let output = child.wait_with_output().unwrap();
    let printed = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    std::fs::remove_dir_all(&dir).ok();

    assert!(!output.status.success(), "it must exit non-zero");
    assert!(
        printed.contains("private key"),
        "and name the problem:\n{printed}"
    );
}

#[test]
fn a_second_process_on_the_same_data_directory_refuses_to_start() {
    // AR22 across two real processes, which is the scenario the lock
    // exists for — the existing tests take the flock twice from within
    // one process. Two workers over one journal deliver the same event
    // twice and can lose delivery records.
    let dir = scratch("two-processes");
    let key = generate_key(&dir);

    let mut first = spawn(&dir, &key, BLACKHOLED_TOKEN_URL, &[]);
    // Let it take the lock before the second one tries.
    wait_until(
        Duration::from_secs(10),
        || dir.join(".lock").exists(),
        "the first process to take the lock",
    );
    std::thread::sleep(Duration::from_millis(500));

    let second = spawn(&dir, &key, BLACKHOLED_TOKEN_URL, &[])
        .wait_with_output()
        .unwrap();
    let printed = format!(
        "{}{}",
        String::from_utf8_lossy(&second.stdout),
        String::from_utf8_lossy(&second.stderr)
    );

    let first_alive = still_running(&mut first);
    first.kill().ok();
    first.wait().ok();
    std::fs::remove_dir_all(&dir).ok();

    assert!(
        !second.status.success(),
        "the second process must refuse to start:\n{printed}"
    );
    assert!(printed.contains("already holds"), "and say why:\n{printed}");
    assert!(
        first_alive,
        "while the first keeps running — the lock protects it, it does not kill it"
    );
}

#[test]
fn check_mode_runs_against_a_live_instance_without_disturbing_it() {
    // AR22/AR29: the self-update probe runs `--check` while the old
    // process is still serving, so it must take neither the port nor
    // the data-directory lock. If a future change made it reuse the
    // startup path — easy to do — every self-update probe would fail
    // against the running instance and updates would stop, quietly.
    let dir = scratch("check-against-live");
    let key = generate_key(&dir);

    let mut running = spawn(&dir, &key, BLACKHOLED_TOKEN_URL, &[]);
    wait_until(
        Duration::from_secs(10),
        || dir.join(".lock").exists(),
        "the running instance to take the lock",
    );
    std::thread::sleep(Duration::from_millis(500));

    let check = Command::new(env!("CARGO_BIN_EXE_almanac"))
        .arg("--check")
        .env("ALMANAC_STATE_DIR", &dir)
        .env("ALMANAC_LISTEN", "127.0.0.1:0")
        .env("ALMANAC_PROFILES_DIR", &dir)
        .env("ALMANAC_DATA_DIR", &dir)
        .env("ALMANAC_JOURNAL", dir.join("journal.jsonl"))
        .env("ALMANAC_TOKEN_STORE", dir.join("tokens.json"))
        .env("ALMANAC_SECRET_KEY", "1".repeat(64))
        .env("ALMANAC_TOKEN", "a-login-token-for-the-tests")
        .env("CLIENT_EMAIL", "test@example.iam.gserviceaccount.com")
        .env("PRIVATE_KEY", &key)
        .env("TOKEN_URI", BLACKHOLED_TOKEN_URL)
        .output()
        .unwrap();

    let printed = format!(
        "{}{}",
        String::from_utf8_lossy(&check.stdout),
        String::from_utf8_lossy(&check.stderr)
    );
    let first_alive = still_running(&mut running);
    running.kill().ok();
    running.wait().ok();
    std::fs::remove_dir_all(&dir).ok();

    assert!(
        check.status.success(),
        "--check must succeed against a live instance:\n{printed}"
    );
    assert!(
        printed.contains("--check: ok"),
        "and report it plainly:\n{printed}"
    );
    assert!(first_alive, "and leave the running instance alone");
}

/// A token endpoint that answers, so the process gets past the
/// startup auth loop and actually binds a listener.
async fn serve_token_stub() -> String {
    use axum::Router;
    async fn token() -> axum::Json<serde_json::Value> {
        axum::Json(serde_json::json!({"access_token": "stub", "expires_in": 3600}))
    }
    let router = Router::new().route("/token", axum::routing::post(token));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, router).await.ok();
    });
    format!("http://{address}/token")
}

/// A port nothing is using, released before the caller binds it.
fn free_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap().port()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sigterm_stops_it_cleanly_after_it_is_serving() {
    // M2, and AR30 rides on the same path: self-update restarts by
    // raising SIGTERM on itself. If this exits non-zero or hangs past
    // TimeoutStopSec, systemd SIGKILLs it — possibly mid journal
    // write — during the operation that happens most often.
    //
    // The process has to be genuinely serving first: the graceful
    // handler is only installed once the listener is bound, so
    // signalling it during the startup retry loop would prove nothing.
    let dir = scratch("sigterm");
    let key = generate_key(&dir);
    let token_url = serve_token_stub().await;
    let port = free_port();

    let mut child = spawn(
        &dir,
        &key,
        &token_url,
        &[("ALMANAC_LISTEN", &format!("127.0.0.1:{port}"))],
    );

    let health = format!("http://127.0.0.1:{port}/healthz");
    let client = reqwest::Client::new();
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut serving = false;
    while Instant::now() < deadline {
        if client.get(&health).send().await.is_ok() {
            serving = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(serving, "the process never started serving");

    let pid = child.id() as i32;
    // SIGTERM is what systemd and `docker stop` send.
    assert_eq!(
        unsafe { libc::kill(pid, libc::SIGTERM) },
        0,
        "failed to signal the process"
    );

    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline && still_running(&mut child) {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(
        !still_running(&mut child),
        "SIGTERM must stop it well within TimeoutStopSec, not need a SIGKILL"
    );

    let output = child.wait_with_output().unwrap();
    let printed = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    std::fs::remove_dir_all(&dir).ok();

    assert!(
        output.status.success(),
        "a graceful stop must exit zero, got {:?}:\n{printed}",
        output.status
    );
    // 3.0.0: the kit logs the stop ("stopped drained=true"); almanac's own
    // lines below still prove the drain ran and finished.
    assert!(
        printed.contains("stopped"),
        "the stop should be visible in the log:\n{printed}"
    );
    assert!(
        printed.contains("draining the worker"),
        "and the drain must actually run — that is the whole point:\n{printed}"
    );
    assert!(
        printed.contains("almanac stopped"),
        "and it should say it finished:\n{printed}"
    );
}

#[test]
fn version_needs_no_configuration_at_all() {
    // Found by the homelab session running the binary by hand outside
    // `latch run` to sanity-check a deploy: every other special mode
    // needs the full production configuration (by design — they answer
    // "can this run here"), so asking "what version is this" started
    // the whole process and complained about a missing webhook and an
    // unreadable profiles directory instead of just answering.
    let output = Command::new(env!("CARGO_BIN_EXE_almanac"))
        .arg("--version")
        .env_clear()
        .output()
        .unwrap();

    let printed = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    assert!(
        output.status.success(),
        "--version must succeed with no environment at all, got {:?}:\n{printed}",
        output.status
    );
    assert!(
        printed.contains(env!("CARGO_PKG_VERSION")),
        "it should print the version it was actually built as:\n{printed}"
    );
    assert!(
        !printed.contains("ALMANAC_NOTIFY_WEBHOOK"),
        "it must not touch anything that needs configuration:\n{printed}"
    );

    let short = Command::new(env!("CARGO_BIN_EXE_almanac"))
        .arg("-V")
        .env_clear()
        .output()
        .unwrap();
    assert!(short.status.success(), "-V is the same flag, spelled short");
}
