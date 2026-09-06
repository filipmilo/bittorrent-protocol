use std::collections::{HashMap, HashSet};

use futures::future::join_all;
use tokio::sync::mpsc;

use crate::{protocol::piece_selection::PieceSelection, tui::ProgressEvent};

use super::{
    connection::{Connection, ConnectionHandle, ConnectionMessage},
    file_serializer::FileSerializer,
    tracker::Peer,
    utils::sha1,
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

    pub fn is_completed(&self) -> bool {
        self.value
            .iter()
            .all(|bitfield_section| *bitfield_section == u8::MAX)
    }
}

pub enum ManagerMessage {
    PieceRecieved(String, u32, Vec<u8>),
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

    serializer: FileSerializer,

    progress_tx: std::sync::mpsc::Sender<crate::tui::ProgressEvent>,

    piece_availability: PieceSelection,
    requested_pieces: HashSet<u32>,
    piece_owners: HashMap<u32, HashSet<String>>,
    end_game: bool,
}

impl ConnectionManager {
    pub async fn new(
        piece_length: u64,
        peers: &[Peer],
        raw_info_hash: Vec<u8>,
        peer_id: String,
        piece_hashes: Vec<String>,
        tracker_interval: u64,
        serializer: FileSerializer,
        progress_tx: std::sync::mpsc::Sender<crate::tui::ProgressEvent>,
    ) -> Self {
        let (tx, rx) = mpsc::channel::<ManagerMessage>(100);

        let connections = join_all(peers.iter().map(|peer| async {
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

        tracing::info!(
            "Connected Peers: {:#?}",
            connections
                .iter()
                .map(|conn| conn.get_peer())
                .collect::<Vec<Peer>>()
        );
        tracing::info!("Tracker interval: {:#?}", tracker_interval);

        let handles: HashMap<String, ConnectionHandle> = connections
            .iter()
            .map(|conn| {
                let handle = conn.create_handle();

                (handle.peer_ip.clone(), handle)
            })
            .collect();

        for mut conn in connections {
            tokio::spawn(async move { conn.serve().await });
        }

        let bitfield = Bitfield::new(piece_hashes.len());

        let piece_num = piece_hashes.len();

        ConnectionManager {
            rx,
            tx,
            bitfield,
            piece_hashes,
            tracker_interval,
            serializer,
            progress_tx,
            connections: handles,
            piece_availability: PieceSelection::from(piece_num),
            requested_pieces: HashSet::new(),
            piece_owners: HashMap::new(),
            end_game: false,
        }
    }

    pub async fn download(&mut self) {
        while let Some(msg) = self.rx.recv().await {
            match msg {
                ManagerMessage::PiecesAvailable(peer_ip, pieces) => {
                    let conn = self.connections.get_mut(&peer_ip).unwrap();

                    conn.available_pieces.extend(&pieces);

                    for piece in pieces {
                        self.piece_availability.increment_piece(piece as usize);
                    }

                    self.requrest_next_piece();
                }
                ManagerMessage::PieceRecieved(from, index, piece) => {
                    let conn = self.connections.get_mut(&from).unwrap();
                    conn.is_downloading = false;

                    if self.bitfield.check_piece(index) {
                        continue;
                    }

                    let (_, hex_hash) = sha1(&piece);

                    if self.piece_hashes[index as usize] != hex_hash {
                        tracing::info!(
                            "Piece Hash Validation Failed -> {}: {} != {}, discarding and re-requesting",
                            index,
                            self.piece_hashes[index as usize],
                            hex_hash
                        );

                        self.requested_pieces.remove(&index);
                        self.requrest_next_piece();
                    } else if let Ok(_) = self.serializer.save_piece(index as u64, piece) {
                        self.bitfield.set_downloaded(index as usize);
                        self.requested_pieces.remove(&index);
                        self.piece_availability.increment_download_count();
                        self.finalize_piece(index, &from);

                        let _ = self.progress_tx.send(ProgressEvent::PieceDownloaded);

                        if self.bitfield.is_completed() {
                            print!("File download completed!");
                            let _ = self.progress_tx.send(ProgressEvent::Completed);
                            break;
                        }

                        self.requrest_next_piece();
                    }
                }
            }
        }
    }

    fn requrest_next_piece(&mut self) {
        if self.end_game {
            self.broadcast_end_game_requests();
            return;
        }

        let index = self.piece_availability.get_next_piece_index(&self.bitfield) as u32;

        let handle = self.connections.values_mut().find(|conn_handle| {
            !conn_handle.is_downloading && conn_handle.available_pieces.contains(&index)
        });

        match handle {
            Some(conn) => {
                let _ = conn.tx.try_send(ConnectionMessage::PieceRequest(index));
                conn.is_downloading = true;

                self.requested_pieces.insert(index);
                self.piece_owners
                    .entry(index)
                    .or_default()
                    .insert(conn.peer_ip.clone());
            }
            None => {
                tracing::info!(
                    "No connection available or all connections are downloading for piece : {}",
                    index,
                );
            }
        }

        if self.all_pieces_requested() {
            tracing::info!("Entering end game mode");
            self.end_game = true;
            self.broadcast_end_game_requests();
        }
    }

    fn all_pieces_requested(&self) -> bool {
        self.requested_pieces.len()
            == self.piece_hashes.len() - (self.piece_availability.downloaded_count as usize)
    }

    // End game: every remaining piece has already been assigned to one peer,
    // so instead of waiting on stragglers we ask every peer that holds a
    // still-missing piece for it, and cancel the losers once one copy lands.
    fn broadcast_end_game_requests(&mut self) {
        for index in 0..self.piece_hashes.len() as u32 {
            if self.bitfield.check_piece(index) {
                continue;
            }

            let new_owners: Vec<String> = self
                .connections
                .values()
                .filter(|conn| conn.available_pieces.contains(&index))
                .map(|conn| conn.peer_ip.clone())
                .filter(|peer_ip| {
                    !self
                        .piece_owners
                        .get(&index)
                        .is_some_and(|owners| owners.contains(peer_ip))
                })
                .collect();

            for peer_ip in new_owners {
                if let Some(conn) = self.connections.get(&peer_ip) {
                    let _ = conn.tx.try_send(ConnectionMessage::PieceRequest(index));
                }

                self.piece_owners.entry(index).or_default().insert(peer_ip);
            }
        }
    }

    // Cancels the piece with every peer that was also asked for it during
    // end game, now that one copy has already been received and verified.
    fn finalize_piece(&mut self, index: u32, downloaded_from: &str) {
        if let Some(owners) = self.piece_owners.remove(&index) {
            for peer_ip in owners {
                if peer_ip == downloaded_from {
                    continue;
                }

                if let Some(conn) = self.connections.get(&peer_ip) {
                    let _ = conn.tx.try_send(ConnectionMessage::Cancel(index));
                }
            }
        }
    }
}
