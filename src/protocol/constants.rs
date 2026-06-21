// NOTE: 16kiB;
pub const REQUEST_BLOCK_SIZE: usize = 16384;
pub const HANDSHAKE_MESSAGE: &[u8; 19] = b"BitTorrent protocol";
pub const MAX_OUTBOUND_REQUESTS: usize = 5;
pub const NUMBER_OF_WANTED_PEERS: usize = 50;
pub const RAREST_FIRST_DOWNLOAD_COUNT_THRESHOLD: u32 = 4;
