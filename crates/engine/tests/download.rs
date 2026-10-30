//! End-to-end tests against a tiny local HTTP server.

mod common;

use std::path::Path;
use std::sync::atomic::Ordering;
use std::time::Duration;

use adm_engine::{download, CancellationToken, DownloadOptions, Error};
use common::{serve, temp_dir, test_body};

fn options(dir: &Path, connections: usize) -> DownloadOptions {
    DownloadOptions {
        dir: dir.to_path_buf(),
        connections,
        min_split_size: 64 * 1024,
        ..Default::default()
    }
}

#[tokio::test]
async fn segmented_download_matches_source() {
    let body = test_body(3 * 1024 * 1024 + 123);
    let server = serve(body.clone(), true, Duration::ZERO).await;
    let dir = temp_dir("segmented");

    let path = download(&server.url, &options(&dir, 8), |_| {}, CancellationToken::new())
        .await
        .unwrap();

    assert_eq!(path.file_name().unwrap(), "test.bin");
    assert_eq!(std::fs::read(&path).unwrap(), *body);
    // No leftovers.
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
}

#[tokio::test]
async fn falls_back_to_one_connection_without_ranges() {
    let body = test_body(500_000);
    let server = serve(body.clone(), false, Duration::ZERO).await;
    let dir = temp_dir("noranges");

    let path = download(&server.url, &options(&dir, 8), |_| {}, CancellationToken::new())
        .await
        .unwrap();

    assert_eq!(std::fs::read(&path).unwrap(), *body);
}

#[tokio::test]
async fn existing_file_is_not_overwritten() {
    let body = test_body(10_000);
    let server = serve(body.clone(), true, Duration::ZERO).await;
    let dir = temp_dir("unique");
    std::fs::write(dir.join("test.bin"), b"keep me").unwrap();

    let path = download(&server.url, &options(&dir, 2), |_| {}, CancellationToken::new())
        .await
        .unwrap();

    assert_eq!(path.file_name().unwrap(), "test (1).bin");
    assert_eq!(std::fs::read(dir.join("test.bin")).unwrap(), b"keep me");
}

#[tokio::test]
async fn resumes_after_cancel() {
    let size = 2 * 1024 * 1024;
    let body = test_body(size);
    let server = serve(body.clone(), true, Duration::from_millis(2)).await;
    let dir = temp_dir("resume");

    // First run: stop at about 40%.
    let cancel = CancellationToken::new();
    let stopper = cancel.clone();
    let result = download(
        &server.url,
        &options(&dir, 4),
        move |p| {
            if p.downloaded > size as u64 * 2 / 5 {
                stopper.cancel();
            }
        },
        cancel,
    )
    .await;
    assert!(matches!(result, Err(Error::Cancelled)), "{result:?}");
    assert!(dir.join("test.bin.admpart.json").exists());
    let served_first = server.served.load(Ordering::Relaxed);

    // Second run picks up where the first stopped.
    let path = download(&server.url, &options(&dir, 4), |_| {}, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), *body);

    let served_total = server.served.load(Ordering::Relaxed);
    let served_second = served_total - served_first;
    assert!(
        served_second < size as u64 * 3 / 4,
        "second run re-downloaded too much: {served_second} of {size}"
    );
}
