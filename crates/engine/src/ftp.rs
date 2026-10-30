//! Downloads and folder listings over FTP (`ftp://user:password@host/path`).
//! Anonymous login is used when the link has no user name.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use percent_encoding::percent_decode_str;
use reqwest::Url;
use suppaftp::list::{File as FtpFile, ListParser};
use suppaftp::tokio::AsyncFtpStream;
use suppaftp::types::FileType;
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
use tokio_util::sync::CancellationToken;

use crate::download::{part_paths, DownloadOptions, Event, Progress};
use crate::filename::sanitize_filename;
use crate::{Error, Result};

const TIMEOUT: Duration = Duration::from_secs(20);

fn ftp_error(e: suppaftp::FtpError) -> Error {
    Error::Tool(format!("FTP: {e}"))
}

/// The decoded path of an FTP link ("/pub/My Files/a.zip").
pub(crate) fn ftp_path(url: &Url) -> String {
    let path = percent_decode_str(url.path()).decode_utf8_lossy().to_string();
    if path.is_empty() { "/".into() } else { path }
}

/// Connects and logs in.
pub(crate) async fn connect(url: &Url) -> Result<AsyncFtpStream> {
    let host = url.host_str().ok_or_else(|| Error::Settings("the FTP link has no server name".into()))?;
    let port = url.port().unwrap_or(21);
    let addr = format!("{host}:{port}");
    let mut ftp = tokio::time::timeout(TIMEOUT, AsyncFtpStream::connect(addr))
        .await
        .map_err(|_| Error::Tool(format!("FTP: no answer from {host}")))?
        .map_err(ftp_error)?;
    let user = match url.username() {
        "" => "anonymous".to_string(),
        u => percent_decode_str(u).decode_utf8_lossy().to_string(),
    };
    let password = url.password().map(|p| percent_decode_str(p).decode_utf8_lossy().to_string());
    let password = password.unwrap_or_else(|| "anonymous@".into());
    ftp.login(user.as_str(), password.as_str()).await.map_err(ftp_error)?;
    ftp.transfer_type(FileType::Binary).await.map_err(ftp_error)?;
    Ok(ftp)
}

/// Downloads one file over FTP, continuing a partial download if there is one.
pub async fn download_ftp<F>(url: &str, opts: &DownloadOptions, on_event: F, cancel: CancellationToken) -> Result<PathBuf>
where
    F: Fn(Event) + Send + Sync + 'static,
{
    let parsed = Url::parse(url).map_err(|_| Error::Settings(format!("not a valid link: {url}")))?;
    let path = ftp_path(&parsed);
    let remote_name = path.rsplit('/').next().unwrap_or("download").to_string();
    let name = sanitize_filename(opts.filename.as_deref().unwrap_or(&remote_name));

    let mut ftp = connect(&parsed).await?;
    let total = ftp.size(&path).await.ok().map(|s| s as u64);
    on_event(Event::Started { filename: name.clone(), total, resumable: true });

    tokio::fs::create_dir_all(&opts.dir).await?;
    let work_dir = opts.work_dir().to_path_buf();
    tokio::fs::create_dir_all(&work_dir).await?;
    let (part, _) = part_paths(&work_dir, &name);
    let mut offset = tokio::fs::metadata(&part).await.map(|m| m.len()).unwrap_or(0);
    if total.is_some_and(|t| offset > t) {
        offset = 0;
    }
    if offset > 0 {
        ftp.resume_transfer(offset as usize).await.map_err(ftp_error)?;
    }

    let mut raw = tokio::fs::OpenOptions::new().create(true).truncate(false).write(true).open(&part).await?;
    raw.set_len(offset).await?;
    raw.seek(std::io::SeekFrom::Start(offset)).await?;
    let mut file = tokio::io::BufWriter::with_capacity(512 * 1024, raw);

    let on_event = Arc::new(on_event);
    let mut stream = ftp.retr_as_stream(&path).await.map_err(ftp_error)?;
    let mut downloaded = offset;
    let mut buf = vec![0u8; 64 * 1024];
    let mut last_report = std::time::Instant::now();
    loop {
        let n = tokio::select! {
            r = tokio::time::timeout(Duration::from_secs(60), stream.read(&mut buf)) => {
                r.map_err(|_| Error::ConnectionClosed)??
            }
            _ = cancel.cancelled() => {
                file.flush().await?;
                return Err(Error::Cancelled);
            }
        };
        if n == 0 {
            break;
        }
        if let Some(limiter) = &opts.limiter {
            limiter.take(n).await;
        }
        file.write_all(&buf[..n]).await?;
        downloaded += n as u64;
        if last_report.elapsed() >= Duration::from_millis(250) {
            last_report = std::time::Instant::now();
            on_event(Event::Progress(Progress { downloaded, total, connections: 1, parts: Vec::new() }));
        }
    }
    file.flush().await?;
    drop(file);
    stream.finish().await.map_err(ftp_error)?;
    let _ = ftp.quit().await;
    on_event(Event::Progress(Progress { downloaded, total, connections: 0, parts: Vec::new() }));

    if let Some(expected) = total {
        if downloaded != expected {
            return Err(Error::Incomplete { got: downloaded, expected });
        }
    }
    crate::download::finish_file(&part, opts, &name).await
}

/// One entry of an FTP folder.
pub(crate) struct Listed {
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
}

/// Lists a folder, understanding both Unix-style and Windows-style servers.
pub(crate) async fn list_dir(ftp: &mut AsyncFtpStream, path: &str) -> Result<Vec<Listed>> {
    let lines = ftp.list(Some(path)).await.map_err(ftp_error)?;
    Ok(lines
        .iter()
        .filter_map(|line| ListParser::parse_posix(line).or_else(|_| ListParser::parse_dos(line)).ok())
        .filter(|f: &FtpFile| f.name() != "." && f.name() != "..")
        .map(|f| Listed { name: f.name().to_string(), is_dir: f.is_directory(), size: f.size() as u64 })
        .collect())
}
