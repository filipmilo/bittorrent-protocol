use std::collections::HashMap;

use futures::future::join_all;
use tokio::sync::mpsc;

use crate::{
    connection::{Connection, ConnectionHandle, ConnectionMessage},
    tracker::Peer,
};

#[derive(Debug)]
pub struct Bitfield {
    value: Vec<u8>,
}

impl Bitfield {
    pub fn new(piece_number: usize) -> Self {
        Self {
            value: vec![0; (piece_number as f64 / 8.0).ceil() as usize],
        }
    }

    pub fn from(pieces: Vec<u8>) -> Self {
        Self { value: pieces }
    }

    pub fn check_piece(&self, piece_index: u32) -> bool {
        let byte_index = piece_index / 8;
        let bit_index = piece_index % 8;

        let mask = 1 << (7 - bit_index);

        return (self.value[byte_index as usize] & mask) != 0;
    }

    pub fn set_downloaded(&mut self, piece_index: usize) {
        let byte_index = piece_index / 8;
        let bit_index = piece_index % 8;

        let mask = 1 << (7 - bit_index);

        self.value[byte_index] |= mask;
    }

    pub fn get_available_pieces(&self) -> Vec<u32> {
        self.value
            .iter()
            .enumerate()
            .flat_map(|entry| {
                let (index, byte) = entry;

                let mut indexes: Vec<u32> = vec![];

                for i in 0..8 {
                    let bit_mask = 7 - i;

                    if byte & (1 << bit_mask) != 0 {
                        indexes.push((i + (8 * index)) as u32)
                    }
                }

                indexes
            })
            .collect()
    }
}

pub enum ManagerMessage {
    PieceRecieved(u32, Vec<u8>),
    PiecesAvailable(String, Vec<u32>),
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
        piece_length: u64,
        peers: &[Peer],
        raw_info_hash: Vec<u8>,
        peer_id: String,
        piece_hashes: Vec<String>,
        tracker_interval: u64,
    ) -> Self {
        let (tx, rx) = mpsc::channel::<ManagerMessage>(100);

        let connections = join_all(peers.iter().map(async |peer| {
            let result = Connection::initialize(
                piece_length as usize,
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

        println!(
            "Connected Peers: {:#?}",
            connections
                .iter()
                .map(|conn| conn.get_peer())
                .collect::<Vec<Peer>>()
        );
        println!("Tracker interval: {:#?}", tracker_interval);

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

                    conn.available_pieces.extend(&pieces);

                    for piece in pieces {
                        if !self.bitfield.check_piece(piece) {
                            let _ = conn.tx.try_send(ConnectionMessage::PieceRequest(piece));
                        }
                    }
                }
                ManagerMessage::PieceRecieved(index, _) => {
                    // TODO: Check piece hash

                    // TODO: Serialize to file

                    self.bitfield.set_downloaded(index as usize);
                }
            }
        }

        // TODO: Determine peer selection strategy (random first)

        // TODO: Delegate peice downloading to connncetions
    }
}
