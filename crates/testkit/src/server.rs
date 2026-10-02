//! The `vyasa` binary as a subprocess under test.

use std::collections::VecDeque;
use std::io::{BufRead, BufReader};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use socket2::{Domain, Socket, Type};

use crate::TestDb;

/// Lines of server stderr kept for a failure message.
const STDERR_LINES: usize = 50;

/// A running server, killed on drop. Its private working directory (media,
/// search index, registry) is removed on drop too.
#[derive(Debug)]
pub struct TestServer {
    base: String,
    port: u16,
    child: Child,
    work_dir: PathBuf,
}

/// Configures a [`TestServer`] before starting it.
#[derive(Debug)]
pub struct TestServerBuilder {
    bin: PathBuf,
    args: Vec<String>,
    envs: Vec<(String, String)>,
    timeout: Duration,
    current_dir: Option<PathBuf>,
    work_dir: PathBuf,
    first_port: Option<u16>,
}

impl TestServer {
    /// Starts `bin serve` against `db` with the defaults.
    #[must_use]
    pub fn start(bin: impl AsRef<Path>, db: &TestDb) -> Self {
        Self::builder(bin, db).start()
    }

    /// A builder with `VYASA_DATABASE_URL` pointing at `db`, quiet logs,
    /// `serve` as the argument and a 30 s start timeout.
    ///
    /// Every environment variable this process has inherited whose name
    /// starts with `VYASA_` is scrubbed from the child before the
    /// defaults below (and anything passed to [`TestServerBuilder::env`])
    /// are applied — a developer's shell or the repository's own `.env`
    /// commonly exports `VYASA_SECRET_KEY`, `VYASA_AI_ANTHROPIC_KEY`, and
    /// so on, and a test that means to exercise the *absence* of one of
    /// those must not accidentally inherit it. `VYASA_DATABASE_URL`,
    /// `VYASA_LOG__LEVEL`, `VYASA_MEDIA_DIR`, `VYASA_INDEX_DIR` and
    /// `VYASA_REGISTRY_DIR` are then set explicitly, the last three
    /// pointing at subdirectories of a private, per-server temporary
    /// directory (removed when the [`TestServer`] drops) so that
    /// concurrently running servers never share a media store or a
    /// Tantivy index. Pass `.env("VYASA_MEDIA_DIR", ...)` (etc.) to
    /// override any of these.
    #[must_use]
    pub fn builder(bin: impl AsRef<Path>, db: &TestDb) -> TestServerBuilder {
        let work_dir = unique_work_dir();
        TestServerBuilder {
            bin: bin.as_ref().to_path_buf(),
            args: vec!["serve".to_owned()],
            envs: vec![
                ("VYASA_DATABASE_URL".to_owned(), db.url().to_owned()),
                ("VYASA_LOG__LEVEL".to_owned(), "warn".to_owned()),
                ("VYASA_DB__MAX_CONNECTIONS".to_owned(), "4".to_owned()),
                (
                    "VYASA_MEDIA_DIR".to_owned(),
                    work_dir.join("media").display().to_string(),
                ),
                (
                    "VYASA_INDEX_DIR".to_owned(),
                    work_dir.join("index").display().to_string(),
                ),
                (
                    "VYASA_REGISTRY_DIR".to_owned(),
                    work_dir.join("registry").display().to_string(),
                ),
            ],
            timeout: Duration::from_secs(30),
            current_dir: None,
            work_dir,
            first_port: None,
        }
    }

    /// `http://127.0.0.1:<port>`.
    #[must_use]
    pub fn base(&self) -> &str {
        &self.base
    }

    /// The port the server listens on.
    #[must_use]
    pub fn port(&self) -> u16 {
        self.port
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.work_dir);
    }
}

impl TestServerBuilder {
    /// Adds or overrides an environment variable.
    ///
    /// # Panics
    /// If `key` is `VYASA_BIND_ADDR`: the harness owns that port, since it
    /// probes it for readiness; read it back with [`TestServer::port`] or
    /// [`TestServer::base`] instead.
    #[must_use]
    pub fn env(mut self, key: &str, value: impl Into<String>) -> Self {
        assert!(
            key != "VYASA_BIND_ADDR",
            "TestServerBuilder::env(\"VYASA_BIND_ADDR\", _) is not allowed: the harness owns \
             that port, since it probes it for readiness; use TestServer::port() or \
             TestServer::base() instead"
        );
        self.envs.retain(|(k, _)| k != key);
        self.envs.push((key.to_owned(), value.into()));
        self
    }

