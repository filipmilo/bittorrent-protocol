use std::collections::HashMap;

use futures::future::join_all;
use tokio::sync::mpsc;

/*
 * NOTE: Connection Manager is meant to be a root context that will delegate work to connections,
 * keep track of downloaded pieces, saves into json format what file we have etc.
 * Connection should worry about peer to which is connected to and thats it, the root context will
 * access the available pieces and thats it.
 */
use crate::{
    connection::{Connection, ConnectionHandle, ConnectionMessage},
    tracker::Peer,
};

pub enum ManagerMessage {
    PieceRecieved(usize, Vec<u8>),
    PiecesAvailable(String, usize),
}

#[derive(Debug)]
pub struct ConnectionManager {
    piece_hashes: Vec<String>,
    tracker_interval: u64,

    connections: HashMap<String, ConnectionHandle>,

    rx: mpsc::Receiver<ManagerMessage>,
    tx: mpsc::Sender<ManagerMessage>,
}

impl ConnectionManager {
    pub async fn new(
        peers: &[Peer],
        raw_info_hash: Vec<u8>,
        peer_id: String,
        piece_hashes: Vec<String>,
        tracker_interval: u64,
    ) -> Self {
        let (tx, rx) = mpsc::channel::<ManagerMessage>(100);

        let connections = join_all(peers.iter().map(async |peer| {
            let result = Connection::initialize(
                &raw_info_hash,
                peer_id.as_bytes(),
                peer.clone(),
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

        let handles: HashMap<String, ConnectionHandle> = connections
            .iter()
            .map(|conn| {
                let handle = conn.create_handle();

                (handle.peer_id.clone(), handle)
            })
            .collect();

        for mut conn in connections {
            tokio::spawn(async move { conn.serve().await });
        }

        ConnectionManager {
            rx,
            tx,
            piece_hashes,
            tracker_interval,
            connections: handles,
        }
    }

    pub async fn download(&mut self) {
        while let Some(msg) = self.rx.recv().await {
            match msg {
                ManagerMessage::PiecesAvailable(peer_id, pieces) => {
                    let conn = self.connections.get_mut(&peer_id).unwrap();

                    conn.available_pieces.push(pieces);

                    let _ = conn.tx.try_send(ConnectionMessage::PieceRequest(pieces));

                    // TODO: Check bitfield
                    //
                }

                ManagerMessage::PieceRecieved(_, _) => todo!(),
            }
        }

        // TODO: Initial connections find out which peers are available
        //

        // TODO: Determine peer selection strategy (random first)

        // TODO: Delegate peice downloading to connncetions
    }
}
