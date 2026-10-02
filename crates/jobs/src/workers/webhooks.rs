//! Webhook delivery worker.
//!
//! The webhook service could always sign a payload and the REST surface
//! could always manage subscriptions, but nothing delivered them: there was
//! no job kind and no worker, so a subscription only ever fired from the
//! manual `/test` endpoint.
//!
//! Retries are deliberately not implemented here. The queue already does
//! exponential backoff with jitter and dead-letters after five attempts, so
//! this worker's contract is simply: return `Err` and let the queue decide
//! when to try again.

use sqlx::PgPool;

/// What the dispatcher puts on the queue for one webhook delivery.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct DeliveryJob {
    /// Webhook subscription row id.
    pub webhook_id: i64,
    /// Wire event name, e.g. `post.published`.
    pub event: String,
    /// Destination URL, captured at fan-out time.
    pub url: String,
    /// Pre-computed `t=…,v1=…` signature header value.
    pub signature: String,
    /// Exact JSON body the signature covers — re-serialising would change
    /// key order and invalidate the HMAC.
    pub body: String,
}

/// Per-attempt HTTP timeout. Long enough for a slow receiver, short enough
/// that one dead endpoint cannot occupy a worker.
const TIMEOUT_SECS: u64 = 10;

/// Most of a receiver's answer that is kept for the delivery log.
const MAX_ANSWER_BYTES: usize = 64 * 1024;

/// Attempts one delivery and records the outcome.
///
/// # Errors
/// Returns the failure reason when the request fails or the receiver
/// answers with a non-2xx status, which asks the queue for a retry.
pub async fn handle(pool: &PgPool, payload: serde_json::Value) -> Result<(), String> {
    let job: DeliveryJob =
        serde_json::from_value(payload).map_err(|e| format!("bad webhook payload: {e}"))?;
    let repo = vyasa_db::repo::WebhooksRepo::new(pool.clone());

    // Receivers are user-supplied URLs: the guarded client refuses
    // internal addresses and does not follow redirects.
    let client = crate::net_guard::guarded_builder(0)
        .timeout(std::time::Duration::from_secs(TIMEOUT_SECS))
        .build()
        .map_err(|e| format!("http client: {e}"))?;

    let sent = client
        .post(&job.url)
        .header(vyasa_core::webhooks::SIGNATURE_HEADER, &job.signature)
        .header("content-type", "application/json")
        .header("user-agent", "Vyasa-Webhook/1")
        .body(job.body.clone())
        .send()
        .await;

    // Whether the attempt counts as delivered is decided by the receiver's
    // status, not merely by the request completing: a 500 is a failure.
    let (status, code, err, answer) = match sent {
        Ok(response) => {
            let code = i32::from(response.status().as_u16());
            let ok = response.status().is_success();
            // The first part of the answer is what makes a 400 debuggable.
            let answer = crate::net_guard::read_capped(response, MAX_ANSWER_BYTES)
                .await
                .ok()
                .map(|b| String::from_utf8_lossy(&b).into_owned());
            if ok {
                ("success", Some(code), None, answer)
            } else {
                ("failed", Some(code), Some(format!("HTTP {code}")), answer)
            }
        }
        Err(e) => ("failed", None, Some(e.to_string()), None),
    };

    if let Err(e) = repo
        .record_delivery_detail(
            job.webhook_id,
            &job.event,
            status,
            code,
            Some(&job.body),
            answer.as_deref(),
            err.as_deref(),
        )
        .await
    {
        // A delivery that succeeded must not be retried just because the
        // audit row could not be written.
        tracing::warn!(webhook_id = job.webhook_id, "delivery log failed: {e}");
    }

    match err {
        None => Ok(()),
        Some(reason) => Err(reason),
    }
}

#[cfg(test)]
mod tests {
    use super::DeliveryJob;

    #[test]
    fn payload_roundtrips_without_touching_the_signed_body() {
        // The body travels as an opaque string precisely so re-encoding
        // cannot reorder keys and break the HMAC.
        let body = r#"{"v":1,"event":"post.published","occurred_at":42,"data":{"id":7}}"#;
        let job = DeliveryJob {
            webhook_id: 1,
            event: String::from("post.published"),
            url: String::from("https://example.test/hook"),
            signature: String::from("t=42,v1=abc"),
            body: String::from(body),
        };
        let encoded = serde_json::to_value(&job).expect("encode");
        let decoded: DeliveryJob = serde_json::from_value(encoded).expect("decode");
        assert_eq!(decoded.body, body);
    }

    #[tokio::test]
    async fn malformed_payload_is_reported_not_panicked() {
        // No pool is touched before the payload is parsed, so this is safe
        // to assert without a database.
        let bad = serde_json::json!({ "webhook_id": "not-a-number" });
        let job: Result<DeliveryJob, _> = serde_json::from_value(bad);
        assert!(job.is_err());
    }
}
