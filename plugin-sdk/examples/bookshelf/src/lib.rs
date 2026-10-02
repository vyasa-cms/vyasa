//! A worked example of the whole `vyasa-plugin-v2` contract.
//!
//! One small plugin that uses every extension point, so that each one has
//! a reference implementation and an end-to-end test:
//!
//! * a custom post type (`book`) with its own public URLs
//! * a custom block (`bookshelf/rating`) it renders itself
//! * a public route (`GET /api/v1/plugin/bookshelf/stats`)
//! * a scheduled task (`tally`) writing to its private storage
//! * an admin settings form, read back at `init`
//! * a stylesheet injected into every page
//! * the `page-html` filter, as in the base world
//! * a custom taxonomy (`shelf`)
//! * named filter points and named events
//! * post meta, the signed-in viewer, and mail

wit_bindgen::generate!({
    path: "../../../crates/plugins/wit",
    world: "vyasa-plugin-v2",
});

use vyasa::plugin::host;

struct Bookshelf;

/// The glyph used for a filled star, from the admin settings form.
///
/// A plugin gets its configuration exactly once, at `init`, so it has to
/// keep it somewhere. A `static mut` would be the obvious thing and is the
/// wrong thing: each call runs in a fresh instance, so it would be reset
/// anyway. Anything that must outlive a call belongs in `kv`.
const STYLE_KEY: &str = "settings/star";
/// Whether to decorate titles, from the same form.
const DECORATE_KEY: &str = "settings/decorate";

/// Whether the operator asked for the title decorations.
///
/// Read per call rather than remembered: every call runs in a fresh
/// instance, so `kv` is the only thing that outlives one. That is a real
/// cost of a configurable filter and worth seeing in an example.
fn decorate() -> bool {
    host::kv_get(DECORATE_KEY)
        .ok()
        .flatten()
        .is_some_and(|v| v == "true")
}

fn setting_star() -> String {
    host::kv_get(STYLE_KEY)
        .ok()
        .flatten()
        .unwrap_or_else(|| "★".to_owned())
}

impl Guest for Bookshelf {
    fn init(config_json: String) -> Result<PluginInfo, String> {
        // The settings the operator filled in on the plugin's admin form
        // arrive here as a JSON object. Persist what we need, because the
        // next call is a different instance.
        let star = serde_json::from_str::<serde_json::Value>(&config_json)
            .ok()
            .and_then(|v| v.get("star").and_then(|s| s.as_str()).map(str::to_owned))
            .unwrap_or_else(|| "★".to_owned());
        host::kv_set(STYLE_KEY, &star)?;
        let decorate = serde_json::from_str::<serde_json::Value>(&config_json)
            .ok()
            .and_then(|v| v.get("decorate").and_then(serde_json::Value::as_bool))
            .unwrap_or(false);
        host::kv_set(DECORATE_KEY, if decorate { "true" } else { "false" })?;
        host::log(
            host::LogLevel::Info,
            &format!("bookshelf ready, star {star}"),
        );
        Ok(PluginInfo {
            name: "bookshelf".to_owned(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
        })
    }

    fn filter(stage: FilterStage, content: String) -> String {
        if !matches!(stage, FilterStage::PageHtml) {
            return content;
        }
        let count = host::kv_get("books/count")
            .ok()
            .flatten()
            .unwrap_or_else(|| "?".to_owned());
        let note = format!("<!-- bookshelf: {count} books -->");
        match content.rfind("</body>") {
            Some(at) => format!("{}{note}{}", &content[..at], &content[at..]),
            None => content,
        }
    }

    fn action(_kind: EventKind, _payload_json: String) -> Result<(), String> {
        Ok(())
    }

    fn register_blocks() -> String {
        r#"[{"kind": "bookshelf/rating", "title": "Book rating", "icon": "★"}]"#.to_owned()
    }

    fn register_routes() -> String {
        r#"[{"method": "GET", "path": "stats"},
            {"method": "GET", "path": "selftest"}]"#
            .to_owned()
    }

    /// Serves `GET /api/v1/plugin/bookshelf/stats`.
    fn handle_request(req: host::HttpRequest) -> Result<host::HttpResponse, String> {
        // A report of what the host lets this plugin do, given the
        // capabilities it declared. Useful when developing a plugin — is
        // this refused because of my manifest or because of my code? — and
        // it is what the host's own test suite asserts against.
        if req.path == "selftest" {
            return Ok(host::HttpResponse {
                status: 200,
                headers: vec![("content-type".to_owned(), "application/json".to_owned())],
                body: selftest(),
            });
        }
        if req.path != "stats" {
            return Ok(host::HttpResponse {
                status: 404,
                headers: vec![],
                body: "no such endpoint".to_owned(),
            });
        }
        let count = host::kv_get("books/count")
            .ok()
            .flatten()
            .unwrap_or_default();
        // A route is rendered per request and cached by nobody, so this is
        // the one place a plugin can honestly personalise.
        let who = match host::current_viewer() {
            Ok(Some(viewer)) => viewer.display_name,
            _ => "anonymous".to_owned(),
        };
        Ok(host::HttpResponse {
            status: 200,
            headers: vec![
                ("content-type".to_owned(), "application/json".to_owned()),
                ("cache-control".to_owned(), "public, max-age=60".to_owned()),
                // Not on the host's allowlist: it is dropped, which is the
                // point of having one.
                ("set-cookie".to_owned(), "sneaky=1".to_owned()),
            ],
            body: format!(
                "{{\"books\":{},\"query\":\"{}\",\"viewer\":\"{}\"}}",
                if count.is_empty() { "0" } else { &count },
                escape(&req.query),
                escape(&who),
            ),
        })
    }

