//! Connection between the browser extension and the app.
//!
//! The browser starts this same program as a Native Messaging host (it passes
//! `chrome-extension://…` as an argument). In that mode the program reads one
//! message from the browser, forwards it to the running app over a local TCP
//! connection, starting the app first if needed, and writes the answer back.
//!
//! The app listens on 127.0.0.1 on a random port. The port and a random token
//! are written to `bridge.json` in the data folder, so only programs that can
//! read the user's files can talk to it.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use adm_engine::Settings;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

pub const HOST_NAME: &str = "com.alldownloadmanager.host";
/// Fixed by the `key` in `extension/manifest.json`.
pub const EXTENSION_ID: &str = "lkcflfbjjefafnnmmijlmodanfanekff";
const VERSION: &str = env!("CARGO_PKG_VERSION");

/// A message from the extension.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Request {
    pub action: String,
    pub url: String,
    pub filename: String,
    pub referrer: String,
    pub cookies: String,
    pub user_agent: String,
    /// A video/audio page or stream, for yt-dlp.
    pub media: bool,
    /// Page title, shown until the real name is known.
    pub title: String,
    /// Added by the host; checked by the app.
    pub token: String,
}

impl Request {
    /// Request headers that make the download look like it came from the browser.
    pub fn headers(&self) -> Vec<(String, String)> {
        let mut headers = Vec::new();
        for (name, value) in [("Cookie", &self.cookies), ("Referer", &self.referrer), ("User-Agent", &self.user_agent)] {
            if !value.is_empty() {
                headers.push((name.to_string(), value.clone()));
            }
        }
        headers
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct BridgeFile {
    port: u16,
    token: String,
}

fn bridge_file() -> PathBuf {
    Settings::data_dir().join("bridge.json")
}

/// Whether the browser started us as a Native Messaging host.
pub fn is_host_launch() -> bool {
    std::env::args().skip(1).any(|a| a.starts_with("chrome-extension://"))
}

// ---------- Host mode (runs in the process the browser starts) ----------

pub fn run_host() {
    let reply = match read_native_message() {
        Ok(req) => handle_in_host(req),
        Err(e) => json!({ "ok": false, "error": format!("bad message from the browser: {e}") }),
    };
    let _ = write_native_message(&reply);
}

fn handle_in_host(req: Request) -> Value {
    match send_to_app(&req) {
        Ok(reply) => return reply,
        Err(_) if req.action == "ping" => {
            return json!({ "ok": true, "running": false, "version": VERSION });
        }
        Err(_) => {}
    }

    // The app isn't running: start it and wait for it to come up.
    if let Err(e) = launch_app() {
        return json!({ "ok": false, "error": format!("couldn't start the app: {e}") });
    }
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        std::thread::sleep(Duration::from_millis(250));
        match send_to_app(&req) {
            Ok(reply) => return reply,
            Err(e) if Instant::now() > deadline => {
                return json!({ "ok": false, "error": format!("the app didn't answer: {e}") });
            }
            Err(_) => {}
        }
    }
}

fn launch_app() -> std::io::Result<()> {
    let command = || -> std::io::Result<std::process::Command> {
        let mut cmd = std::process::Command::new(std::env::current_exe()?);
        cmd.stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        Ok(cmd)
    };
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // The browser ends the host's job when the host exits; break away from
        // it so the app keeps running.
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
        if command()?.creation_flags(DETACHED_PROCESS | CREATE_BREAKAWAY_FROM_JOB).spawn().is_ok() {
            return Ok(());
        }
    }
    command()?.spawn().map(|_| ())
}

fn send_to_app(req: &Request) -> std::io::Result<Value> {
    let data = std::fs::read(bridge_file())?;
    let info: BridgeFile = serde_json::from_slice(&data).map_err(std::io::Error::other)?;
    let addr = SocketAddr::from(([127, 0, 0, 1], info.port));
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(2))?;
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;

    let mut req = req.clone();
    req.token = info.token;
    let mut line = serde_json::to_vec(&req).map_err(std::io::Error::other)?;
    line.push(b'\n');
    stream.write_all(&line)?;

    let mut answer = String::new();
    BufReader::new(stream).read_line(&mut answer)?;
    serde_json::from_str(&answer).map_err(std::io::Error::other)
}

/// Native Messaging framing: a 4-byte little-endian length, then JSON.
fn read_native_message() -> std::io::Result<Request> {
    let mut stdin = std::io::stdin().lock();
    let mut len = [0u8; 4];
    stdin.read_exact(&mut len)?;
    let len = u32::from_le_bytes(len) as usize;
    if len > 1024 * 1024 {
        return Err(std::io::Error::other("message too large"));
    }
    let mut buf = vec![0u8; len];
    stdin.read_exact(&mut buf)?;
    serde_json::from_slice(&buf).map_err(std::io::Error::other)
}

