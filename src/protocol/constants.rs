use std::time::Duration;

// NOTE: 16kiB;
pub const REQUEST_BLOCK_SIZE: usize = 16384;
pub const HANDSHAKE_MESSAGE: &[u8; 19] = b"BitTorrent protocol";
pub const MAX_OUTBOUND_REQUESTS: usize = 5;
pub const NUMBER_OF_WANTED_PEERS: usize = 100;
pub const RAREST_FIRST_DOWNLOAD_COUNT_THRESHOLD: u32 = 4;
pub const END_GAME_PIECE_THRESHOLD: usize = 5;
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
pub const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);
pub const TARGET_LIVE_PEERS: usize = 50;
pub const PEER_FLOOR: usize = 20;
pub const MIN_ANNOUNCE_GAP: Duration = Duration::from_secs(60);
pub const ROSTER_CAP: usize = 2 * NUMBER_OF_WANTED_PEERS;

// BEP 3 puts the peer inactivity timeout at two minutes; sending at half of
// that leaves room for one keep-alive to be lost or delayed.
pub const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(60);
