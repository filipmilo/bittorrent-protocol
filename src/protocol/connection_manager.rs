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
    piece_count: usize,
}

impl Bitfield {
    pub fn new(piece_count: usize) -> Self {
        Self {
            value: vec![0; piece_count.div_ceil(8)],
            piece_count,
        }
    }

    // A peer's bitfield is padded to a byte boundary and arrives from the
    // network, so it is read through `piece_count` rather than trusted for its
    // length: spare bits cannot invent pieces and a short one cannot panic.
    pub fn from(pieces: Vec<u8>, piece_count: usize) -> Self {
        Self {
            value: pieces,
            piece_count,
        }
    }

    pub fn check_piece(&self, piece_index: u32) -> bool {
        let byte_index = (piece_index / 8) as usize;
        let mask = 1 << (7 - piece_index % 8);

        self.value
            .get(byte_index)
            .is_some_and(|byte| byte & mask != 0)
    }

    pub fn set_downloaded(&mut self, piece_index: usize) {
        let byte_index = piece_index / 8;
        let mask = 1 << (7 - piece_index % 8);

        self.value[byte_index] |= mask;
    }

    fn indexes(&self) -> impl Iterator<Item = u32> {
        0..self.piece_count as u32
    }

    pub fn get_available_pieces(&self) -> Vec<u32> {
        self.indexes()
            .filter(|index| self.check_piece(*index))
            .collect()
    }

    pub fn is_completed(&self) -> bool {
        self.indexes().all(|index| self.check_piece(index))
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

                // Counted once per peer per piece, so losing the peer can take
                // exactly as much back off again.
                let fresh = pieces
                    .into_iter()
                    .filter(|piece| conn.available_pieces.insert(*piece))
                    .collect::<Vec<u32>>();

                for piece in fresh {
                    self.piece_availability.increment_piece(piece as usize);
                }

                self.fill_request_slots();

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
                    self.fill_request_slots();
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

                    self.fill_request_slots();
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

    // Every idle peer gets something to do, and each is given the rarest piece
    // *it* holds rather than one piece being sought globally: a single global
    // choice leaves peers idle whenever they happen not to hold it.
    fn fill_request_slots(&mut self) {
        if self.end_game {
            self.broadcast_end_game_requests();
            return;
        }

        let idle = self
            .connections
            .values()
            .filter(|conn| conn.is_available())
            .map(|conn| conn.address.clone())
            .collect::<Vec<String>>();

        for address in idle {
            if let Some(index) = self.next_piece_for(&address) {
                self.assign(&address, index);
            }
        }

        if self.all_pieces_requested() {
            tracing::info!("Entering end game mode");

            let _ = self.progress_tx.send(ProgressEvent::EndGame);

            self.end_game = true;
            self.broadcast_end_game_requests();
        }
    }

    fn next_piece_for(&self, address: &str) -> Option<u32> {
        let conn = self.connections.get(address)?;

        self.piece_availability
            .select(&conn.available_pieces, &self.bitfield, &self.requested_pieces)
    }

    fn assign(&mut self, address: &str, index: u32) {
        let Some(conn) = self.connections.get_mut(address) else {
            return;
        };

        if conn
            .tx
            .try_send(ConnectionMessage::PieceRequest(index))
            .is_err()
        {
            return;
        }

        conn.is_downloading = true;
        conn.current_piece = Some(index);

        self.requested_pieces.insert(index);
        self.piece_owners
            .entry(index)
            .or_default()
            .insert(address.to_string());
    }

    fn missing_pieces(&self) -> impl Iterator<Item = u32> {
        (0..self.piece_hashes.len() as u32).filter(|index| !self.bitfield.check_piece(*index))
    }

    fn all_pieces_requested(&self) -> bool {
        let mut missing = self.missing_pieces().peekable();

        missing.peek().is_some()
            && self
                .missing_pieces()
                .all(|index| self.requested_pieces.contains(&index))
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

#[cfg(test)]
mod tests {
    use super::*;

    fn completed(piece_count: usize) -> Bitfield {
        let mut bitfield = Bitfield::new(piece_count);

        (0..piece_count).for_each(|index| bitfield.set_downloaded(index));

        bitfield
    }

    #[test]
    fn a_fresh_bitfield_holds_no_pieces() {
        let bitfield = Bitfield::new(21754);

        assert!(!bitfield.is_completed());
        assert!(bitfield.get_available_pieces().is_empty());
    }

    #[test]
    fn a_downloaded_piece_reads_back() {
        let mut bitfield = Bitfield::new(20);

        bitfield.set_downloaded(0);
        bitfield.set_downloaded(7);
        bitfield.set_downloaded(19);

        assert_eq!(bitfield.get_available_pieces(), vec![0, 7, 19]);
    }

    // The Debian torrent has 3136 pieces and passed on the old byte-wise check
    // by luck; the Ubuntu torrent has 21754, leaving six spare bits that could
    // never be set, so the download could never report itself finished.
    #[test]
    fn a_piece_count_that_is_not_a_multiple_of_eight_can_still_complete() {
        assert_ne!(21754 % 8, 0);

        assert!(completed(21754).is_completed());
    }

    #[test]
    fn a_piece_count_that_fills_its_last_byte_can_still_complete() {
        assert_eq!(3136 % 8, 0);

        assert!(completed(3136).is_completed());
    }

    #[test]
    fn one_missing_piece_keeps_a_download_incomplete() {
        let mut bitfield = completed(21754);

        bitfield.value[0] &= 0b0111_1111;

        assert!(!bitfield.is_completed());
        assert!(!bitfield.check_piece(0));
    }

    #[test]
    fn spare_bits_in_a_peers_bitfield_do_not_invent_pieces() {
        let bitfield = Bitfield::from(vec![0xFF, 0xFF], 10);

        assert_eq!(bitfield.get_available_pieces(), (0..10).collect::<Vec<u32>>());
    }

    #[test]
    fn a_peer_bitfield_shorter_than_the_torrent_reports_what_it_has() {
        let bitfield = Bitfield::from(vec![0xFF], 21754);

        assert_eq!(bitfield.get_available_pieces(), (0..8).collect::<Vec<u32>>());
        assert!(!bitfield.check_piece(21753));
    }
}