fn write_native_message(value: &Value) -> std::io::Result<()> {
    let data = serde_json::to_vec(value).map_err(std::io::Error::other)?;
    let mut stdout = std::io::stdout().lock();
    stdout.write_all(&(data.len() as u32).to_le_bytes())?;
    stdout.write_all(&data)?;
    stdout.flush()
}

// ---------- App side ----------

/// Listens for the host on a random local port. `handle` answers each request.
pub async fn serve<F>(handle: F) -> std::io::Result<()>
where
    F: Fn(Request) -> Value + Send + Sync + 'static,
{
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt};

    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await?;
    let token = random_token();
    let info = BridgeFile { port: listener.local_addr()?.port(), token: token.clone() };
    let path = bridge_file();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, serde_json::to_vec(&info).map_err(std::io::Error::other)?)?;

    let handle = std::sync::Arc::new(handle);
    loop {
        let (stream, _) = listener.accept().await?;
        let handle = handle.clone();
        let token = token.clone();
        tauri::async_runtime::spawn(async move {
            let (read, mut write) = stream.into_split();
            let mut line = String::new();
            let mut reader = tokio::io::BufReader::new(read.take(1024 * 1024));
            let reply = match reader.read_line(&mut line).await {
                Ok(_) => match serde_json::from_str::<Request>(&line) {
                    Ok(req) if req.token == token => handle(req),
                    Ok(_) => json!({ "ok": false, "error": "wrong token" }),
                    Err(e) => json!({ "ok": false, "error": e.to_string() }),
                },
                Err(e) => json!({ "ok": false, "error": e.to_string() }),
            };
            let mut out = reply.to_string();
            out.push('\n');
            let _ = write.write_all(out.as_bytes()).await;
        });
    }
}

pub fn ping_reply() -> Value {
    json!({ "ok": true, "running": true, "version": VERSION })
}

fn random_token() -> String {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};
    (0..4)
        .map(|i| {
            let mut h = RandomState::new().build_hasher();
            h.write_u64(i);
            h.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos());
            format!("{:016x}", h.finish())
        })
        .collect()
}

// ---------- Registering with the browsers ----------

/// Tells Chrome, Brave and Edge where the host is. Run on every start, so it
/// stays right after the app is moved or updated.
pub fn register() -> std::io::Result<()> {
    let exe = std::env::current_exe()?;
    let manifest = json!({
        "name": HOST_NAME,
        "description": "All Download Manager",
        "path": exe,
        "type": "stdio",
        "allowed_origins": [format!("chrome-extension://{EXTENSION_ID}/")],
    });
    let manifest = serde_json::to_vec_pretty(&manifest).map_err(std::io::Error::other)?;
    platform::register(&manifest)
}

#[cfg(windows)]
mod platform {
    use super::HOST_NAME;
    use adm_engine::Settings;
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;

    const BROWSERS: &[&str] = &[
        r"Software\Google\Chrome\NativeMessagingHosts",
        r"Software\BraveSoftware\Brave-Browser\NativeMessagingHosts",
        r"Software\Microsoft\Edge\NativeMessagingHosts",
        r"Software\Chromium\NativeMessagingHosts",
    ];

    pub fn register(manifest: &[u8]) -> std::io::Result<()> {
        let dir = Settings::data_dir();
        std::fs::create_dir_all(&dir)?;
        let path = dir.join("native-host.json");
        std::fs::write(&path, manifest)?;
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        for base in BROWSERS {
            let (key, _) = hkcu.create_subkey(format!(r"{base}\{HOST_NAME}"))?;
            key.set_value("", &path.display().to_string())?;
        }
        Ok(())
    }
}

#[cfg(target_os = "linux")]
mod platform {
    use super::HOST_NAME;

    const BROWSERS: &[&str] = &[
        "google-chrome",
        "chromium",
        "BraveSoftware/Brave-Browser",
        "microsoft-edge",
        "google-chrome-for-testing",
    ];

    pub fn register(manifest: &[u8]) -> std::io::Result<()> {
        let Some(config) = dirs::config_dir() else { return Ok(()) };
        for browser in BROWSERS {
            let dir = config.join(browser).join("NativeMessagingHosts");
            std::fs::create_dir_all(&dir)?;
            std::fs::write(dir.join(format!("{HOST_NAME}.json")), manifest)?;
        }
        Ok(())
    }
}

#[cfg(not(any(windows, target_os = "linux")))]
mod platform {
    pub fn register(_manifest: &[u8]) -> std::io::Result<()> {
        Ok(())
    }
}
