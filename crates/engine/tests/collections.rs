//! Whole folders and playlists: web folder listings, FTP servers and
//! (with yt-dlp) multi-video pages.

mod common;

use std::path::Path;
use std::time::Duration;

use adm_engine::{find_tools, list_collection, AddRequest, CollectionKind, Entry, Manager, MediaTools, Settings, Status};
use common::{serve_dir, temp_dir, test_body};
use tokio::runtime::Handle;

async fn wait_all_done(m: &Manager) -> Vec<Entry> {
    for _ in 0..600 {
        let es = m.entries();
        if es.iter().all(|e| matches!(e.status, Status::Completed | Status::Failed)) {
            return es;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("timed out: {:#?}", m.entries());
}

/// Adds every item the way the app does: one subfolder per collection, keeping its structure.
fn add_all(m: &Manager, c: &adm_engine::Collection) {
    let reqs = c
        .items
        .iter()
        .map(|i| AddRequest {
            url: i.url.clone(),
            media: i.media,
            title: Some(i.name.clone()),
            subfolder: Some(if i.folder.is_empty() { c.title.clone() } else { format!("{}/{}", c.title, i.folder) }),
            ..Default::default()
        })
        .collect();
    m.add_many(reqs).unwrap();
}

fn write(path: &Path, data: &[u8]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, data).unwrap();
}

fn index(entries: &[(&str, &str)]) -> String {
    // An Apache-style "Index of" page.
    let mut html = String::from("<html><body><h1>Index of</h1><table>\n<tr><td><a href=\"../\">Parent Directory</a></td></tr>\n");
    for (href, size) in entries {
        html.push_str(&format!("<tr><td><a href=\"{href}\">{href}</a></td><td>2024-01-01</td><td align=\"right\">{size}</td></tr>\n"));
    }
    html + "</table></body></html>"
}

#[tokio::test(flavor = "multi_thread")]
async fn downloads_a_whole_web_folder() {
    let site = temp_dir("folder-site");
    let ep1 = test_body(120_000);
    let ep2 = test_body(80_000);
    write(&site.join("Movies/index.html"), index(&[("Season%201/", "-"), ("film.mp4", "1.0K")]).as_bytes());
    write(&site.join("Movies/Season 1/index.html"), index(&[("ep1.mkv", "117K"), ("ep2.mkv", "78K")]).as_bytes());
    write(&site.join("Movies/Season 1/ep1.mkv"), &ep1);
    write(&site.join("Movies/Season 1/ep2.mkv"), &ep2);
    write(&site.join("Movies/film.mp4"), b"film");
    let base = serve_dir(site).await;

    let c = list_collection(&format!("{base}/Movies/"), &[], &MediaTools::default()).await.unwrap();
    assert_eq!(c.kind, CollectionKind::Folder);
    assert_eq!(c.title, "Movies");
    let mut names: Vec<_> = c.items.iter().map(|i| format!("{}|{}", i.folder, i.name)).collect();
    names.sort();
    assert_eq!(names, ["Season 1|ep1.mkv", "Season 1|ep2.mkv", "|film.mp4"]);
    assert_eq!(c.items.iter().find(|i| i.name == "ep1.mkv").unwrap().size, Some(117 * 1024));

    let out = temp_dir("folder-out");
    let m = Manager::new(Settings { download_dir: out.clone(), ..Default::default() }, out.join("list.json"), Handle::current(), |_| {});
    add_all(&m, &c);
    let es = wait_all_done(&m).await;
    assert!(es.iter().all(|e| e.status == Status::Completed), "{es:#?}");
    assert_eq!(std::fs::read(out.join("Movies/Season 1/ep1.mkv")).unwrap(), *ep1);
    assert_eq!(std::fs::read(out.join("Movies/Season 1/ep2.mkv")).unwrap(), *ep2);
    assert_eq!(std::fs::read(out.join("Movies/film.mp4")).unwrap(), b"film");
}

/// Starts a read-only anonymous FTP server on `dir` with Python's pyftpdlib,
/// or returns None when it isn't installed.
async fn ftp_server(dir: &Path) -> Option<(std::process::Child, u16)> {
    let ok = std::process::Command::new("python3").args(["-c", "import pyftpdlib"]).output().ok()?.status.success();
    if !ok {
        return None;
    }
    let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let child = std::process::Command::new("python3")
        .args(["-m", "pyftpdlib", "-i", "127.0.0.1", "-p", &port.to_string(), "-d"])
        .arg(dir)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;
    for _ in 0..100 {
        if tokio::net::TcpStream::connect(("127.0.0.1", port)).await.is_ok() {
            return Some((child, port));
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    None
}

#[tokio::test(flavor = "multi_thread")]
async fn downloads_a_whole_ftp_folder_and_resumes() {
    let root = temp_dir("ftp-root");
    let big = test_body(900_000);
    write(&root.join("pub/Linux/distro.iso"), &big);
    write(&root.join("pub/Linux/docs/readme.txt"), b"hello ftp");
    write(&root.join("pub/My Music/song one.mp3"), &test_body(50_000));
    let Some((mut server, port)) = ftp_server(&root).await else {
        eprintln!("skipping: pyftpdlib is not installed");
        return;
    };

    let c = list_collection(&format!("ftp://127.0.0.1:{port}/pub/"), &[], &MediaTools::default()).await.unwrap();
    assert_eq!(c.title, "pub");
    let mut names: Vec<_> = c.items.iter().map(|i| format!("{}|{}|{:?}", i.folder, i.name, i.size)).collect();
    names.sort();
    assert_eq!(names, ["Linux/docs|readme.txt|Some(9)", "Linux|distro.iso|Some(900000)", "My Music|song one.mp3|Some(50000)"]);

    let out = temp_dir("ftp-out");
    // Half of the ISO is already here from an earlier, interrupted run.
    write(&out.join("pub/Linux/distro.iso.admpart"), &big[..450_000]);
    let m = Manager::new(Settings { download_dir: out.clone(), ..Default::default() }, out.join("list.json"), Handle::current(), |_| {});
    add_all(&m, &c);
    let es = wait_all_done(&m).await;
    let _ = server.kill();
    assert!(es.iter().all(|e| e.status == Status::Completed), "{es:#?}");
    assert_eq!(std::fs::read(out.join("pub/Linux/distro.iso")).unwrap(), *big);
    assert_eq!(std::fs::read(out.join("pub/Linux/docs/readme.txt")).unwrap(), b"hello ftp");
    assert!(out.join("pub/My Music/song one.mp3").exists());
    assert!(!out.join("pub/Linux/distro.iso.admpart").exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn lists_a_page_with_several_videos_as_a_playlist() {
    let tools = find_tools(Path::new("/no-tools-here"));
    if tools.yt_dlp.is_none() {
        eprintln!("skipping: yt-dlp is not on PATH");
        return;
    }
    let site = temp_dir("playlist-site");
    write(&site.join("a.mp4"), &test_body(1000));
    write(&site.join("b.mp4"), &test_body(1000));
    write(&site.join("holiday.html"), b"<!doctype html><title>Holiday Videos</title><video src=\"/a.mp4\"></video><video src=\"/b.mp4\"></video>");
    let base = serve_dir(site).await;

    let c = list_collection(&format!("{base}/holiday.html"), &[], &tools).await.unwrap();
    assert_eq!(c.kind, CollectionKind::Playlist);
    assert_eq!(c.title, "Holiday Videos");
    assert_eq!(c.items.len(), 2);
    assert!(c.items.iter().all(|i| i.media));
}
