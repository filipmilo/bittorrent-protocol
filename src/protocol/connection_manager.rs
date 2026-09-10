use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use tokio::sync::mpsc;

use crate::{protocol::piece_selection::PieceSelection, tui::ProgressEvent};

use super::{
    connection::{Connection, ConnectionHandle, ConnectionMessage},
    constants::{MIN_ANNOUNCE_GAP, PEER_FLOOR, ROSTER_CAP, TARGET_LIVE_PEERS},
    file_serializer::FileSerializer,
    peer_roster::{Lifecycle, Roster},
    piece_layout::PieceLayout,
    tracker::{Peer, TrackerRequest, TrackerResponse},
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

#[derive(Debug, Clone)]
struct Dialer {
    layout: PieceLayout,
    raw_info_hash: Vec<u8>,
    peer_id: String,
}

impl Dialer {
    fn dial(&self, peer: Peer, tx: mpsc::Sender<ManagerMessage>) {
        let dialer = self.clone();

        tokio::spawn(async move {
            let address = peer.address();

            match Connection::initialize(
                dialer.layout,
                &dialer.raw_info_hash,
                dialer.peer_id.as_bytes(),
                peer,
                tx.clone(),
            )
            .await
            {
                Ok(mut conn) => {
                    let connected = ManagerMessage::Connected(address, conn.create_handle());

                    if tx.send(connected).await.is_ok() {
                        conn.serve().await;
                    }
                }
                Err(error) => {
                    tracing::info!("Peer: {} -> Failure ({})", address, error);

                    let _ = tx.send(ManagerMessage::ConnectionFailed(address)).await;
                }
            }
        });
    }
}

pub enum ManagerMessage {
    PeersDiscovered(Vec<Peer>),
    Handshaking(String),
    Connected(String, ConnectionHandle),
    ConnectionFailed(String),
    PieceRecieved(String, u32, Vec<u8>),
    PiecesAvailable(String, Vec<u32>),
    ChokeState(String, bool),
}

#[derive(Debug)]
pub struct ConnectionManager {
    piece_hashes: Vec<String>,
    bitfield: Bitfield,

    connections: HashMap<String, ConnectionHandle>,
    roster: Roster,
    dialer: Dialer,
    announce_tx: mpsc::Sender<()>,

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
    pub fn new(
        layout: PieceLayout,
        peers: &[Peer],
        raw_info_hash: Vec<u8>,
        peer_id: String,
        piece_hashes: Vec<String>,
        tracker_interval: u64,
        tracker_request: TrackerRequest,
        serializer: FileSerializer,
        progress_tx: std::sync::mpsc::Sender<crate::tui::ProgressEvent>,
    ) -> Self {
        let (tx, rx) = mpsc::channel::<ManagerMessage>(100);
        let (announce_tx, announce_rx) = mpsc::channel::<()>(1);

        let dialer = Dialer {
            layout,
            raw_info_hash,
            peer_id,
        };

        peers
            .iter()
            .for_each(|peer| dialer.dial(peer.clone(), tx.clone()));

        tokio::spawn(Self::replenish(
            tracker_request,
            Duration::from_secs(tracker_interval),
            announce_rx,
            tx.clone(),
        ));

        let piece_num = piece_hashes.len();

        ConnectionManager {
            rx,
            tx,
            dialer,
            announce_tx,
            piece_hashes,
            serializer,
            progress_tx,
            bitfield: Bitfield::new(piece_num),
            roster: Roster::from(peers),
            connections: HashMap::new(),
            piece_availability: PieceSelection::from(piece_num),
            requested_pieces: HashSet::new(),
            piece_owners: HashMap::new(),
            end_game: false,
        }
    }

    // The tracker hands back a near-identical pool each time, so a re-announce
    // earns its keep by replacing peers that died rather than by finding many
    // new ones -- and never sooner than MIN_ANNOUNCE_GAP, so a burst of deaths
    // cannot become a burst of announces.
    async fn replenish(
        request: TrackerRequest,
        interval: Duration,
        mut wake: mpsc::Receiver<()>,
        tx: mpsc::Sender<ManagerMessage>,
    ) {
        let mut announced_at = Instant::now();

        loop {
            tokio::select! {
                _ = tokio::time::sleep(interval) => {}
                woken = wake.recv() => {
                    if woken.is_none() {
                        return;
                    }
                }
            }

            let since = announced_at.elapsed();

            if since < MIN_ANNOUNCE_GAP {
                tokio::time::sleep(MIN_ANNOUNCE_GAP - since).await;
            }

            announced_at = Instant::now();

            let discovered = match request.fetch_peer_info().await {
                Ok(TrackerResponse::Success(info)) => info.ip_v4_peers(),
                Ok(TrackerResponse::Failure(reason)) => {
                    tracing::info!("Tracker refused the re-announce: {:?}", reason);
                    continue;
                }
                Err(error) => {
                    tracing::info!("Re-announce failed: {}", error);
                    continue;
                }
            };

            if tx
                .send(ManagerMessage::PeersDiscovered(discovered))
                .await
                .is_err()
            {
                return;
            }
        }
    }

    fn live_peers(&self) -> usize {
        self.connections
            .values()
            .filter(|conn| !conn.tx.is_closed())
            .count()
    }

    fn request_peers_if_short(&self) {
        if self.live_peers() < PEER_FLOOR {
            let _ = self.announce_tx.try_send(());
        }
    }

    pub async fn download(&mut self) {
        self.publish_peers();

        while let Some(msg) = self.rx.recv().await {
            if self.handle_message(msg) {
                break;
            }

            self.request_peers_if_short();
        }
    }

    fn handle_message(&mut self, message: ManagerMessage) -> bool {
        match message {
            ManagerMessage::PeersDiscovered(peers) => {
                let shortfall = TARGET_LIVE_PEERS.saturating_sub(self.live_peers());
                let fresh = self.roster.absorb(&peers, shortfall);

                self.roster.prune(ROSTER_CAP);

                let _ = self
                    .progress_tx
                    .send(ProgressEvent::PeersDiscovered { count: fresh.len() });

                fresh
                    .into_iter()
                    .for_each(|peer| self.dialer.dial(peer, self.tx.clone()));

                self.publish_peers();

                false
            }
            ManagerMessage::Handshaking(peer_ip) => {
                self.roster.mark(&peer_ip, Lifecycle::Handshaking);
                self.publish_peers();

                false
            }
            ManagerMessage::Connected(peer_ip, handle) => {
                self.roster.mark(&peer_ip, Lifecycle::Live);
                self.connections.insert(peer_ip, handle);
                self.publish_peers();

                false
            }
            ManagerMessage::ConnectionFailed(peer_ip) => {
                self.roster.mark(&peer_ip, Lifecycle::Failed);
                self.connections.remove(&peer_ip);
                self.publish_peers();

                false
            }
            ManagerMessage::ChokeState(peer_ip, choked) => {
                if let Some(conn) = self.connections.get_mut(&peer_ip) {
                    conn.choked = choked;
                }

                self.publish_peers();

                false
            }
            ManagerMessage::PiecesAvailable(peer_ip, pieces) => {
                let Some(conn) = self.connections.get_mut(&peer_ip) else {
                    return false;
                };

                // A `Have` only nudges the piece count by one, and arrives once per
                // piece per peer. Only the first announcement adds a table row.
                let joined_the_swarm = conn.available_pieces.is_empty();

                conn.available_pieces.extend(&pieces);

                for piece in pieces {
                    self.piece_availability.increment_piece(piece as usize);
                }

                self.requrest_next_piece();

                if joined_the_swarm {
                    self.publish_peers();
                }

                false
            }
            ManagerMessage::PieceRecieved(from, index, piece) => {
                let Some(conn) = self.connections.get_mut(&from) else {
                    return false;
                };

                conn.is_downloading = false;
                conn.current_piece = None;

                if self.bitfield.check_piece(index) {
                    self.publish_peers();

                    return false;
                }

                let (_, hex_hash) = sha1(&piece);

                if self.piece_hashes[index as usize] != hex_hash {
                    tracing::info!(
                        "Piece Hash Validation Failed -> {}: {} != {}, discarding and re-requesting",
                        index,
                        self.piece_hashes[index as usize],
                        hex_hash
                    );

                    let _ = self.progress_tx.send(ProgressEvent::HashMismatch { index });

                    self.requested_pieces.remove(&index);
                    self.requrest_next_piece();
                    self.publish_peers();

                    return false;
                }

                if self.serializer.save_piece(index as u64, piece).is_ok() {
                    self.bitfield.set_downloaded(index as usize);
                    self.requested_pieces.remove(&index);
                    self.piece_availability.increment_download_count();
                    self.finalize_piece(index, &from);

                    let _ = self.progress_tx.send(ProgressEvent::PieceDownloaded);

                    if self.bitfield.is_completed() {
                        let _ = self.progress_tx.send(ProgressEvent::Completed);

                        return true;
                    }

                    self.requrest_next_piece();
                }

                self.publish_peers();

                false
            }
        }
    }

    fn publish_peers(&self) {
        let _ = self
            .progress_tx
            .send(ProgressEvent::Peers(self.roster.rows(&self.connections)));
    }

    fn requrest_next_piece(&mut self) {
        if self.end_game {
            self.broadcast_end_game_requests();
            return;
        }

        let index = self.piece_availability.get_next_piece_index(&self.bitfield) as u32;

        let handle = self.connections.values_mut().find(|conn_handle| {
            !conn_handle.is_downloading
                && !conn_handle.tx.is_closed()
                && conn_handle.available_pieces.contains(&index)
        });

        match handle {
            Some(conn) => {
                let _ = conn.tx.try_send(ConnectionMessage::PieceRequest(index));
                conn.is_downloading = true;
                conn.current_piece = Some(index);

                self.requested_pieces.insert(index);
                self.piece_owners
                    .entry(index)
                    .or_default()
                    .insert(conn.address.clone());
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

            let _ = self.progress_tx.send(ProgressEvent::EndGame);

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
                .filter(|conn| !conn.tx.is_closed() && conn.available_pieces.contains(&index))
                .map(|conn| conn.address.clone())
                .filter(|address| {
                    !self
                        .piece_owners
                        .get(&index)
                        .is_some_and(|owners| owners.contains(address))
                })
                .collect();

            for address in new_owners {
                if let Some(conn) = self.connections.get(&address) {
                    let _ = conn.tx.try_send(ConnectionMessage::PieceRequest(index));
                }

                self.piece_owners.entry(index).or_default().insert(address);
            }
        }
    }

    // Cancels the piece with every peer that was also asked for it during
    // end game, now that one copy has already been received and verified.
    fn finalize_piece(&mut self, index: u32, downloaded_from: &str) {
        if let Some(owners) = self.piece_owners.remove(&index) {
            for address in owners {
                if address == downloaded_from {
                    continue;
                }

                if let Some(conn) = self.connections.get(&address) {
                    let _ = conn.tx.try_send(ConnectionMessage::Cancel(index));
                }
            }
        }
    }
}
