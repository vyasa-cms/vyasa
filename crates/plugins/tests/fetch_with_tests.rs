//! `fetch-with`: the grant, the quotas, the status as data.
#![allow(
    clippy::pedantic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::type_complexity
)]

use vyasa_plugins::broker::{Broker, Quotas};
use vyasa_plugins::host::vyasa::plugin::host::{FetchRequest, Host as _};
use vyasa_plugins::host::{AiWindow, FetchBackend, FetchReply, HostCtx};
use vyasa_testkit::TestDb;

struct Fake {
    reply: FetchReply,
    seen: std::sync::Mutex<Vec<(String, Vec<(String, String)>)>>,
}

impl FetchBackend for Fake {
    fn get<'a>(
        &'a self,
        _url: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + 'a>>
    {
        Box::pin(async { Err(String::from("unused")) })
    }
    fn get_with<'a>(
        &'a self,
        url: &'a str,
        headers: Vec<(String, String)>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<FetchReply, String>> + Send + 'a>>
    {
        self.seen.lock().unwrap().push((url.to_owned(), headers));
        let reply = FetchReply {
            status: self.reply.status,
            headers: self.reply.headers.clone(),
            body: self.reply.body.clone(),
        };
        Box::pin(async move { Ok(reply) })
    }
}

async fn ctx_with(
    grants: serde_json::Value,
    quotas: Quotas,
    status: u16,
    body: &str,
) -> (HostCtx, std::sync::Arc<Fake>, TestDb) {
    let db = TestDb::new().await;
    let pool = db.pool().clone();
    let repo = vyasa_db::repo::PluginsRepo::new(pool.clone());
    let row = repo
        .create("fetcher", "0.1.0", b"{}", "aa", &grants)
        .await
        .unwrap();
    let fake = std::sync::Arc::new(Fake {
        reply: FetchReply {
            status,
            headers: vec![("etag".into(), "\"e\"".into())],
            body: body.into(),
        },
        seen: std::sync::Mutex::new(vec![]),
    });
    let ctx = HostCtx {
        plugin_name: "fetcher".into(),
        plugin_id: row.id,
        limits: None,
        wasi: wasmtime_wasi::WasiCtxBuilder::new().build(),
        table: wasmtime_wasi::ResourceTable::new(),
        broker: std::sync::Arc::new(Broker::with_quotas(repo, pool.clone(), quotas)),
        dest: pool,
        ai: None,
        fetch: Some(fake.clone()),
        viewer: None,
        ai_window: AiWindow::Unbounded,
        call_deadline: None,
    };
    (ctx, fake, db)
}

fn req(url: &str) -> FetchRequest {
    FetchRequest {
        url: url.into(),
        headers: vec![("accept".into(), "text/plain".into())],
    }
}

#[tokio::test]
async fn a_granted_host_answers_with_status_headers_and_body_for_any_status() {
    for status in [200u16, 304, 404, 403] {
        let (mut c, fake, _db) = ctx_with(
            serde_json::json!(["net:fetch:api.github.com"]),
            Quotas::default(),
            status,
            "text",
        )
        .await;
        let r = c
            .fetch_with(req("https://api.github.com/repos/o/r"))
            .await
            .unwrap();
        assert_eq!((r.status, r.body.as_str()), (status, "text"));
        assert_eq!(r.headers, vec![("etag".to_owned(), "\"e\"".to_owned())]);
        assert_eq!(fake.seen.lock().unwrap().len(), 1);
    }
}

#[tokio::test]
async fn an_ungranted_host_or_scheme_is_denied_and_nothing_is_sent() {
    let (mut c, fake, _db) = ctx_with(
        serde_json::json!(["net:fetch:api.github.com"]),
        Quotas::default(),
        200,
        "x",
    )
    .await;
    for url in [
        "https://evil.example/x",
        "http://api.github.com/x",
        "https://api.github.com.evil.example/x",
    ] {
        let err = c.fetch_with(req(url)).await.unwrap_err();
        assert!(
            err == "capability-denied" || err.contains("https"),
            "{url}: {err}"
        );
    }
    assert!(fake.seen.lock().unwrap().is_empty());
}

#[tokio::test]
async fn the_shared_fetch_and_byte_quotas_apply() {
    let quotas = Quotas {
        fetches_per_min: 2,
        ..Quotas::default()
    };
    let (mut c, _fake, _db) = ctx_with(
        serde_json::json!(["net:fetch:api.github.com"]),
        quotas,
        200,
        "x",
    )
    .await;
    c.fetch_with(req("https://api.github.com/a")).await.unwrap();
    c.fetch_with(req("https://api.github.com/b")).await.unwrap();
    assert_eq!(
        c.fetch_with(req("https://api.github.com/c"))
            .await
            .unwrap_err(),
        "capability-denied"
    );
    let quotas = Quotas {
        bytes_per_min: 10,
        ..Quotas::default()
    };
    let (mut c, _f, _db2) = ctx_with(
        serde_json::json!(["net:fetch:api.github.com"]),
        quotas,
        200,
        "0123456789ABCDEF",
    )
    .await;
    assert_eq!(
        c.fetch_with(req("https://api.github.com/a"))
            .await
            .unwrap_err(),
        "byte quota exceeded"
    );
}

#[tokio::test]
async fn redirects_are_refused_so_authorization_cannot_follow() {
    // The production backend builds its client with redirects off (asserted by
    // the existing `the_client_builds_with_redirects_refused`); a 3xx reaches the
    // plugin as data, never as a second request.
    let (mut c, fake, _db) = ctx_with(
        serde_json::json!(["net:fetch:api.github.com"]),
        Quotas::default(),
        302,
        "",
    )
    .await;
    let r = c.fetch_with(req("https://api.github.com/a")).await.unwrap();
    assert_eq!(r.status, 302);
    assert_eq!(
        fake.seen.lock().unwrap().len(),
        1,
        "one request, no follow-up"
    );
}
