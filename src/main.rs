use protocol::download_task::DownloadTask;

mod protocol;

#[tokio::main]
async fn main() {
    DownloadTask::download("./torrents/debian-13.2.0-amd64-netinst.iso.torrent".into()).await;
}