    /// Replaces the arguments (default `serve`).
    #[must_use]
    pub fn args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.args = args.into_iter().map(Into::into).collect();
        self
    }

    /// Overrides the spawned server's working directory (default: this
    /// process's own). Only needed when the binary reads something
    /// relative to its cwd with no environment override (e.g. the admin
    /// SPA's `admin/dist/index.html`, which has no `VYASA_*` knob).
    #[must_use]
    pub fn current_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.current_dir = Some(dir.into());
        self
    }

    /// How long to wait for the server to accept connections.
    #[must_use]
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Starts the first attempt on `port` instead of a free one, so a test
    /// can hand the harness a port that is already taken.
    #[doc(hidden)]
    #[must_use]
    pub fn first_port(mut self, port: u16) -> Self {
        self.first_port = Some(port);
        self
    }

    /// Spawns the server and waits until it listens.
    ///
    /// The port is held by the harness until then (see [`reserve_port`]),
    /// so no other process picks it for itself while this server boots.
    /// As a backstop, a connect only counts as ready while the child is
    /// still running, and a child that exits reporting "Address already
    /// in use" is restarted once on a fresh port.
    ///
    /// # Panics
    /// With the server's recent stderr if it exits first (other than that
    /// one retry) or does not listen within the timeout.
    #[must_use]
    pub fn start(self) -> TestServer {
        let deadline = Instant::now() + self.timeout;
        let (mut port, mut reservation) = match self.first_port {
            Some(port) => (port, None),
            None => reserve_port(),
        };
        let mut retried = false;
        loop {
            let base = format!("http://127.0.0.1:{port}");
            let outcome = self.attempt(port, deadline);
            drop(reservation.take());
            match outcome {
                Attempt::Ready(child) => {
                    return TestServer {
                        base,
                        port,
                        child,
                        work_dir: self.work_dir,
                    }
                }
                Attempt::Exited(status, stderr)
                    if !retried && stderr.contains("Address already in use") =>
                {
                    retried = true;
                    (port, reservation) = reserve_port();
                    eprintln!(
                        "vyasa-testkit: server exited ({status}): port taken, retrying on {port}"
                    );
                }
                Attempt::Exited(status, stderr) => {
                    let _ = std::fs::remove_dir_all(&self.work_dir);
                    panic!(
                        "server exited ({status}) before listening on {base}; last stderr:\n\
                         {stderr}"
                    );
                }
                Attempt::TimedOut(stderr) => {
                    let _ = std::fs::remove_dir_all(&self.work_dir);
                    panic!(
                        "server did not come up on {base} within {:?}; last stderr:\n{stderr}",
                        self.timeout
                    );
                }
            }
        }
    }

    /// One spawn on `port`, waited on until ready, exited or `deadline`.
    fn attempt(&self, port: u16, deadline: Instant) -> Attempt {
        let mut cmd = Command::new(&self.bin);
        cmd.args(&self.args);
        // Scrub every inherited `VYASA_*` variable before applying this
        // builder's own: see the doc on `TestServer::builder` for why.
        for (key, _) in std::env::vars() {
            if key.starts_with("VYASA_") {
                cmd.env_remove(key);
            }
        }
        cmd.envs(self.envs.iter().map(|(k, v)| (k.as_str(), v.as_str())))
            .env("VYASA_BIND_ADDR", format!("127.0.0.1:{port}"))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        if let Some(dir) = &self.current_dir {
            cmd.current_dir(dir);
        }
        let mut child = cmd
            .spawn()
            .unwrap_or_else(|e| panic!("spawning {}: {e}", self.bin.display()));
        let tail = Arc::new(Mutex::new(VecDeque::with_capacity(STDERR_LINES)));
        if let Some(stderr) = child.stderr.take() {
            let tail = tail.clone();
            std::thread::spawn(move || {
                for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                    let mut tail = tail
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    if tail.len() == STDERR_LINES {
                        tail.pop_front();
                    }
                    tail.push_back(line);
                }
            });
        }
        loop {
            let connected = TcpStream::connect(("127.0.0.1", port)).is_ok();
            if let Ok(Some(status)) = child.try_wait() {
                // Let the reader drain the pipe before reporting.
                std::thread::sleep(Duration::from_millis(100));
                return Attempt::Exited(status, render(&tail));
            }
            if connected {
                return Attempt::Ready(child);
            }
            if Instant::now() > deadline {
                let _ = child.kill();
                let _ = child.wait();
                return Attempt::TimedOut(render(&tail));
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

/// How one spawn of the server ended.
enum Attempt {
    Ready(Child),
    Exited(std::process::ExitStatus, String),
    TimedOut(String),
}

fn render(tail: &Mutex<VecDeque<String>>) -> String {
    let tail = tail
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    tail.iter().cloned().collect::<Vec<_>>().join("\n")
}

/// Picks a free port and keeps it bound, but not listening, with
/// `SO_REUSEADDR` until the returned socket drops.
///
/// Releasing the port before the server binds it (bind port 0, close)
/// leaves a window in which another process can take it: the server then
/// fails to bind while the readiness probe connects to the other
/// process. While the harness holds it, the kernel does not hand the
/// port to anyone else's port-0 bind, yet the server can still bind and
/// listen on it, since both sockets set `SO_REUSEADDR` and only one
/// listens (Tokio and std listeners set it on Unix).
fn reserve_port() -> (u16, Option<Socket>) {
    let socket = Socket::new(Domain::IPV4, Type::STREAM, None).expect("create a socket");
    socket
        .set_reuse_address(true)
        .expect("set SO_REUSEADDR on the port reservation");
    socket
        .bind(&SocketAddr::from(([127, 0, 0, 1], 0)).into())
        .expect("bind an ephemeral port");
    let port = socket
        .local_addr()
        .ok()
        .and_then(|addr| addr.as_socket())
        .map(|addr| addr.port())
        .expect("read the reserved port");
    (port, Some(socket))
}

/// A private directory for one server's media, search index and
/// registry, distinct from every other server's — including one from a
/// concurrently running test in the same or another process.
fn unique_work_dir() -> PathBuf {
    std::env::temp_dir().join(format!(
        "vyasa-testkit-{}-{:016x}",
        std::process::id(),
        rand::random::<u64>()
    ))
}
