//! The reference community plugin: the BuddyPress test, deliberately tiny —
//! because the CMS should be doing most of the work.
//!
//! * a **topic** is an entry of a custom post type; the forum index is
//!   that type's archive, or any page composing `forum/topic-list`
//! * **replies are core comments** on topic entries: threading, the
//!   moderation queue, the spam-guard hook and email notifications are
//!   all inherited — a community plugin composes with the CMS instead of
//!   rebuilding it, and this file staying small is the proof
//! * a signed-in visitor **starts a topic through a plugin route**, which
//!   is the one place `current-viewer` is honest; the entry is created
//!   through `create-post` under this plugin's own capabilities

wit_bindgen::generate!({
    path: "../../../crates/plugins/wit",
    world: "vyasa-plugin-v2",
});

use vyasa::plugin::host;

struct ForumLite;

fn html(status: u16, body: String) -> host::HttpResponse {
    host::HttpResponse {
        status,
        headers: vec![(
            "content-type".to_owned(),
            "text/html; charset=utf-8".to_owned(),
        )],
        body,
    }
}

fn field(encoded: &str, key: &str) -> Option<String> {
    encoded.split('&').find_map(|pair| {
        let (k, v) = pair.split_once('=')?;
        (k == key).then(|| v.replace('+', " "))
    })
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

impl Guest for ForumLite {
    fn init(_config_json: String) -> Result<PluginInfo, String> {
        Ok(PluginInfo {
            name: "forum-lite".to_owned(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
        })
    }

    fn filter(_stage: FilterStage, content: String) -> String {
        content
    }

    fn action(_kind: EventKind, _payload_json: String) -> Result<(), String> {
        Ok(())
    }

    fn register_blocks() -> String {
        "[]".to_owned()
    }

    fn register_routes() -> String {
        serde_json::json!([
            {"method": "GET", "path": "new"},
            {"method": "POST", "path": "topics"}
        ])
        .to_string()
    }

    fn handle_request(req: host::HttpRequest) -> Result<host::HttpResponse, String> {
        match (req.method.as_str(), req.path.as_str()) {
            ("GET", "new") => {
                // Starting a topic needs an account; reading never does.
                if !matches!(host::current_viewer(), Ok(Some(_))) {
                    return Ok(html(
                        401,
                        "<h1>Sign in to start a topic</h1>\
                         <p>Reading the forum needs no account; writing does.</p>"
                            .to_owned(),
                    ));
                }
                Ok(html(
                    200,
                    "<h1>Start a topic</h1>\
                     <form method=\"post\" action=\"/api/v1/plugin/forum-lite/topics\">\
                     <label>Title <input name=\"title\" required maxlength=\"120\"></label>\
                     <label>Your question <textarea name=\"body\" required></textarea></label>\
                     <button type=\"submit\">Post it</button></form>"
                        .to_owned(),
                ))
            }
            ("POST", "topics") => {
                let Ok(Some(viewer)) = host::current_viewer() else {
                    return Ok(html(401, "<p>Sign in to start a topic.</p>".to_owned()));
                };
                let title = field(&req.body, "title").unwrap_or_default();
                let body = field(&req.body, "body").unwrap_or_default();
                if title.trim().is_empty() || body.trim().is_empty() {
                    return Ok(html(422, "<p>A topic needs a title and a body.</p>".to_owned()));
                }
                // The entry is created under this plugin's own capability
                // grants; replies arrive as ordinary comments on it, and
                // the moderation queue is the site's, not this plugin's.
                let id = host::create_post(&host::NewPost {
                    post_type: "topic".to_owned(),
                    title: format!("{} — asked by {}", title.trim(), viewer.display_name),
                    slug: String::new(),
                    body_html: format!("<p>{}</p>", escape(body.trim())),
                    excerpt: String::new(),
                    status: "published".to_owned(),
                })?;
                let slug = host::get_post(id).map(|p| p.slug).unwrap_or_default();
                Ok(host::HttpResponse {
                    status: 303,
                    headers: vec![("location".to_owned(), format!("/topic/{slug}"))],
                    body: String::new(),
                })
            }
            _ => Ok(host::HttpResponse {
                status: 404,
                headers: vec![],
                body: "no such endpoint".to_owned(),
            }),
        }
    }

    fn render_block(_kind: String, _attrs_json: String) -> Result<String, String> {
        Err("forum-lite declares no editor blocks".to_owned())
    }

    fn run_task(_name: String) -> Result<(), String> {
        Ok(())
    }

    fn register_schedule() -> String {
        "[]".to_owned()
    }

    fn register_admin() -> String {
        serde_json::json!({
            "title": "Forum",
            "description": "Topics are entries of the topic type; replies are the site's own comments, moderated in the ordinary queue.",
            "fields": []
        })
        .to_string()
    }

    fn register_assets() -> String {
        serde_json::json!({
            "css": ".forum-list{list-style:none;padding:0;margin:0;display:grid;gap:.6rem}\
                    .forum-topic{display:flex;align-items:baseline;gap:.8rem;border:1px solid var(--vy-color-border);\
                    border-radius:var(--vy-radius);padding:.8rem 1rem;background:var(--vy-color-surface)}\
                    .forum-topic a{font-weight:600;text-decoration:none;color:var(--vy-color-text)}\
                    .forum-replies{margin-left:auto;color:var(--vy-color-text-muted);font-size:.85em}\
                    .forum-new{display:inline-block;margin-top:1rem;background:var(--vy-color-primary);\
                    color:var(--vy-color-on-primary);padding:.55em 1.1em;border-radius:var(--vy-radius);\
                    text-decoration:none;font-weight:600}"
        })
        .to_string()
    }

    fn register_post_types() -> String {
        serde_json::json!([
            {"slug": "topic", "singular": "Topic", "plural": "Topics",
             "public": true, "has-archive": true}
        ])
        .to_string()
    }

    fn register_taxonomies() -> String {
        "[]".to_owned()
    }

    fn filter_at(point: String, payload: String) -> String {
        match point.as_str() {
            "register-sections" => serde_json::json!([
                {
                    "kind": "forum/topic-list",
                    "title": "Topic list",
                    "category": "community",
                    "description": "The newest topics, with reply counts",
                    "settings-schema": {
                        "type": "object",
                        "properties": {
                            "bind": {"type": "object", "properties": {}},
                            "heading": {"type": "string", "maxLength": 120}
                        },
                        "additionalProperties": false
                    },
                    "sample": {"bind": {"source": "topic", "sort": "newest", "limit": 10},
                               "heading": "Latest topics"},
                    "binds": true,
                    "inline": ["heading"]
                }
            ])
            .to_string(),
            "render-section" => {
                let Ok(req) = serde_json::from_str::<serde_json::Value>(&payload) else {
                    return payload;
                };
                if req.get("kind").and_then(|k| k.as_str()) != Some("forum/topic-list") {
                    return payload;
                }
                let heading = req
                    .get("settings")
                    .and_then(|s| s.get("heading"))
                    .and_then(|h| h.as_str())
                    .unwrap_or("Topics");
                let mut out = format!("<h2>{}</h2><ul class=\"forum-list\">", escape(heading));
                let empty = Vec::new();
                for entry in req.get("bound").and_then(|b| b.as_array()).unwrap_or(&empty) {
                    let title = entry.get("title").and_then(|t| t.as_str()).unwrap_or("");
                    let url = entry.get("url").and_then(|u| u.as_str()).unwrap_or("#");
                    let id = entry
                        .get("id")
                        .and_then(serde_json::Value::as_u64)
                        .unwrap_or(0);
                    // Replies are the site's comments; counting them costs
                    // one read and proves the composition.
                    let replies = host::list_comments(id, 50).map(|c| c.len()).unwrap_or(0);
                    out.push_str(&format!(
                        "<li class=\"forum-topic\"><a href=\"{}\">{}</a>\
                         <span class=\"forum-replies\">{replies} replies</span></li>",
                        escape(url),
                        escape(title)
                    ));
                }
                out.push_str(
                    "</ul><a class=\"forum-new\" href=\"/api/v1/plugin/forum-lite/new\">\
                     Start a topic</a>",
                );
                out
            }
            _ => payload,
        }
    }

    fn on_event(_name: String, _payload_json: String) -> Result<(), String> {
        Ok(())
    }
}

export!(ForumLite);
