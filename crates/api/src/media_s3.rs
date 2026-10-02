//! Media on S3-compatible object storage.
//!
//! Local disk is correct for one server and wrong for several: each would
//! hold different files, so an upload that landed on one instance would
//! 404 from the other. This is the shared-storage half of running more
//! than one node.
//!
//! Written against the S3 REST API directly rather than through an SDK:
//! three verbs on one object each, and the signing is a hundred lines that
//! can be read in one sitting. Works with AWS S3, Cloudflare R2, Backblaze
//! B2 and MinIO.
//!
//! Lives in the API crate for the same reason [`crate::plugin_fetch`]
//! does: `vyasa-core` owns the domain and stays free of network IO, and
//! there is exactly one implementation to audit at the boundary.

use async_trait::async_trait;
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};
use vyasa_common::{AppError, StorageConfig};
use vyasa_core::media::StorageBackend;

/// Longest a single object operation may take.
const TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// The signing algorithm S3 expects.
const ALGORITHM: &str = "AWS4-HMAC-SHA256";

/// `sha256("")`, the payload hash for a request with no body.
const EMPTY_SHA256: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

/// An S3-compatible bucket.
pub struct S3Backend {
    client: reqwest::Client,
    bucket: String,
    region: String,
    endpoint: String,
    access_key_id: String,
    secret_access_key: String,
    path_style: bool,
    /// Key prefix, so one bucket can hold more than one site.
    prefix: String,
}

impl S3Backend {
    /// Builds a backend, or `None` when the configuration is incomplete.
    #[must_use]
    pub fn new(config: &StorageConfig) -> Option<Self> {
        if !config.is_s3() {
            return None;
        }
        Some(Self {
            client: reqwest::Client::builder()
                .timeout(TIMEOUT)
                .build()
                .unwrap_or_default(),
            bucket: config.bucket.clone(),
            region: config.region.clone(),
            endpoint: config.endpoint.trim_end_matches('/').to_owned(),
            access_key_id: config.access_key_id.clone(),
            secret_access_key: config.secret_access_key.as_ref()?.expose().to_owned(),
            path_style: config.path_style,
            prefix: String::from("media/"),
        })
    }

    /// The URL and the canonical URI path for one object key.
    ///
    /// Path-style puts the bucket in the path; virtual-host style puts it
    /// in the hostname. Both are signed over the *path* the request
    /// actually uses, which is why they are computed together.
    fn target(&self, key: &str) -> Result<(String, String, String), AppError> {
        let key = format!("{}{key}", self.prefix);
        let encoded = encode_path(&key);
        let scheme_host = self
            .endpoint
            .split_once("://")
            .ok_or_else(|| AppError::internal_msg("storage endpoint has no scheme"))?;
        let (scheme, host) = scheme_host;
        if self.path_style {
            let path = format!("/{}/{encoded}", self.bucket);
            Ok((format!("{scheme}://{host}{path}"), path, host.to_owned()))
        } else {
            let host = format!("{}.{host}", self.bucket);
            let path = format!("/{encoded}");
            Ok((format!("{scheme}://{host}{path}"), path, host))
        }
    }