    /// Renders one `bookshelf/rating` block.
    ///
    /// Output is sanitized by the host, so this cannot inject a script no
    /// matter what the stored attributes say — but escaping the parts that
    /// come from a document is still this plugin's job, not the host's.
    fn render_block(kind: String, attrs_json: String) -> Result<String, String> {
        if kind != "bookshelf/rating" {
            return Err(format!("unknown block {kind}"));
        }
        let attrs: serde_json::Value =
            serde_json::from_str(&attrs_json).map_err(|e| e.to_string())?;
        let stars = attrs
            .get("stars")
            .and_then(|v| v.as_u64())
            .unwrap_or(0)
            .min(5);
        let title = attrs.get("title").and_then(|v| v.as_str()).unwrap_or("");
        let star = setting_star();
        Ok(format!(
            "<p class=\"bookshelf-rating\"><span class=\"bookshelf-rating__stars\">{}</span> {}</p>",
            star.repeat(stars as usize),
            escape(title),
        ))
    }

    /// Counts published books into private storage.
    fn run_task(name: String) -> Result<(), String> {
        if name != "tally" {
            return Err(format!("unknown task {name}"));
        }
        let books = host::query_posts(Some("book"), 100)?;
        host::kv_set("books/count", &books.len().to_string())?;
        Ok(())
    }

    fn register_schedule() -> String {
        r#"[{"name": "tally", "every-seconds": 300}]"#.to_owned()
    }

