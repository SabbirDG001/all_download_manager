//! Video/audio downloads through the real yt-dlp and ffmpeg. Skipped when
//! they aren't on PATH.

mod common;

use std::path::Path;
use std::process::Command;
use std::time::Duration;

use adm_engine::{find_tools, AddRequest, Entry, Manager, Quality, Settings, Status};
use common::{serve_dir, temp_dir};
use tokio::runtime::Handle;

async fn wait_done(m: &Manager, id: u64) -> Entry {
    for _ in 0..600 {
        let e = m.entries().into_iter().find(|e| e.id == id).unwrap();
        if matches!(e.status, Status::Completed | Status::Failed) {
            return e;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("timed out: {:#?}", m.entries());
}

fn make_media(dir: &Path) {
    let ffmpeg = |args: &[&str]| {
        let ok = Command::new("ffmpeg").args(["-loglevel", "error", "-y"]).args(args).current_dir(dir).status().unwrap();
        assert!(ok.success());
    };
    ffmpeg(&[
        "-f", "lavfi", "-i", "testsrc=size=320x240:rate=25", "-f", "lavfi", "-i", "sine=frequency=440",
        "-t", "6", "-c:v", "libx264", "-preset", "ultrafast", "-c:a", "aac", "-shortest", "clip.mp4",
    ]);
    std::fs::create_dir_all(dir.join("hls")).unwrap();
    ffmpeg(&["-i", "clip.mp4", "-c", "copy", "-f", "hls", "-hls_time", "2", "-hls_playlist_type", "vod", "hls/stream.m3u8"]);
    std::fs::write(
        dir.join("watch.html"),
        "<!doctype html><title>Sunset Timelapse</title><video src=\"/clip.mp4\"></video>",
    )
    .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn downloads_stream_and_extracts_audio() {
    let tools = find_tools(Path::new("/no-tools-here"));
    if tools.yt_dlp.is_none() || tools.ffmpeg.is_none() {
        eprintln!("skipping: yt-dlp and ffmpeg are not on PATH");
        return;
    }
    let src = temp_dir("media-src");
    make_media(&src);
    let base = serve_dir(src).await;
    let out = temp_dir("media-out");

    let settings = Settings { download_dir: out.clone(), ..Default::default() };
    let m = Manager::new(settings, out.join("list.json"), Handle::current(), |_| {});
    m.set_tools_dir(out.join("no-tools"));

    // An HLS stream: yt-dlp fetches the pieces and ffmpeg joins them.
    let id = m
        .add(AddRequest { url: format!("{base}/hls/stream.m3u8"), media: true, ..Default::default() })
        .unwrap();
    let e = wait_done(&m, id).await;
    assert_eq!(e.status, Status::Completed, "{:?}", e.error);
    let path = e.path.unwrap();
    assert!(std::fs::metadata(&path).unwrap().len() > 10_000, "{path:?}");
    assert!(e.title.is_some());

    // A web page with a <video>, saved as MP3.
    let id = m
        .add(AddRequest {
            url: format!("{base}/watch.html"),
            media: true,
            quality: Some(Quality::Audio),
            ..Default::default()
        })
        .unwrap();
    let e = wait_done(&m, id).await;
    assert_eq!(e.status, Status::Completed, "{:?}", e.error);
    let path = e.path.unwrap();
    assert_eq!(path.extension().unwrap(), "mp3");
    assert!(path.exists());
}

/// Installs the real yt-dlp from GitHub. Needs internet, so it only runs with
/// `cargo test -- --ignored`.
#[tokio::test]
#[ignore]
async fn installs_yt_dlp() {
    let dir = temp_dir("tools-install");
    let path = adm_engine::install_yt_dlp(&dir, |_| {}, adm_engine::CancellationToken::new()).await.unwrap();
    let status = adm_engine::tools_status(&dir).await;
    let info = status.yt_dlp.expect("yt-dlp found");
    assert_eq!(info.path, path);
    assert!(info.managed);
    assert!(info.version.unwrap().starts_with("20"));
}
