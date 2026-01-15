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

#[derive(Debug)]
struct Bitfield {
    value: Vec<u8>,
}

impl Bitfield {
    pub fn new(piece_number: usize) -> Self {
        Self {
            value: vec![0; (piece_number as f64 / 8.0).ceil() as usize],
        }
    }

    pub fn check_piece(&self, piece_index: usize) -> bool {
        let byte_index = piece_index / 8;
        let bit_index = piece_index % 8;

        let mask = 1 << (7 - bit_index);

        return (self.value[byte_index] & mask) == 1;
    }

    pub fn set_downloaded(&mut self, piece_index: usize) {
        let byte_index = piece_index / 8;
        let bit_index = piece_index % 8;

        let mask = 1 << (7 - bit_index);

        self.value[byte_index] |= mask;
    }
}

pub enum ManagerMessage {
    PieceRecieved(usize, Vec<u8>),
    PiecesAvailable(String, usize),
}

#[derive(Debug)]
pub struct ConnectionManager {
    piece_hashes: Vec<String>,
    bitfield: Bitfield,
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

        let bitfield = Bitfield::new(piece_hashes.len());

        ConnectionManager {
            rx,
            tx,
            bitfield,
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

                    // TODO: Check if downloaded and if not then add it to the queue
                    if !self.bitfield.check_piece(pieces) {
                        let _ = conn.tx.try_send(ConnectionMessage::PieceRequest(pieces));
                    }
                }

                ManagerMessage::PieceRecieved(index, _) => {
                    // TODO: Check piece hash

                    // TODO: Serialize to file

                    self.bitfield.set_downloaded(index);
                }
            }
        }

        // TODO: Initial connections find out which peers are available
        //

        // TODO: Determine peer selection strategy (random first)

        // TODO: Delegate peice downloading to connncetions
    }
}
