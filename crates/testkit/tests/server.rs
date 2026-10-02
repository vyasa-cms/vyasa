//! The server harness reports why a server did not come up.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::time::{Duration, Instant};
use vyasa_testkit::{TestDb, TestServer};

fn panic_text(err: &(dyn std::any::Any + Send)) -> String {
    err.downcast_ref::<String>()
        .cloned()
        .or_else(|| err.downcast_ref::<&str>().map(|s| (*s).to_owned()))
        .unwrap_or_default()
}

#[tokio::test]
async fn early_exit_reports_stderr() {
    let db = TestDb::new().await;
    let started = Instant::now();
    let err = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        TestServer::builder("sh", &db)
            .args(["-c", "echo boom-marker >&2; exit 3"])
            .timeout(Duration::from_secs(20))
            .start()
    }))
    .expect_err("a server that exits must fail the test");
    let text = panic_text(&*err);
    assert!(text.contains("boom-marker"), "{text}");
    assert!(text.contains("exited"), "{text}");
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "reported without waiting for the timeout"
    );
}

#[tokio::test]
async fn timeout_reports_stderr() {
    let db = TestDb::new().await;
    let err = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        TestServer::builder("sh", &db)
            .args(["-c", "echo still-booting >&2; sleep 30"])
            .timeout(Duration::from_millis(800))
            .start()
    }))
    .expect_err("a server that never listens must fail the test");
    let text = panic_text(&*err);
    assert!(text.contains("still-booting"), "{text}");
    assert!(text.contains("did not come up"), "{text}");
}

#[tokio::test]
async fn env_rejects_vyasa_bind_addr() {
    let db = TestDb::new().await;
    let err = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        TestServer::builder("sh", &db).env("VYASA_BIND_ADDR", "127.0.0.1:1")
    }))
    .expect_err("overriding the harness's bind address must panic");
    let text = panic_text(&*err);
    assert!(text.contains("VYASA_BIND_ADDR"), "{text}");
    assert!(text.contains("owns that port"), "{text}");
}

/// An ambient `VYASA_*` variable (a developer's shell, or the repo's own
/// `.env`) must never reach the child: a test of "no secret key configured"
/// or "no AI provider key configured" would otherwise pass or fail
/// depending on what happens to be exported outside the test, not on what
/// the test itself set up.
#[tokio::test]
async fn ambient_vyasa_env_is_scrubbed() {
    let db = TestDb::new().await;
    // Unique to this test; nothing else reads or sets it, and it is
    // cleared again below regardless of how the assertions come out.
    std::env::set_var("VYASA_TESTKIT_SCRUB_PROBE", "leak-me");
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        TestServer::builder("sh", &db)
            // Filtered rather than a bare `env`: the harness only keeps the
            // last 50 stderr lines, and a real shell's full environment can
            // easily run past that, which would make this assertion flaky
            // for a reason that has nothing to do with scrubbing.
            .args(["-c", "env | grep '^VYASA_' >&2; exit 3"])
            .timeout(Duration::from_secs(20))
            .start()
    }));
    std::env::remove_var("VYASA_TESTKIT_SCRUB_PROBE");
    let err = result.expect_err("a server that exits must fail the test");
    let text = panic_text(&*err);
    assert!(
        !text.contains("VYASA_TESTKIT_SCRUB_PROBE"),
        "an ambient VYASA_ variable reached the child: {text}"
    );
    assert!(text.contains("VYASA_DATABASE_URL"), "{text}");
}

/// Set on the child when this test binary is re-run as a stand-in server.
const FAKE_SERVER: &str = "TESTKIT_FAKE_SERVER";

/// The stand-in server the port tests below spawn: this test binary run
/// again as `--exact fake_server` with [`FAKE_SERVER`] set. It binds
/// `VYASA_BIND_ADDR` and reports a failed bind the way `vyasa serve` does.
/// Run normally (without the variable) it does nothing.
#[test]
fn fake_server() {
    if std::env::var_os(FAKE_SERVER).is_none() {
        return;
    }
    let addr = std::env::var("VYASA_BIND_ADDR").expect("bind addr");
    match std::net::TcpListener::bind(&addr) {
        Ok(listener) => {
            for stream in listener.incoming() {
                drop(stream);
            }
        }
        Err(err) => {
            eprintln!("vyasa serve: cannot bind {addr}: {err}");
            std::process::exit(1);
        }
    }
}

fn fake_server_builder(db: &TestDb) -> vyasa_testkit::TestServerBuilder {
    TestServer::builder(std::env::current_exe().expect("test binary"), db)
        .args(["--exact", "fake_server", "--nocapture"])
        .env(FAKE_SERVER, "1")
        .timeout(Duration::from_secs(20))
}

/// The port the harness picked is taken by someone else before our server
/// binds it (the race [`TestServer`]'s port reservation closes; forced
/// here with `first_port`). The server's "Address already in use" exit is
/// retried on a fresh port instead of failing the test.
///
/// The port is held by a connected client socket rather than a listener,
/// so nothing answers the readiness probe on it and the outcome does not
/// depend on how fast the child exits.
#[tokio::test]
async fn a_taken_port_is_retried_on_a_fresh_one() {
    let db = TestDb::new().await;
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let client = std::net::TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    let taken = client.local_addr().unwrap().port();
    let server = fake_server_builder(&db).first_port(taken).start();
    assert_ne!(server.port(), taken);
    std::net::TcpStream::connect(("127.0.0.1", server.port())).expect("our server listens");
}

/// A server that keeps failing to bind still fails the test, with its
/// stderr, after the one retry.
#[tokio::test]
async fn a_second_address_in_use_exit_fails_the_test() {
    let db = TestDb::new().await;
    let started = Instant::now();
    let err = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        TestServer::builder("sh", &db)
            .args([
                "-c",
                "echo \"cannot bind: Address already in use (os error 98)\" >&2; exit 1",
            ])
            .timeout(Duration::from_secs(20))
            .start()
    }))
    .expect_err("a server that never binds must fail the test");
    let text = panic_text(&*err);
    assert!(text.contains("exited"), "{text}");
    assert!(text.contains("Address already in use"), "{text}");
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "no timeout wait"
    );
}

/// A server binds the port the harness holds for it (both sockets set
/// `SO_REUSEADDR`, only the server listens), and servers started together
/// get distinct ports.
#[tokio::test]
async fn concurrent_servers_get_distinct_ports() {
    let db = TestDb::new().await;
    let servers: Vec<TestServer> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..8)
            .map(|_| scope.spawn(|| fake_server_builder(&db).start()))
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });
    let mut ports: Vec<u16> = servers.iter().map(TestServer::port).collect();
    ports.sort_unstable();
    ports.dedup();
    assert_eq!(ports.len(), servers.len());
}
