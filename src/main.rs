use protocol::download_task::DownloadTask;

mod protocol;
mod tui;

#[tokio::main]
async fn main() {
    color_eyre::install().unwrap();
    let (progress_tx, progress_rx) = std::sync::mpsc::channel::<tui::ProgressEvent>();

    tokio::spawn(async move {
        DownloadTask::new(progress_tx)
            .download("./torrents/debian-13.2.0-amd64-netinst.iso.torrent".into())
            .await;
    });

    let mut terminal = ratatui::init_with_options(ratatui::TerminalOptions {
        viewport: ratatui::Viewport::Inline(8),
    });
    let _ = tui::app::App::new(progress_rx).run(&mut terminal);
    ratatui::restore();
}
