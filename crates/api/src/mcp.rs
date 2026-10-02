//! `vyasa mcp` — a Model Context Protocol server over stdio.
//!
//! Claude (or any MCP client) manages the site's content through the
//! same REST API everything else uses: this process is a thin courier
//! between newline-delimited JSON-RPC on stdio and `VYASA_URL`,
//! authenticating with `VYASA_API_KEY` — so every capability check,
//! validation rule and audit row applies to an AI exactly as it would
//! to a person with that key. No database access, no back doors.
//!
//! Content arrives as plain text (`content_text`) and becomes block
//! paragraphs/headings/code fences, or as a full block document
//! (`content_blocks`) for callers that speak the schema.

use std::io::{BufRead as _, Write as _};

use serde_json::{json, Value};

/// The protocol revision this server answers with.
const PROTOCOL: &str = "2024-11-05";

/// Runs the loop until stdin closes. Returns non-zero on setup errors.
pub fn run() -> std::process::ExitCode {
    let base = std::env::var("VYASA_URL").unwrap_or_else(|_| "http://127.0.0.1:3000".to_owned());
    let base = base.trim_end_matches('/').to_owned();
    let Ok(key) = std::env::var("VYASA_API_KEY") else {
        eprintln!("vyasa mcp: set VYASA_API_KEY (create one under Settings → API keys)");
        return std::process::ExitCode::FAILURE;
    };
    let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    else {
        eprintln!("vyasa mcp: could not start async runtime");
        return std::process::ExitCode::FAILURE;
    };
    let client = Api { base, key };

    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let Ok(msg) = serde_json::from_str::<Value>(&line) else {
            continue; // not JSON: nothing sane to answer
        };
        let id = msg.get("id").cloned();
        let method = msg.get("method").and_then(Value::as_str).unwrap_or("");
        // Notifications (no id) get no reply.
        let Some(id) = id else { continue };
        let params = msg.get("params").cloned().unwrap_or_else(|| json!({}));
        let reply = match method {
            "initialize" => json!({
                "jsonrpc": "2.0", "id": id,
                "result": {
                    "protocolVersion": PROTOCOL,
                    "capabilities": {"tools": {}},
                    "serverInfo": {"name": "vyasa", "version": env!("CARGO_PKG_VERSION")},
                }
            }),
            "ping" => json!({"jsonrpc": "2.0", "id": id, "result": {}}),
            "tools/list" => json!({
                "jsonrpc": "2.0", "id": id,
                "result": {"tools": tool_defs()}
            }),
            "tools/call" => {
                let outcome = runtime.block_on(call_tool(&client, &params));
                match outcome {
                    Ok(text) => json!({
                        "jsonrpc": "2.0", "id": id,
                        "result": {"content": [{"type": "text", "text": text}]}
                    }),
                    Err(text) => json!({
                        "jsonrpc": "2.0", "id": id,
                        "result": {"content": [{"type": "text", "text": text}], "isError": true}
                    }),
                }
            }
            _ => json!({
                "jsonrpc": "2.0", "id": id,
                "error": {"code": -32601, "message": format!("method not found: {method}")}
            }),
        };
        let mut out = stdout.lock();
        let _ = writeln!(out, "{reply}");
        let _ = out.flush();
    }
    std::process::ExitCode::SUCCESS
}

struct Api {
    base: String,
    key: String,
}

impl Api {
    async fn request(
        &self,
        method: &str,
        path: &str,
        body: Option<Value>,
    ) -> Result<Value, String> {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .map_err(|e| e.to_string())?;
        let url = format!("{}{path}", self.base);
        let mut req = match method {
            "POST" => client.post(&url),
            "PUT" => client.put(&url),
            "DELETE" => client.delete(&url),
            _ => client.get(&url),
        }
        .header("authorization", format!("Bearer {}", self.key));
        if let Some(body) = body {
            req = req.json(&body);
        }
        let response = req.send().await.map_err(|e| e.to_string())?;
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        if status.is_success() {
            if text.is_empty() {
                Ok(json!({"ok": true}))
            } else {
                serde_json::from_str(&text).map_err(|e| e.to_string())
            }
        } else {
            Err(format!("{status}: {text}"))
        }
    }
}