    /// Signs and sends one request.
    async fn send(
        &self,
        method: &str,
        key: &str,
        body: Option<&[u8]>,
    ) -> Result<reqwest::Response, AppError> {
        let (url, canonical_path, host) = self.target(key)?;
        let payload_hash = body.map_or_else(
            || EMPTY_SHA256.to_owned(),
            |bytes| hex::encode(Sha256::digest(bytes)),
        );
        let now = chrono::Utc::now();
        let amz_date = now.format("%Y%m%dT%H%M%SZ").to_string();
        let date = now.format("%Y%m%d").to_string();

        // Canonical request. The header set is the minimum S3 requires, and
        // it must be sorted, lowercased and listed in `SignedHeaders`.
        let canonical_headers =
            format!("host:{host}\nx-amz-content-sha256:{payload_hash}\nx-amz-date:{amz_date}\n");
        let signed_headers = "host;x-amz-content-sha256;x-amz-date";
        let canonical_request = format!(
            "{method}\n{canonical_path}\n\n{canonical_headers}\n{signed_headers}\n{payload_hash}"
        );

        let scope = format!("{date}/{}/s3/aws4_request", self.region);
        let string_to_sign = format!(
            "{ALGORITHM}\n{amz_date}\n{scope}\n{}",
            hex::encode(Sha256::digest(canonical_request.as_bytes()))
        );
        let signature = hex::encode(sign(
            &signing_key(&self.secret_access_key, &date, &self.region, "s3"),
            string_to_sign.as_bytes(),
        ));
        let authorization = format!(
            "{ALGORITHM} Credential={}/{scope}, SignedHeaders={signed_headers}, Signature={signature}",
            self.access_key_id
        );

        let mut request = self
            .client
            .request(
                method
                    .parse()
                    .map_err(|_| AppError::internal_msg("bad storage method"))?,
                &url,
            )
            .header("x-amz-date", &amz_date)
            .header("x-amz-content-sha256", &payload_hash)
            .header(reqwest::header::AUTHORIZATION, authorization);
        if let Some(bytes) = body {
            request = request.body(bytes.to_vec());
        }
        request
            .send()
            .await
            .map_err(|e| AppError::internal_msg(format!("storage request failed: {e}")))
    }
}

/// The SigV4 signing key: a chain of HMACs over the scope.
///
/// `service` is always `s3` in production; it is a parameter so the test
/// below can check the derivation against the vector AWS publishes, which
/// is stated for `iam`.
fn signing_key(secret: &str, date: &str, region: &str, service: &str) -> Vec<u8> {
    let mut key = sign(format!("AWS4{secret}").as_bytes(), date.as_bytes());
    key = sign(&key, region.as_bytes());
    key = sign(&key, service.as_bytes());
    sign(&key, b"aws4_request")
}

fn sign(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut mac = <Hmac<Sha256> as hmac::KeyInit>::new_from_slice(key)
        .unwrap_or_else(|_| unreachable!("any key size"));
    mac.update(data);
    mac.finalize().into_bytes().to_vec()
}

/// Percent-encodes an object key for the canonical URI.
///
/// `/` stays a separator; everything outside the unreserved set is
/// encoded, and uppercase hex is required — a lowercase escape produces a
/// different canonical request and therefore a signature the service
/// rejects.
fn encode_path(key: &str) -> String {
    let mut out = String::with_capacity(key.len());
    for byte in key.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => {
                out.push(byte as char);
            }
            other => {
                use std::fmt::Write as _;
                let _ = write!(out, "%{other:02X}");
            }
        }
    }
    out
}

#[async_trait]
impl StorageBackend for S3Backend {
    fn kind(&self) -> vyasa_db::content_models::MediaStorage {
        vyasa_db::content_models::MediaStorage::S3
    }

    async fn put(&self, path: &str, bytes: &[u8]) -> Result<(), AppError> {
        let response = self.send("PUT", path, Some(bytes)).await?;
        if response.status().is_success() {
            Ok(())
        } else {
            Err(storage_error("store", response).await)
        }
    }

    async fn get(&self, path: &str) -> Result<Vec<u8>, AppError> {
        let response = self.send("GET", path, None).await?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(AppError::not_found("media file", path));
        }
        if !response.status().is_success() {
            return Err(storage_error("read", response).await);
        }
        response
            .bytes()
            .await
            .map(|b| b.to_vec())
            .map_err(|e| AppError::internal_msg(format!("could not read media: {e}")))
    }

    async fn delete(&self, path: &str) -> Result<(), AppError> {
        let response = self.send("DELETE", path, None).await?;
        // S3 answers 204 for a key that was never there, which is the
        // behaviour this trait asks for.
        if response.status().is_success() || response.status() == reqwest::StatusCode::NOT_FOUND {
            Ok(())
        } else {
            Err(storage_error("delete", response).await)
        }
    }
}

