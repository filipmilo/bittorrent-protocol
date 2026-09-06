pub mod app;
pub mod event_log;
pub mod stats;
pub mod summary;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerRow {
    pub ip: String,
    pub available_pieces: usize,
    pub in_flight: Option<u32>,
    pub choked: bool,
}

#[derive(Debug)]
pub enum ProgressEvent {
    Started {
        total_pieces: usize,
        piece_length: u64,
        output_path: String,
    },
    PieceDownloaded,
    Peers(Vec<PeerRow>),
    HashMismatch {
        index: u32,
    },
    EndGame,
    Completed,
}
