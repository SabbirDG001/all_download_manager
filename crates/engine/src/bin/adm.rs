//! Command-line front end for the engine, useful for testing it on its own.
//!
//!     adm get https://example.com/big.zip
//!     adm get https://example.com/big.zip -d D:\Downloads -c 16
//!     adm config set download-dir D:\Downloads

use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Instant;

use adm_engine::{download, CancellationToken, DownloadOptions, Error, Progress, Settings};
use anyhow::{bail, Context};
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "adm", version, about = "All Download Manager")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Download a file. Run the same command again to resume it.
    Get {
        url: String,
        /// Folder to save into (default: the download folder from settings).
        #[arg(short, long)]
        dir: Option<PathBuf>,
        /// File name to save as.
        #[arg(short, long)]
        output: Option<String>,
        /// Parallel connections (1-32).
        #[arg(short, long)]
        connections: Option<usize>,
        /// Extra header, e.g. -H "Cookie: a=b". Can be repeated.
        #[arg(short = 'H', long = "header")]
        headers: Vec<String>,
    },
    /// Show or change settings.
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },
}

#[derive(Subcommand)]
enum ConfigAction {
    /// Print the current settings and where they are stored.
    Show,
    /// Change a setting: download-dir, temp-dir, connections, user-agent.
    Set { key: String, value: String },
    /// Clear an optional setting: temp-dir, user-agent.
    Unset { key: String },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let mut settings = Settings::load()?;

    match cli.command {
        Command::Config { action } => config(&mut settings, action),
        Command::Get { url, dir, output, connections, headers } => {
            let headers = headers
                .iter()
                .map(|h| {
                    h.split_once(':')
                        .map(|(k, v)| (k.to_string(), v.to_string()))
                        .with_context(|| format!("header must look like 'Name: value', got '{h}'"))
                })
                .collect::<anyhow::Result<Vec<_>>>()?;
            let opts = DownloadOptions {
                dir: dir.unwrap_or(settings.download_dir.clone()),
                temp_dir: settings.temp_dir.clone(),
                filename: output,
                connections: connections.unwrap_or(settings.connections).clamp(1, 32),
                headers,
                user_agent: settings.user_agent.clone(),
                ..Default::default()
            };
            get(&url, opts).await
        }
    }
}

async fn get(url: &str, opts: DownloadOptions) -> anyhow::Result<()> {
    let cancel = CancellationToken::new();
    let on_ctrl_c = cancel.clone();
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            on_ctrl_c.cancel();
        }
    });

    let started = Instant::now();
    let speed = Mutex::new(SpeedMeter::default());
    let result = download(
        url,
        &opts,
        move |p: Progress| print_progress(p, &mut speed.lock().unwrap()),
        cancel,
    )
    .await;
    println!();

    match result {
        Ok(path) => {
            println!("Saved to {} in {:.1}s", path.display(), started.elapsed().as_secs_f64());
            Ok(())
        }
        Err(Error::Cancelled) => {
            println!("Paused. Run the same command again to resume.");
            Ok(())
        }
        Err(e) => bail!(e),
    }
}

/// Smooths the speed over the last few seconds.
#[derive(Default)]
struct SpeedMeter {
    samples: std::collections::VecDeque<(Instant, u64)>,
}

impl SpeedMeter {
    fn update(&mut self, downloaded: u64) -> f64 {
        let now = Instant::now();
        self.samples.push_back((now, downloaded));
        while self.samples.len() > 2 && now - self.samples[0].0 > std::time::Duration::from_secs(3) {
            self.samples.pop_front();
        }
        let (t0, b0) = self.samples[0];
        let secs = (now - t0).as_secs_f64();
        if secs > 0.0 {
            downloaded.saturating_sub(b0) as f64 / secs
        } else {
            0.0
        }
    }
}

fn print_progress(p: Progress, meter: &mut SpeedMeter) {
    let speed = meter.update(p.downloaded);
    let line = match p.total {
        Some(total) if total > 0 => {
            let pct = p.downloaded as f64 * 100.0 / total as f64;
            let eta = if speed > 0.0 {
                format_duration((total - p.downloaded.min(total)) as f64 / speed)
            } else {
                "--".into()
            };
            format!(
                "{pct:5.1}%  {} / {}  {}/s  ETA {eta}  [{} conn]",
                format_bytes(p.downloaded as f64),
                format_bytes(total as f64),
                format_bytes(speed),
                p.connections
            )
        }
        _ => format!("{}  {}/s", format_bytes(p.downloaded as f64), format_bytes(speed)),
    };
    print!("\r{line:<78}");
    let _ = std::io::stdout().flush();
}

fn format_bytes(b: f64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = b;
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    format!("{v:.1} {}", UNITS[i])
}

fn format_duration(secs: f64) -> String {
    let s = secs.round() as u64;
    if s >= 3600 {
        format!("{}h{:02}m", s / 3600, (s % 3600) / 60)
    } else {
        format!("{}m{:02}s", s / 60, s % 60)
    }
}

fn config(settings: &mut Settings, action: ConfigAction) -> anyhow::Result<()> {
    match action {
        ConfigAction::Show => {
            println!("Settings file: {}", Settings::path().display());
            println!("{}", serde_json::to_string_pretty(settings)?);
            return Ok(());
        }
        ConfigAction::Set { key, value } => match key.as_str() {
            "download-dir" => settings.download_dir = PathBuf::from(value),
            "temp-dir" => settings.temp_dir = Some(PathBuf::from(value)),
            "connections" => settings.connections = value.parse().context("not a number")?,
            "user-agent" => settings.user_agent = Some(value),
            _ => bail!("unknown setting '{key}'"),
        },
        ConfigAction::Unset { key } => match key.as_str() {
            "temp-dir" => settings.temp_dir = None,
            "user-agent" => settings.user_agent = None,
            _ => bail!("'{key}' can't be unset"),
        },
    }
    settings.save()?;
    println!("Saved to {}", Settings::path().display());
    Ok(())
}
