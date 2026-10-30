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
    assert!(m.add(AddRequest { url: "file:///etc/passwd".into(), ..Default::default() }).is_err());
    assert!(m.entries().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn speed_limit_slows_downloads_and_can_change_live() {
    let body = test_body(600_000);
    let server = serve(body.clone(), true, Duration::ZERO).await;
    let dir = temp_dir("mgr-limit");
    let settings = Settings {
        download_dir: dir.clone(),
        speed_limit_kbps: 200,
        speed_limit_on: true,
        ..Default::default()
    };
    let m = Manager::new(settings.clone(), dir.join("list.json"), Handle::current(), |_| {});

    let started = std::time::Instant::now();
    add(&m, &server.url, "slow.bin");
    // 600 KB at 200 KB/s takes about 3 s; check it isn't done after 1.5 s.
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let e = &m.entries()[0];
    assert_eq!(e.status, Status::Downloading, "finished too fast for the limit");
    assert!(e.downloaded < 500_000, "{}", e.downloaded);

    // Turning the limit off lets the running download finish right away.
    m.set_settings(Settings { speed_limit_on: false, ..settings });
    wait_for(&m, "done", |es| es[0].status == Status::Completed).await;
    assert!(started.elapsed() < Duration::from_millis(2900));
    assert_eq!(std::fs::read(dir.join("slow.bin")).unwrap(), *body);
}

#[tokio::test(flavor = "multi_thread")]
async fn sorts_into_folders_by_type() {
    let body = test_body(10_000);
    let server = serve(body, true, Duration::ZERO).await;
    let dir = temp_dir("mgr-sort");
    let settings = Settings { download_dir: dir.clone(), sort_into_folders: true, ..Default::default() };
    let m = Manager::new(settings, dir.join("list.json"), Handle::current(), |_| {});

    add(&m, &server.url, "movie.mp4");
    add(&m, &server.url, "notes.unknown");
    // A folder the user picked is used as is.
    let picked = dir.join("picked");
    m.add(AddRequest { url: server.url.clone(), filename: Some("x.zip".into()), dir: Some(picked.clone()), ..Default::default() })
        .unwrap();
    wait_for(&m, "all done", |es| es.iter().all(|e| e.status == Status::Completed)).await;
    assert!(dir.join("Video/movie.mp4").exists());
    assert!(dir.join("notes.unknown").exists());
    assert!(picked.join("x.zip").exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn scheduler_runs_its_downloads_only_when_active() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    let body = test_body(300_000);
    let server = serve(body.clone(), true, Duration::from_millis(2)).await;
    let dir = temp_dir("mgr-schedule");
    let done_events = Arc::new(AtomicUsize::new(0));
    let counter = done_events.clone();
    let settings = Settings { download_dir: dir.clone(), ..Default::default() };
    let m = Manager::new(settings, dir.join("list.json"), Handle::current(), move |c| {
        if matches!(c, adm_engine::Change::ScheduleDone) {
            counter.fetch_add(1, Ordering::SeqCst);
        }
    });

    let req = |name: &str| AddRequest {
        url: server.url.clone(),
        filename: Some(name.into()),
        scheduled: true,
        ..Default::default()
    };
    let a = m.add(req("night1.bin")).unwrap();
    let b = m.add(req("night2.bin")).unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(status(&m, a), Status::Scheduled);
    assert_eq!(status(&m, b), Status::Scheduled);

    // The window opens, then closes again before they finish.
    m.set_schedule_active(true);
    wait_for(&m, "started", |es| es.iter().all(|e| e.status == Status::Downloading)).await;
    m.set_schedule_active(false);
    assert_eq!(status(&m, a), Status::Scheduled);
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_eq!(status(&m, a), Status::Scheduled, "stopped downloads go back to the scheduler");
    assert_eq!(done_events.load(Ordering::SeqCst), 0);

    // Next window: they resume and finish, and "when done" fires once.
    m.set_schedule_active(true);
    wait_for(&m, "finished", |es| es.iter().all(|e| e.status == Status::Completed)).await;
    for _ in 0..100 {
        if done_events.load(Ordering::SeqCst) > 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(done_events.load(Ordering::SeqCst), 1);
    assert!(!m.schedule_active());
    assert_eq!(std::fs::read(dir.join("night2.bin")).unwrap(), *body);

    // An active window with nothing to do doesn't trigger "when done".
    m.set_schedule_active(true);
    assert!(!m.schedule_active());
    assert_eq!(done_events.load(Ordering::SeqCst), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn reports_each_finished_download_once() {
    use std::sync::{Arc, Mutex};

    let body = test_body(200_000);
    let server = serve(body, true, Duration::from_millis(1)).await;
    let dir = temp_dir("mgr-finished");
    let finished = Arc::new(Mutex::new(Vec::new()));
    let seen = finished.clone();
    let settings = Settings { download_dir: dir.clone(), ..Default::default() };
    let m = Manager::new(settings, dir.join("list.json"), Handle::current(), move |c| {
        if let adm_engine::Change::Finished(e) = c {
            seen.lock().unwrap().push((e.id, e.status));
        }
    });

    let ok = add(&m, &server.url, "ok.bin");
    let paused = add(&m, &server.url, "paused.bin");
    m.pause(&[paused]);
    // Nothing listens on port 1: the download fails.
    let bad = m.add(AddRequest { url: "http://127.0.0.1:1/missing.bin".into(), ..Default::default() }).unwrap();
    wait_for(&m, "done", |es| {
        es.iter().find(|e| e.id == ok).unwrap().status == Status::Completed
            && es.iter().find(|e| e.id == bad).unwrap().status == Status::Failed
    })
    .await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    let mut got = finished.lock().unwrap().clone();
    got.sort_by_key(|(id, _)| *id);
    // The paused one is not "finished".
    assert_eq!(got, [(ok, Status::Completed), (bad, Status::Failed)]);
    assert_eq!(m.entry(ok).unwrap().status, Status::Completed);
}