/// Plain text → a block document: `#`-headings, ``` fences, and
/// blank-line paragraphs. Deliberately small — a caller with richer
/// needs sends `content_blocks`.
fn text_to_blocks(text: &str) -> Value {
    let mut blocks: Vec<Value> = Vec::new();
    let mut lines = text.lines().peekable();
    let mut paragraph: Vec<String> = Vec::new();
    let esc = |t: &str| {
        t.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
    };
    let flush = |paragraph: &mut Vec<String>, blocks: &mut Vec<Value>| {
        if !paragraph.is_empty() {
            let text = esc(&paragraph.join(" "));
            blocks.push(json!({"kind": "paragraph", "attrs": {"text": text}, "children": []}));
            paragraph.clear();
        }
    };
    while let Some(line) = lines.next() {
        let trimmed = line.trim_end();
        if let Some(fence) = trimmed.strip_prefix("```") {
            flush(&mut paragraph, &mut blocks);
            let language = fence.trim().to_owned();
            let mut code = Vec::new();
            for code_line in lines.by_ref() {
                if code_line.trim_end().starts_with("```") {
                    break;
                }
                code.push(code_line.to_owned());
            }
            blocks.push(json!({
                "kind": "code",
                "attrs": {"code": code.join("\n"), "language": language},
                "children": []
            }));
        } else if trimmed.starts_with('#') {
            flush(&mut paragraph, &mut blocks);
            let level = trimmed
                .chars()
                .take_while(|c| *c == '#')
                .count()
                .clamp(1, 6);
            let text = esc(trimmed.trim_start_matches('#').trim());
            let level = level.max(2); // h1 is the entry title's
            blocks.push(json!({
                "kind": "heading",
                "attrs": {"level": level, "text": text},
                "children": []
            }));
        } else if trimmed.is_empty() {
            flush(&mut paragraph, &mut blocks);
        } else {
            paragraph.push(trimmed.to_owned());
        }
    }
    flush(&mut paragraph, &mut blocks);
    json!({"schema_version": 1, "blocks": blocks})
}

fn tool_defs() -> Value {
    let post_ref = json!({"type": "string", "description": "Post id"});
    json!([
        {
            "name": "list_posts",
            "description": "List entries, newest first. status: draft|published|scheduled|private|trash.",
            "inputSchema": {"type": "object", "properties": {
                "status": {"type": "string"},
                "page": {"type": "integer", "minimum": 1}
            }}
        },
        {
            "name": "search_posts",
            "description": "Full-text search over published content.",
            "inputSchema": {"type": "object", "properties": {
                "query": {"type": "string"}
            }, "required": ["query"]}
        },
        {
            "name": "get_post",
            "description": "One entry with its full content.",
            "inputSchema": {"type": "object", "properties": {"id": post_ref}, "required": ["id"]}
        },
        {
            "name": "create_post",
            "description": "Create an entry. content_text is plain text (# headings, ``` code \
                            fences, blank-line paragraphs); content_blocks is a full block \
                            document and wins when both are sent. Defaults to a draft.",
            "inputSchema": {"type": "object", "properties": {
                "title": {"type": "string"},
                "content_text": {"type": "string"},
                "content_blocks": {"type": "object"},
                "status": {"type": "string"},
                "type": {"type": "string", "description": "post (default) or page"},
                "excerpt": {"type": "string"}
            }, "required": ["title"]}
        },
        {
            "name": "update_post",
            "description": "Update an entry's title, content, excerpt or status.",
            "inputSchema": {"type": "object", "properties": {
                "id": post_ref,
                "title": {"type": "string"},
                "content_text": {"type": "string"},
                "content_blocks": {"type": "object"},
                "excerpt": {"type": "string"},
                "status": {"type": "string"}
            }, "required": ["id"]}
        },
        {
            "name": "publish_post",
            "description": "Publish an entry now.",
            "inputSchema": {"type": "object", "properties": {"id": post_ref}, "required": ["id"]}
        },
        {
            "name": "trash_post",
            "description": "Move an entry to the trash (restorable).",
            "inputSchema": {"type": "object", "properties": {"id": post_ref}, "required": ["id"]}
        }
    ])
}

async fn call_tool(api: &Api, params: &Value) -> Result<String, String> {
    let name = params.get("name").and_then(Value::as_str).unwrap_or("");
    let args = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let arg = |key: &str| args.get(key).and_then(Value::as_str).map(str::to_owned);
    let id_of = || -> Result<String, String> {
        args.get("id")
            .and_then(|v| {
                v.as_str()
                    .map(str::to_owned)
                    .or_else(|| v.as_i64().map(|n| n.to_string()))
            })
            .ok_or_else(|| "id is required".to_owned())
    };
    let pretty = |v: Value| serde_json::to_string_pretty(&v).unwrap_or_default();

    match name {
        "list_posts" => {
            let status = arg("status").unwrap_or_else(|| "published".to_owned());
            let page = args.get("page").and_then(Value::as_u64).unwrap_or(1);
            let out = api
                .request(
                    "GET",
                    &format!("/api/v1/posts?status={status}&per_page=20&page={page}"),
                    None,
                )
                .await?;
            Ok(pretty(out))
        }
        "search_posts" => {
            let query = arg("query").ok_or("query is required")?;
            let encoded: String = query
                .bytes()
                .flat_map(|b| {
                    if b.is_ascii_alphanumeric() {
                        vec![char::from(b)]
                    } else {
                        format!("%{b:02X}").chars().collect()
                    }
                })
                .collect();
            let out = api
                .request("GET", &format!("/api/v1/search?q={encoded}"), None)
                .await?;
            Ok(pretty(out))
        }
        "get_post" => {
            let out = api
                .request("GET", &format!("/api/v1/posts/{}", id_of()?), None)
                .await?;
            Ok(pretty(out))
        }
        other => call_write_tool(api, other, &args).await,
    }
}

