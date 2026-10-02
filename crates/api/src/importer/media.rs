//! Media fetching for the importer: bounded-concurrency downloads from
//! the source uploads URL, stored through [`MediaService`].
//!
//! The jobs queue (phase 16) remains the runtime path for user-triggered
//! fetches; the CLI runs these inline so an import is self-contained
//! without a running server worker pool (deviation recorded in
//! PHASE-29.md).

use vyasa_common::AppError;

/// Downloads `url` and stores it as media owned by `owner_id`.
///
/// # Errors
/// Returns [`AppError::Validation`] for non-https URLs and network/IO
/// failures surface as [`AppError::Internal`].
#[allow(dead_code)] // single-item helper used by tests
pub async fn download(
    service: &vyasa_core::media::MediaService,
    owner_id: i64,
    file_name: &str,
    url: &str,
) -> Result<i64, AppError> {
    if !url.starts_with("https://") {
        return Err(AppError::validation(format!(
            "media url must be https: {url}"
        )));
    }
    let bytes = fetch(url).await?;
    let row = service
        .upload(owner_id, file_name, bytes, None, None)
        .await?;
    Ok(row.id)
}

async fn fetch(url: &str) -> Result<Vec<u8>, AppError> {
    // Blocking IO inside spawn_blocking keeps the async surface honest.
    let url = url.to_owned();
    tokio::task::spawn_blocking(move || {
        let resp = ureq::get(&url)
            .timeout(std::time::Duration::from_secs(60))
            .call();
        match resp {
            Ok(r) => {
                let mut buf = Vec::new();
                use std::io::Read as _;
                let mut reader = r.into_reader();
                if reader.read_to_end(&mut buf).is_err() {
                    return Err(AppError::internal_msg(format!("media read {url}")));
                }
                Ok(buf)
            }
            Err(e) => Err(AppError::internal_msg(format!("media fetch {url}: {e}"))),
        }
    })
    .await
    .map_err(|e| AppError::internal_msg(format!("media task: {e}")))?
}

/// Fetches many urls with bounded concurrency (fetches run in parallel;
/// stores serialize through the service).
pub async fn download_many(
    service: &vyasa_core::media::MediaService,
    owner_id: i64,
    items: Vec<(String, String)>,
    concurrency: usize,
) -> Vec<(String, Result<i64, String>)> {
    let sem = std::sync::Arc::new(tokio::sync::Semaphore::new(concurrency.max(1)));
    let mut fetched = Vec::with_capacity(items.len());
    for (name, url) in &items {
        let permit = std::sync::Arc::clone(&sem);
        let url = url.clone();
        let name = name.clone();
        fetched.push(tokio::spawn(async move {
            let _permit = permit.acquire_owned().await;
            let bytes = fetch(&url).await;
            (name, url, bytes)
        }));
    }
    let mut out = Vec::with_capacity(fetched.len());
    for f in fetched {
        match f.await {
            Ok((name, _url, Ok(bytes))) => {
                match service.upload(owner_id, &name, bytes, None, None).await {
                    Ok(row) => out.push((name, Ok(row.id))),
                    Err(e) => out.push((name, Err(e.to_string()))),
                }
            }
            Ok((name, url, Err(e))) => {
                let _ = url;
                out.push((name, Err(e.to_string())))
            }
            Err(e) => out.push((String::new(), Err(format!("join: {e}")))),
        }
    }
    out
}
