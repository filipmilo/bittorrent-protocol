use std::time::Instant;

use protocol::download_task::DownloadTask;

mod protocol;
mod tui;

const DEFAULT_TORRENT: &str = "./torrents/debian-13.2.0-amd64-netinst.iso.torrent";

#[tokio::main]
async fn main() {
    let torrent = std::env::args()
        .nth(1)
        .unwrap_or_else(|| DEFAULT_TORRENT.to_string());

    let file_appender = tracing_appender::rolling::never(".", "bittorrent.log");
    let (non_blocking, _guard) = tracing_appender::non_blocking(file_appender);
    tracing_subscriber::fmt().with_writer(non_blocking).init();

    color_eyre::install().unwrap();
    let (progress_tx, progress_rx) = std::sync::mpsc::channel::<tui::ProgressEvent>();

    tokio::spawn(async move {
        DownloadTask::new(progress_tx).download(torrent).await;
    });

    let mut terminal = ratatui::init();
    let outcome = tui::app::App::new(progress_rx, Instant::now()).run(&mut terminal);
    ratatui::restore();

    match outcome {
        Ok(summary) => summary.lines().iter().for_each(|line| println!("{line}")),
        Err(error) => eprintln!("{error}"),
    }
}
