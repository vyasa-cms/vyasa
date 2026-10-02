//! `indexnow_ping` worker: one GET to the shared IndexNow endpoint.
//!
//! The payload arrives fully formed (url, key, key location) — the
//! dispatcher already decided the URL was worth announcing — so this
//! worker is a courier, not a policy layer. A non-2xx answer fails the
//! job and the queue's retry does the persistence.

/// Where the ping goes; every participating engine syncs from here.
const ENDPOINT: &str = "https://api.indexnow.org/indexnow";

/// The request URL for a payload, or `None` when fields are missing.
fn ping_url(payload: &serde_json::Value) -> Option<String> {
    let field = |k: &str| payload.get(k).and_then(serde_json::Value::as_str);
    let url = field("url")?;
    let key = field("key")?;
    let key_location = field("key_location")?;
    Some(format!(
        "{ENDPOINT}?url={}&key={}&keyLocation={}",
        urlencode(url),
        urlencode(key),
        urlencode(key_location)
    ))
}

fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(char::from(b));
            }
            _ => {
                use std::fmt::Write as _;
                let _ = write!(out, "%{b:02X}");
            }
        }
    }
    out
}

/// Delivers one ping.
///
/// # Errors
/// A malformed payload or a non-success status fails the job.
pub async fn handle(payload: serde_json::Value) -> Result<(), String> {
    let url = ping_url(&payload).ok_or("indexnow payload missing url/key/key_location")?;
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| e.to_string())?;
    let response = client.get(&url).send().await.map_err(|e| e.to_string())?;
    let status = response.status();
    if status.is_success() || status.as_u16() == 202 {
        tracing::info!(
            url = payload.get("url").and_then(serde_json::Value::as_str),
            "indexnow: announced"
        );
        Ok(())
    } else {
        Err(format!("indexnow answered {status}"))
    }
}

#[cfg(test)]
mod tests {
    use super::ping_url;

    #[test]
    fn the_ping_carries_url_key_and_key_location_encoded() {
        let url = ping_url(&serde_json::json!({
            "url": "https://blog.example/post/hello world",
            "key": "abc123",
            "key_location": "https://blog.example/indexnow.txt",
        }))
        .expect("built");
        assert!(url.starts_with("https://api.indexnow.org/indexnow?url="));
        assert!(url.contains("hello%20world"), "{url}");
        assert!(url.contains("&key=abc123&"), "{url}");
        assert!(
            url.contains("keyLocation=https%3A%2F%2Fblog.example%2Findexnow.txt"),
            "{url}"
        );
        assert!(ping_url(&serde_json::json!({"url": "x"})).is_none());
    }
}