/// A storage failure with the service's own explanation, which is usually
/// an XML `<Code>` worth seeing in a log.
async fn storage_error(action: &str, response: reqwest::Response) -> AppError {
    let status = response.status().as_u16();
    let body = response.text().await.unwrap_or_default();
    let detail = body
        .split_once("<Code>")
        .and_then(|(_, rest)| rest.split_once("</Code>"))
        .map_or_else(String::new, |(code, _)| format!(" ({code})"));
    AppError::internal_msg(format!("could not {action} media: HTTP {status}{detail}"))
}

#[cfg(test)]
mod tests {
    use super::{encode_path, signing_key, S3Backend, EMPTY_SHA256};
    use sha2::{Digest, Sha256};
    use vyasa_common::{Secret, StorageConfig};

    fn config() -> StorageConfig {
        StorageConfig {
            provider: String::from("s3"),
            bucket: String::from("media-bucket"),
            region: String::from("auto"),
            endpoint: String::from("https://example.r2.cloudflarestorage.com"),
            access_key_id: String::from("AKIAIOSFODNN7EXAMPLE"),
            secret_access_key: Some(Secret::new("wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY")),
            path_style: true,
        }
    }

    #[test]
    fn an_incomplete_configuration_is_not_object_storage() {
        // Half a configuration must not look usable, or an operator would
        // run two nodes believing they share a bucket.
        let mut c = config();
        assert!(c.is_s3());
        for break_it in [
            |c: &mut StorageConfig| c.bucket.clear(),
            |c: &mut StorageConfig| c.endpoint.clear(),
            |c: &mut StorageConfig| c.access_key_id.clear(),
            |c: &mut StorageConfig| c.secret_access_key = None,
            |c: &mut StorageConfig| c.provider = String::from("local"),
        ] {
            let mut broken = config();
            break_it(&mut broken);
            assert!(!broken.is_s3());
            assert!(S3Backend::new(&broken).is_none());
        }
        c.provider = String::from("s3");
        assert!(S3Backend::new(&c).is_some());
    }

    #[test]
    fn path_style_and_virtual_host_sign_over_the_path_they_request() {
        let mut c = config();
        let backend = S3Backend::new(&c).expect("configured");
        let (url, path, host) = backend.target("12/34/photo.jpg").expect("a target");
        assert_eq!(
            url,
            "https://example.r2.cloudflarestorage.com/media-bucket/media/12/34/photo.jpg"
        );
        assert_eq!(path, "/media-bucket/media/12/34/photo.jpg");
        assert_eq!(host, "example.r2.cloudflarestorage.com");

        c.path_style = false;
        let backend = S3Backend::new(&c).expect("configured");
        let (url, path, host) = backend.target("12/34/photo.jpg").expect("a target");
        assert_eq!(
            url,
            "https://media-bucket.example.r2.cloudflarestorage.com/media/12/34/photo.jpg"
        );
        // The signature covers this path, not the path-style one; getting
        // it wrong is a 403 with no useful explanation.
        assert_eq!(path, "/media/12/34/photo.jpg");
        assert_eq!(host, "media-bucket.example.r2.cloudflarestorage.com");
    }

    #[test]
    fn keys_are_encoded_the_way_the_canonical_request_requires() {
        // Separators survive; everything else outside the unreserved set is
        // percent-encoded in *uppercase* hex, or the signature will not
        // match what the service computes.
        assert_eq!(encode_path("12/34/photo.jpg"), "12/34/photo.jpg");
        assert_eq!(encode_path("a b"), "a%20b");
        assert_eq!(encode_path("caf\u{e9}.png"), "caf%C3%A9.png");
        assert_eq!(encode_path("a+b&c"), "a%2Bb%26c");
        assert_eq!(encode_path("~-._"), "~-._");
    }