    fn register_admin() -> String {
        r#"{
            "title": "Bookshelf",
            "description": "How book ratings are drawn.",
            "fields": [
                {"key": "star", "label": "Star glyph", "type": "select",
                 "options": ["★", "●", "✦"], "default": "★",
                 "help": "Used by the Book rating block."},
                {"key": "decorate", "label": "Decorate post titles",
                 "type": "boolean", "default": false,
                 "help": "Adds a book badge to titles and the site name to the browser tab. Off by default."}
            ]
        }"#
        .to_owned()
    }

    fn register_assets() -> String {
        serde_json::json!({
            "css": ".bookshelf-rating__stars{color:#c58b00;letter-spacing:.1em}\
                    .bookshelf-rating{margin:.5rem 0}"
        })
        .to_string()
    }

    fn register_post_types() -> String {
        r#"[{"slug": "book", "singular": "Book", "plural": "Books",
             "public": true, "has-archive": true}]"#
            .to_owned()
    }

    fn register_taxonomies() -> String {
        r#"[{"slug": "shelf", "singular": "Shelf", "plural": "Shelves",
             "hierarchical": false, "public": true}]"#
            .to_owned()
    }

    /// The string-keyed filter points.
    ///
    /// A point this plugin does not handle must return `payload`
    /// untouched — that is what lets the host chain every plugin through
    /// every point without any of them knowing about the others.
    fn filter_at(point: String, payload: String) -> String {
        // The designer-section surface rides the string-keyed points, so
        // it is handled before the cosmetic decoration gate: a shelf grid
        // an author placed on a page is content, not decoration.
        match point.as_str() {
            // Declares this plugin's designer sections. The host merges
            // them into the same registry the built-ins live in, which is
            // what puts them in the insert library, the inspector and the
            // assistants' vocabulary with no host special cases.
            "register-sections" => {
                return serde_json::json!([{
                    "kind": "bookshelf/shelf-grid",
                    "title": "Shelf grid",
                    "category": "content",
                    "description": "Books from the shelf, with their spines out",
                    "settings-schema": {
                        "type": "object",
                        "properties": {
                            "bind": {"type": "object", "properties": {}},
                            "heading": {"type": "string", "maxLength": 120}
                        },
                        "additionalProperties": false
                    },
                    "sample": {"bind": {"source": "book", "sort": "newest", "limit": 6}},
                    "binds": true,
                    "inline": ["heading"]
                }])
                .to_string();
            }
            // Renders one instance: the host resolved the binding and
            // handed the entries in — this plugin never queries.
            "render-section" => {
                let Ok(req) = serde_json::from_str::<serde_json::Value>(&payload) else {
                    return payload;
                };
                if req.get("kind").and_then(|k| k.as_str()) != Some("bookshelf/shelf-grid") {
                    return payload;
                }
                let heading = req
                    .get("settings")
                    .and_then(|s| s.get("heading"))
                    .and_then(|h| h.as_str())
                    .unwrap_or("The shelf");
                let mut html = format!("<h2 class=\"bookshelf-heading\">{}</h2>", escape(heading));
                html.push_str("<ul class=\"bookshelf-shelf\">");
                let empty = Vec::new();
                for entry in req
                    .get("bound")
                    .and_then(|b| b.as_array())
                    .unwrap_or(&empty)
                {
                    let title = entry.get("title").and_then(|t| t.as_str()).unwrap_or("");
                    let url = entry.get("url").and_then(|u| u.as_str()).unwrap_or("#");
                    html.push_str(&format!(
                        "<li class=\"bookshelf-spine\"><a href=\"{}\">{}</a></li>",
                        escape(url),
                        escape(title)
                    ));
                }
                html.push_str("</ul>");
                return html;
            }
            _ => {}
        }
        // Decoration is off unless an operator turns it on. A plugin
        // installed to add books has no business restyling every title on
        // someone's site the moment it is enabled — and a demo least of
        // all.
        if !decorate() {
            return payload;
        }
        match point.as_str() {
            // Note what this deliberately does *not* do: personalise by
            // viewer. Page renders are cached and the cached HTML is
            // served to everyone, so `current-viewer` is `none` here by
            // design — `handle_request` is where it is honest, because a
            // route is rendered per request.
            "post-title" => format!("{payload} 📚"),
            // Reaches the page's real `<title>`, not only `og:title` —
            // the two are separate strings and each is filtered once.
            "seo-title" => format!("{payload} · Bookshelf"),
            "search-query" => payload.replace("novel", "book"),
            _ => payload,
        }
    }

    /// The string-keyed events.
    fn on_event(name: String, payload_json: String) -> Result<(), String> {
        match name.as_str() {
            // Stamp every saved post with the time this plugin saw it.
            // Meta travels with the post, unlike `kv`.
            "post-saved" => {
                let id = serde_json::from_str::<serde_json::Value>(&payload_json)
                    .ok()
                    .and_then(|v| v.get("post_id").and_then(|i| i.as_u64()))
                    .ok_or("no post_id")?;
                host::set_post_meta(id, "seen", "yes")
            }
            // Tell the site owner when a new account appears. The plugin
            // never learns the address: `site-admin` resolves host-side.
            "user-created" => host::send_mail("site-admin", "A new account", "Someone signed up."),
            _ => Ok(()),
        }
    }
}

/// One line per host function: `ok`, or the reason it was refused.
fn selftest() -> String {
    // `site_title` is on the host's public-option list; `admin_email` is
    // not, and is refused even though this plugin holds `db:read:options`.
    let public = outcome(host::get_option("site_title").map(|_| ()));
    let private = outcome(host::get_option("admin_email").map(|_| ()));
    let posts = match host::query_posts(None, 5) {
        Ok(list) => format!("ok:{}", list.len()),
        Err(e) => e,
    };
    let one_post = match host::query_posts(None, 1) {
        Ok(list) => match list.first() {
            Some(p) => outcome(host::get_post(p.id).map(|_| ())),
            None => "ok:none".to_owned(),
        },
        Err(e) => e,
    };
    // Every one of these is refused: the manifest never asked for them.
    let event = outcome(host::emit_event(host::EventKind::PostUpdated, "{\"id\":1}"));
    let fetched = outcome(host::fetch("https://example.com/").map(|_| ()));
    let comments = outcome(host::list_comments(1, 5).map(|_| ()));
    let created = outcome(
        host::create_post(&host::NewPost {
            post_type: "post".to_owned(),
            title: "Nope".to_owned(),
            slug: String::new(),
            body_html: "<p>no</p>".to_owned(),
            excerpt: String::new(),
            status: "draft".to_owned(),
        })
        .map(|_| ()),
    );
    let ai = outcome(host::ai_complete("s", "u").map(|_| ()));

    format!(
        "{{\"option_public\":\"{public}\",\"option_private\":\"{private}\",\
          \"query_posts\":\"{posts}\",\"get_post\":\"{one_post}\",\
          \"emit_event\":\"{event}\",\"fetch\":\"{fetched}\",\
          \"list_comments\":\"{comments}\",\"create_post\":\"{created}\",\
          \"ai_complete\":\"{ai}\"}}"
    )
}

fn outcome(result: Result<(), String>) -> String {
    match result {
        Ok(()) => "ok".to_owned(),
        Err(e) => e,
    }
}

/// Escapes text taken from a document before it goes into markup.
fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(c),
        }
    }
    out
}

export!(Bookshelf);
