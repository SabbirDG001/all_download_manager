mod common;

use std::path::Path;
use std::time::Duration;

use adm_engine::{AddRequest, Entry, Manager, Settings, Status};
use common::{serve, temp_dir, test_body};
use tokio::runtime::Handle;

fn manager(dir: &Path, max_active: usize) -> Manager {
    let settings = Settings {
        download_dir: dir.to_path_buf(),
        connections: 4,
        max_active,
        ..Default::default()
    };
    Manager::new(settings, dir.join("list.json"), Handle::current(), |_| {})
}

/// Waits until `check` is true for the list, or fails after a few seconds.
async fn wait_for(m: &Manager, what: &str, check: impl Fn(&[Entry]) -> bool) {
    for _ in 0..500 {
        if check(&m.entries()) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("timed out waiting for {what}: {:#?}", m.entries());
}

fn add(m: &Manager, url: &str, name: &str) -> u64 {
    m.add(AddRequest { url: url.into(), filename: Some(name.into()), ..Default::default() })
        .unwrap()
}

fn status(m: &Manager, id: u64) -> Status {
    m.entries().iter().find(|e| e.id == id).unwrap().status
}

#[tokio::test(flavor = "multi_thread")]
async fn runs_one_at_a_time_when_limited() {
    let body = test_body(600_000);
    let server = serve(body.clone(), true, Duration::from_millis(2)).await;
    let dir = temp_dir("mgr-queue");
    let m = manager(&dir, 1);

    let a = add(&m, &server.url, "a.bin");
    let b = add(&m, &server.url, "b.bin");
    assert_eq!(status(&m, a), Status::Downloading);
    assert_eq!(status(&m, b), Status::Queued);

    wait_for(&m, "both done", |es| es.iter().all(|e| e.status == Status::Completed)).await;
    assert_eq!(std::fs::read(dir.join("a.bin")).unwrap(), *body);
    assert_eq!(std::fs::read(dir.join("b.bin")).unwrap(), *body);
}

#[tokio::test(flavor = "multi_thread")]
async fn pause_and_resume() {
    let body = test_body(1_500_000);
    let server = serve(body.clone(), true, Duration::from_millis(3)).await;
    let dir = temp_dir("mgr-pause");
    let m = manager(&dir, 3);

    let id = add(&m, &server.url, "file.bin");
    wait_for(&m, "some progress", |es| es[0].downloaded > 0).await;
    m.pause(&[id]);
    assert_eq!(status(&m, id), Status::Paused);
    // Give the task time to stop, then make sure it stays paused.
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(status(&m, id), Status::Paused);
    assert!(m.entries()[0].downloaded < body.len() as u64);

    m.resume(&[id]);
    wait_for(&m, "done", |es| es[0].status == Status::Completed).await;
    assert_eq!(std::fs::read(dir.join("file.bin")).unwrap(), *body);
    assert_eq!(m.entries()[0].path.as_deref(), Some(dir.join("file.bin").as_path()));
}

#[tokio::test(flavor = "multi_thread")]
async fn remove_running_download_deletes_partial_file() {
    let body = test_body(1_500_000);
    let server = serve(body, true, Duration::from_millis(3)).await;
    let dir = temp_dir("mgr-remove");
    let m = manager(&dir, 3);

    let id = add(&m, &server.url, "gone.bin");
    wait_for(&m, "some progress", |es| es[0].downloaded > 0).await;
    m.remove(&[id], true);
    assert!(m.entries().is_empty());

    // Only the list file should remain once the task has cleaned up.
    for _ in 0..300 {
        let names: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .filter(|n| n.starts_with("gone"))
            .collect();
        if names.is_empty() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("partial files were not deleted");
}

#[tokio::test(flavor = "multi_thread")]
async fn list_survives_restart() {
    let body = test_body(1_500_000);
    let server = serve(body.clone(), true, Duration::from_millis(3)).await;
    let dir = temp_dir("mgr-restart");

    let first = manager(&dir, 3);
    add(&first, &server.url, "keep.bin");
    wait_for(&first, "some progress", |es| es[0].downloaded > 0).await;
    first.pause_all();
    tokio::time::sleep(Duration::from_millis(100)).await;
    drop(first);

    let second = manager(&dir, 3);
    let entries = second.entries();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].status, Status::Paused);
    second.resume_all();
    wait_for(&second, "done", |es| es[0].status == Status::Completed).await;
    assert_eq!(std::fs::read(dir.join("keep.bin")).unwrap(), *body);
}

#[tokio::test(flavor = "multi_thread")]
async fn rejects_bad_links() {
    let dir = temp_dir("mgr-bad");
    let m = manager(&dir, 3);
    assert!(m.add(AddRequest { url: "not a link".into(), ..Default::default() }).is_err());
    assert!(m.add(AddRequest { url: "ftp://x/y".into(), ..Default::default() }).is_err());
    assert!(m.entries().is_empty());
}