/// The tools that change content, split out so each half stays readable.
async fn call_write_tool(api: &Api, name: &str, args: &Value) -> Result<String, String> {
    let arg = |key: &str| args.get(key).and_then(Value::as_str).map(str::to_owned);
    let id_of = || -> Result<String, String> {
        args.get("id")
            .and_then(|v| {
                v.as_str()
                    .map(str::to_owned)
                    .or_else(|| v.as_i64().map(|n| n.to_string()))
            })
            .ok_or_else(|| "id is required".to_owned())
    };
    let content_of = || -> Option<Value> {
        args.get("content_blocks")
            .filter(|v| v.is_object())
            .cloned()
            .or_else(|| arg("content_text").map(|t| text_to_blocks(&t)))
    };
    let pretty = |v: Value| serde_json::to_string_pretty(&v).unwrap_or_default();
    match name {
        "create_post" => {
            let title = arg("title").ok_or("title is required")?;
            let body = json!({
                "title": title,
                "type": arg("type").unwrap_or_else(|| "post".to_owned()),
                "status": arg("status").unwrap_or_else(|| "draft".to_owned()),
                "content": content_of()
                    .unwrap_or_else(|| json!({"schema_version": 1, "blocks": []})),
                "excerpt": arg("excerpt"),
            });
            let out = api.request("POST", "/api/v1/posts", Some(body)).await?;
            Ok(pretty(out))
        }
        "update_post" => {
            let mut body = serde_json::Map::new();
            if let Some(t) = arg("title") {
                body.insert("title".into(), t.into());
            }
            if let Some(e) = arg("excerpt") {
                body.insert("excerpt".into(), e.into());
            }
            if let Some(s) = arg("status") {
                body.insert("status".into(), s.into());
            }
            if let Some(c) = content_of() {
                body.insert("content".into(), c);
            }
            if body.is_empty() {
                return Err("nothing to update".to_owned());
            }
            let out = api
                .request(
                    "PUT",
                    &format!("/api/v1/posts/{}", id_of()?),
                    Some(Value::Object(body)),
                )
                .await?;
            Ok(pretty(out))
        }
        "publish_post" => {
            let out = api
                .request(
                    "PUT",
                    &format!("/api/v1/posts/{}", id_of()?),
                    Some(json!({"status": "published"})),
                )
                .await?;
            Ok(pretty(out))
        }
        "trash_post" => {
            let out = api
                .request("DELETE", &format!("/api/v1/posts/{}", id_of()?), None)
                .await?;
            Ok(pretty(out))
        }
        other => Err(format!("no tool named {other:?}")),
    }
}

#[cfg(test)]
mod tests {
    use super::{text_to_blocks, tool_defs};

    #[test]
    fn plain_text_becomes_paragraphs_headings_and_code() {
        let doc = text_to_blocks(
            "# Title-ish\n\nFirst paragraph\nstill first.\n\n## Section\n\n```rust\nfn x() {}\n```\n\nLast <b>escaped</b>.",
        );
        let blocks = doc["blocks"].as_array().expect("blocks");
        let kinds: Vec<&str> = blocks
            .iter()
            .map(|b| b["kind"].as_str().unwrap_or(""))
            .collect();
        assert_eq!(
            kinds,
            ["heading", "paragraph", "heading", "code", "paragraph"]
        );
        assert_eq!(
            blocks[0]["attrs"]["level"], 2,
            "h1 is reserved for the title"
        );
        assert_eq!(blocks[1]["attrs"]["text"], "First paragraph still first.");
        assert_eq!(blocks[3]["attrs"]["language"], "rust");
        assert_eq!(blocks[3]["attrs"]["code"], "fn x() {}");
        assert_eq!(
            blocks[4]["attrs"]["text"], "Last &lt;b&gt;escaped&lt;/b&gt;.",
            "pasted markup is text, not markup"
        );
    }

    #[test]
    fn every_tool_declares_a_schema() {
        let defs = tool_defs();
        for tool in defs.as_array().expect("array") {
            assert!(tool["name"].is_string());
            assert!(tool["description"].is_string());
            assert_eq!(tool["inputSchema"]["type"], "object");
        }
    }
}
