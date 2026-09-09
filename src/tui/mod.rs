pub mod app;
pub mod event_log;
pub mod stats;
pub mod summary;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum PeerPhase {
    Downloading,
    Idle,
    Choked,
    Handshaking,
    Connecting,
    Failed,
}

impl PeerPhase {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Downloading => "downloading",
            Self::Idle => "idle",
            Self::Choked => "choked",
            Self::Handshaking => "handshaking",
            Self::Connecting => "connecting",
            Self::Failed => "failed",
        }
    }

    pub fn is_live(&self) -> bool {
        matches!(self, Self::Downloading | Self::Idle | Self::Choked)
    }

    pub fn is_pending(&self) -> bool {
        matches!(self, Self::Handshaking | Self::Connecting)
    }

    pub fn group(&self) -> u8 {
        match (self.is_live(), self.is_pending()) {
            (true, _) => 0,
            (_, true) => 1,
            _ => 2,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerRow {
    pub ip: String,
    pub phase: PeerPhase,
    pub available_pieces: usize,
    pub in_flight: Option<u32>,
}

#[derive(Debug)]
pub enum ProgressEvent {
    Started {
        total_pieces: usize,
        piece_length: u64,
        output_path: String,
    },
    TrackerQuery,
    TrackerPeers {
        count: usize,
    },
    PieceDownloaded,
    Peers(Vec<PeerRow>),
    HashMismatch {
        index: u32,
    },
    EndGame,
    Completed,
}
