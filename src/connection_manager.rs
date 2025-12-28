use std::sync::Arc;

use futures::future::join_all;
use tokio::sync::mpsc;

/*
 * NOTE: Connection Manager is meant to be a root context that will delegate work to connections,
 * keep track of downloaded pieces, saves into json format what file we have etc.
 * Connection should worry about peer to which is connected to and thats it, the root context will
 * access the available pieces and thats it.
 */
use crate::{connection::Connection, tracker::Peer};

pub enum ConnectionMessage {
    PieceRecieved(usize, Vec<u8>),
    PiecesAvailable(Vec<usize>),
}

#[derive(Debug)]
pub struct ConnectionManager {
    piece_hashes: Vec<String>,
    tracker_interval: u64,

    rx: mpsc::Receiver<ConnectionMessage>,
    tx: mpsc::Sender<ConnectionMessage>,
}

impl ConnectionManager {
    pub async fn new(
        peers: &[Peer],
        raw_info_hash: Vec<u8>,
        peer_id: String,
        piece_hashes: Vec<String>,
        tracker_interval: u64,
    ) -> Self {
        let (tx, rx) = mpsc::channel::<ConnectionMessage>(100);

        let connections = join_all(peers.iter().map(async |peer| {
            let result = Connection::initialize(
                &raw_info_hash,
                peer_id.as_bytes(),
                &peer.ip,
                &peer.port,
                tx.clone(),
            )
            .await;

            if let Ok(conn) = result {
                return Some(conn);
            }

            None
        }))
        .await
        .into_iter()
        .filter_map(|conn| conn)
        .collect::<Vec<Connection>>();

        println!("{:#?}", connections);
        println!("{:#?}", tracker_interval);

        for mut conn in connections {
            tokio::spawn(async move { conn.read_message().await });
        }

        ConnectionManager {
            rx,
            tx,
            piece_hashes,
            tracker_interval,
        }
    }

    pub async fn download(&mut self) {
        while let Some(msg) = self.rx.recv().await {
            match msg {
                ConnectionMessage::PiecesAvailable(pieces) => {
                    println!("{:?}", pieces);
                }
                ConnectionMessage::PieceRecieved(_, _) => todo!(),
            }
        }

        // TODO: Initial connections find out which peers are available
        //

        // TODO: Determine peer selection strategy (random first)

        // TODO: Delegate peice downloading to connncetions
    }
}
