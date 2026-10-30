//! End-to-end tests against a tiny local HTTP server.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use adm_engine::{download, CancellationToken, DownloadOptions, Error};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

struct Server {
    url: String,
    /// Body bytes sent so far, over all connections.
    served: Arc<AtomicU64>,
}

/// Serves `body` at `/files/test.bin`. With `ranges` false it ignores `Range`
/// headers. Each 16 KB chunk is followed by `delay`, to make downloads slow
/// enough to interrupt.
async fn serve(body: Arc<Vec<u8>>, ranges: bool, delay: Duration) -> Server {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let served = Arc::new(AtomicU64::new(0));
    let counter = served.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else { return };
            let body = body.clone();
            let counter = counter.clone();
            tokio::spawn(async move {
                let mut req = Vec::new();
                let mut buf = [0u8; 1024];
                while !req.windows(4).any(|w| w == b"\r\n\r\n") {
                    let Ok(n) = sock.read(&mut buf).await else { return };
                    if n == 0 {
                        return;
                    }
                    req.extend_from_slice(&buf[..n]);
                }
                let req = String::from_utf8_lossy(&req).to_lowercase();
                let range = req
                    .lines()
                    .find_map(|l| l.strip_prefix("range: bytes="))
                    .filter(|_| ranges)
                    .map(|r| {
                        let (a, b) = r.trim().split_once('-').unwrap();
                        let a: usize = a.parse().unwrap();
                        let b: usize = if b.is_empty() { body.len() - 1 } else { b.parse().unwrap() };
                        (a, b.min(body.len() - 1))
                    });
                let (head, slice) = match range {
                    Some((a, b)) => (
                        format!(
                            "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes {a}-{b}/{}\r\nContent-Length: {}\r\nAccept-Ranges: bytes\r\nETag: \"v1\"\r\nConnection: close\r\n\r\n",
                            body.len(),
                            b - a + 1
                        ),
                        &body[a..=b],
                    ),
                    None => (
                        format!(
                            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            body.len()
                        ),
                        &body[..],
                    ),
                };
                if sock.write_all(head.as_bytes()).await.is_err() {
                    return;
                }
                for chunk in slice.chunks(16 * 1024) {
                    if sock.write_all(chunk).await.is_err() {
                        return;
                    }
                    counter.fetch_add(chunk.len() as u64, Ordering::Relaxed);
                    if !delay.is_zero() {
                        tokio::time::sleep(delay).await;
                    }
                }
            });
        }
    });
    Server { url: format!("http://{addr}/files/test.bin"), served }
}

fn test_body(len: usize) -> Arc<Vec<u8>> {
    // Not repetitive, so a byte written at the wrong offset is caught.
    let mut x: u32 = 12345;
    Arc::new(
        (0..len)
            .map(|_| {
                x = x.wrapping_mul(1_103_515_245).wrapping_add(12345);
                (x >> 16) as u8
            })
            .collect(),
    )
}

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("adm-test-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

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
