use crate::tui::app::App;

use super::{
    bencode::Bencode,
    connection_manager::ConnectionManager,
    file_serializer::FileSerializer,
    torrent_file::{Info, TorrentFile},
    tracker::{Peer, TrackerRequest, TrackerResponse},
    utils::sha1,
};
use nanoid::nanoid;

pub struct DownloadTask {
    progress_tx: std::sync::mpsc::Sender<crate::tui::ProgressEvent>,
}

impl DownloadTask {
    pub fn new(progress_tx: std::sync::mpsc::Sender<crate::tui::ProgressEvent>) -> Self {
        Self { progress_tx }
    }

    pub async fn download(&self, file_path: String) {
        let file = std::fs::read(file_path)
            .expect("Can't open torrent file.")
            .iter()
            .map(|it| it.clone())
            .collect::<Vec<u8>>();

        let torrent = Self::parse_file(file);

        if let Ok(torr) = torrent {
            println!(
                "Piece Length for this torrent :{:?}",
                torr.info.piece_length
            );

            let serializer = Self::open_file_serializer(&torr.info);

            let pieces = torr
                .info
                .pieces
                .chunks(20)
                .map(|chunk| {
                    chunk
                        .iter()
                        .map(|&byte| format!("%{:02x}", byte))
                        .collect::<String>()
                })
                .collect::<Vec<String>>();

            let _ = self.progress_tx.send(crate::tui::ProgressEvent::Started {
                total_pieces: pieces.len(),
            });

            let (raw_info_hash, info_hash) = sha1(&torr.info_raw);

            let peer_id = format!("-RS0001-{}", nanoid!(12));

            let tracker_request = TrackerRequest::from(
                torr.announce,
                info_hash,
                peer_id.clone(),
                6881,
                torr.info.length.unwrap(),
            );

            let response = tracker_request.fetch_peer_info().await;

            if let Ok(resp) = response {
                match resp {
                    TrackerResponse::Success(peer_info) => {
                        let ip_v4_peers: Vec<Peer> = peer_info
                            .peers
                            .into_iter()
                            .filter(|peer| !peer.ip.contains(":"))
                            .collect();

                        ConnectionManager::new(
                            torr.info.piece_length,
                            &ip_v4_peers,
                            raw_info_hash,
                            peer_id,
                            pieces,
                            peer_info.interval,
                            serializer.unwrap(),
                            self.progress_tx.clone(),
                        )
                        .await
                        .download()
                        .await;
                    }

                    TrackerResponse::Failure(err) => {
                        println!("{:?}", err);
                    }
                }
            }
        }
    }

    fn parse_file(file: Vec<u8>) -> Result<TorrentFile, String> {
        let decoded_dictionary = Bencode::decode_dict(file);

        TorrentFile::try_from(decoded_dictionary)
    }

    fn open_file_serializer(info: &Info) -> Result<FileSerializer, std::io::Error> {
        match info.length {
            Some(length) => FileSerializer::new(&info.name, info.piece_length, length),
            None => panic!("Multi file donwloads not supported."),
        }
    }
}