    #[test]
    fn the_signing_key_is_the_documented_hmac_chain() {
        // AWS's documented example inputs, with the expected value
        // cross-checked against an independent implementation of the same
        // derivation rather than taken on trust. A chain that is wrong
        // produces a signature the service rejects with no explanation
        // worth reading, so it is worth pinning to something outside this
        // file.
        assert_eq!(
            hex::encode(signing_key(
                "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY",
                "20150830",
                "us-east-1",
                "iam",
            )),
            "2c94c0cf5378ada6887f09bb697df8fc0affdb34ba1cdd5bda32b664bd55b73c"
        );
        // The bucket path uses `s3`, so the two must differ.
        assert_ne!(
            signing_key("secret", "20150830", "us-east-1", "s3"),
            signing_key("secret", "20150830", "us-east-1", "iam")
        );
    }

    #[test]
    fn the_empty_payload_hash_is_the_hash_of_nothing() {
        // Used verbatim for GET and DELETE; a wrong constant here would
        // fail every read with a signature mismatch.
        assert_eq!(EMPTY_SHA256, hex::encode(Sha256::digest(b"")));
    }
}

/// Against a real object store, when one is provided.
///
/// `scripts/test-with-db.sh` starts MinIO and exports the variables below,
/// so the same contract the local backend satisfies is checked against a
/// server that actually verifies the signature.
#[cfg(test)]
mod live_tests {
    use super::S3Backend;
    use vyasa_common::{Secret, StorageConfig};
    use vyasa_core::media::StorageBackend as _;

    fn from_env() -> Option<S3Backend> {
        S3Backend::new(&StorageConfig {
            provider: String::from("s3"),
            bucket: std::env::var("VYASA_TEST_S3_BUCKET").ok()?,
            region: String::from("us-east-1"),
            endpoint: std::env::var("VYASA_TEST_S3_ENDPOINT").ok()?,
            access_key_id: std::env::var("VYASA_TEST_S3_KEY").ok()?,
            secret_access_key: Some(Secret::new(std::env::var("VYASA_TEST_S3_SECRET").ok()?)),
            path_style: true,
        })
    }

    #[tokio::test]
    async fn object_storage_honours_the_same_contract_as_local_disk() {
        let Some(backend) = from_env() else {
            eprintln!("skipping: no VYASA_TEST_S3_ENDPOINT");
            return;
        };
        vyasa_core::media::assert_backend_contract(&backend)
            .await
            .expect("object storage honours the backend contract");
    }

    #[tokio::test]
    async fn a_key_needing_escapes_round_trips() {
        // The canonical request signs the *encoded* path, so a key with a
        // space or a non-ASCII character is where a signing bug shows up.
        let Some(backend) = from_env() else {
            eprintln!("skipping: no VYASA_TEST_S3_ENDPOINT");
            return;
        };
        let path = format!("{}/1/a photo cafe\u{301}.png", vyasa_common::next_id_i64());
        backend.put(&path, b"bytes").await.expect("stores");
        assert_eq!(backend.get(&path).await.expect("reads"), b"bytes");
        backend.delete(&path).await.expect("deletes");
    }

    #[tokio::test]
    async fn a_wrong_secret_is_refused_rather_than_silently_accepted() {
        if from_env().is_none() {
            eprintln!("skipping: no VYASA_TEST_S3_ENDPOINT");
            return;
        }
        let bad = S3Backend::new(&StorageConfig {
            provider: String::from("s3"),
            bucket: std::env::var("VYASA_TEST_S3_BUCKET").unwrap_or_default(),
            region: String::from("us-east-1"),
            endpoint: std::env::var("VYASA_TEST_S3_ENDPOINT").unwrap_or_default(),
            access_key_id: std::env::var("VYASA_TEST_S3_KEY").unwrap_or_default(),
            secret_access_key: Some(Secret::new("not-the-secret")),
            path_style: true,
        })
        .expect("configured");
        let err = bad
            .put("1/1/nope.bin", b"x")
            .await
            .expect_err("a bad signature must be refused");
        // Proves the service is checking the signature, which is what
        // makes the passing tests above mean anything.
        assert!(err.to_string().contains("403"), "{err}");
    }
}
