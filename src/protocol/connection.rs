use std::collections::{HashMap, HashSet, VecDeque};

use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    sync::mpsc,
    time::Instant,
};

use super::{
    connection_manager::{Bitfield, ManagerMessage},
    constants::{
        CONNECT_TIMEOUT, HANDSHAKE_MESSAGE, HANDSHAKE_TIMEOUT, KEEPALIVE_INTERVAL,
        MAX_OUTBOUND_REQUESTS, REQUEST_BLOCK_SIZE,
    },
    piece_layout::PieceLayout,
    tracker::Peer,
};

const HANDSHAKE_LENGTH: usize = 68;

async fn timed_out<T>(
    limit: std::time::Duration,
    operation: impl Future<Output = std::io::Result<T>>,
    stage: &'static str,
) -> std::io::Result<T> {
    tokio::time::timeout(limit, operation)
        .await
        .unwrap_or_else(|_| {
            Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                format!("{stage} timed out"),
            ))
        })
}

#[derive(Debug, Clone)]
enum Messages {
    Choke,
    Unchoke,
    Interested,
    NotInterested,
    Have(u32),
    Bitfield(Vec<u8>),
    Request(u32, u32, u32),
    Piece(u32, u32, Vec<u8>),
    Cancel(u32, u32, u32),
    KeepAlive,
}

impl Messages {
    fn keepalive() -> Self {
        Self::KeepAlive
    }

    fn from_code(code: u8, payload: &[u8]) -> Self {
        match code {
            0 => Self::Choke,
            1 => Self::Unchoke,
            2 => Self::Interested,
            3 => Self::NotInterested,
            4 => Self::Have(u32::from_be_bytes(payload.try_into().unwrap()) as u32),
            5 => Self::Bitfield(payload.iter().map(|&val| val).collect()),
            6 => {
                let index = u32::from_be_bytes(payload[0..4].try_into().unwrap());
                let begin = u32::from_be_bytes(payload[4..8].try_into().unwrap());
                let length = u32::from_be_bytes(payload[8..].try_into().unwrap());

                Self::Request(index, begin, length)
            }
            7 => {
                let index = u32::from_be_bytes(payload[0..4].try_into().unwrap());
                let begin = u32::from_be_bytes(payload[4..8].try_into().unwrap());

                let piece: Vec<u8> = payload[8..].iter().map(|&val| val).collect();

                Self::Piece(index, begin, piece)
            }
            8 => {
                let index = u32::from_be_bytes(payload[0..4].try_into().unwrap());
                let begin = u32::from_be_bytes(payload[4..8].try_into().unwrap());
                let length = u32::from_be_bytes(payload[8..].try_into().unwrap());

                Self::Cancel(index, begin, length)
            }
            _ => Self::KeepAlive,
        }
    }

    // Every message is its length as a big-endian u32 followed by that many
    // bytes, so the prefix is derived from the payload rather than restated per
    // variant -- Have and Bitfield used to declare the wrong one.
    fn to_bytes(&self) -> Vec<u8> {
        let payload: Vec<u8> = match self {
            Self::KeepAlive => vec![],
            Self::Choke => vec![0],
            Self::Unchoke => vec![1],
            Self::Interested => vec![2],
            Self::NotInterested => vec![3],
            Self::Have(index) => [&[4][..], &index.to_be_bytes()].concat(),
            Self::Bitfield(bitfield) => [&[5][..], bitfield].concat(),
            Self::Request(index, begin, length) => [
                &[6][..],
                &index.to_be_bytes(),
                &begin.to_be_bytes(),
                &length.to_be_bytes(),
            ]
            .concat(),
            Self::Piece(index, begin, block) => [
                &[7][..],
                &index.to_be_bytes(),
                &begin.to_be_bytes(),
                block,
            ]
            .concat(),
            Self::Cancel(index, begin, length) => [
                &[8][..],
                &index.to_be_bytes(),
                &begin.to_be_bytes(),
                &length.to_be_bytes(),
            ]
            .concat(),
        };

        [&(payload.len() as u32).to_be_bytes()[..], &payload].concat()
    }

    fn get_request_fields(&self) -> Option<(u32, u32, u32)> {
        if let Messages::Request(index, begin, length) = self {
            return Some((*index, *begin, *length));
        }

        return None;
    }
}

#[derive(Debug)]
pub enum ConnectionMessage {
    PieceRequest(u32),
    Cancel(u32),
}

#[derive(Debug)]
pub struct ConnectionHandle {
    pub address: String,
    pub choked: bool,
    pub is_downloading: bool,
    pub current_piece: Option<u32>,
    pub available_pieces: HashSet<u32>,

    pub tx: mpsc::Sender<ConnectionMessage>,
}

impl ConnectionHandle {
    pub fn is_alive(&self) -> bool {
        !self.tx.is_closed()
    }

    pub fn is_available(&self) -> bool {
        self.is_alive() && !self.is_downloading && !self.choked
    }
}

#[derive(Debug)]
struct PieceProgress {
    piece: Vec<u8>,
    progress: Vec<bool>,
}

impl PieceProgress {
    fn new(size: usize, block_count: usize) -> Self {
        Self {
            piece: vec![0; size],
            progress: vec![false; block_count],
        }
    }

    fn add_block(&mut self, begin: u32, block: Vec<u8>) -> bool {
        let begin_indx = begin as usize;
        let progress_indx = begin_indx / REQUEST_BLOCK_SIZE;

        if begin_indx + block.len() <= self.piece.len() && !self.progress[progress_indx] {
            self.piece[begin_indx..begin_indx + block.len()].copy_from_slice(&block);
            self.progress[progress_indx] = true;

            return true;
        }
        false
    }

    fn is_finished(&self) -> bool {
        self.progress.iter().all(|block| *block)
    }
}

#[derive(Debug)]
pub struct Connection {
    peer: Peer,
    stream: TcpStream,
    choked: bool,
    am_interested: bool,
    layout: PieceLayout,

    tx: mpsc::Sender<ManagerMessage>,

    rx: mpsc::Receiver<ConnectionMessage>,
    conn_tx: mpsc::Sender<ConnectionMessage>,

    // NOTE: Should only be a chain of Messages::Request type
    download_pipeline: VecDeque<Messages>,
    in_flight_requests: Vec<Messages>,

    in_progress: HashMap<u32, PieceProgress>,
}

impl Connection {
    pub async fn initialize(
        layout: PieceLayout,
        raw_info_hash: &[u8],
        raw_peer_id: &[u8],
        peer: Peer,
        tx: mpsc::Sender<ManagerMessage>,
    ) -> std::io::Result<Self> {
        let mut stream = timed_out(
            CONNECT_TIMEOUT,
            TcpStream::connect(peer.address()),
            "connect",
        )
        .await?;

        let _ = tx
            .send(ManagerMessage::Handshaking(peer.address()))
            .await;

        let handshake = Self::construct_handshake(raw_info_hash, raw_peer_id);
        let mut data = vec![0; HANDSHAKE_LENGTH];

        timed_out(
            HANDSHAKE_TIMEOUT,
            async {
                stream.write_all(&handshake).await?;
                stream.read_exact(&mut data).await
            },
            "handshake",
        )
        .await?;

        if data[28..48] != handshake[28..48] {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "info hash mismatch",
            ));
        }

        tracing::info!("Peer: {} -> Success", peer.address());

        let (conn_tx, rx) = mpsc::channel::<ConnectionMessage>(100);

        Ok(Connection {
            layout,
            tx,
            conn_tx,
            rx,
            stream,
            peer,
            choked: true,
            am_interested: false,
            download_pipeline: VecDeque::new(),
            in_flight_requests: vec![],
            in_progress: HashMap::new(),
        })
    }

    pub fn create_handle(&self) -> ConnectionHandle {
        ConnectionHandle {
            address: self.peer.address(),
            choked: self.choked,
            is_downloading: false,
            current_piece: None,
            available_pieces: HashSet::new(),

            tx: self.conn_tx.clone(),
        }
    }

    pub async fn serve(&mut self) {
        let mut last_write = Instant::now();

        loop {
            tokio::select! {
                _ = tokio::time::sleep_until(last_write + KEEPALIVE_INTERVAL) => {
                    Self::write_message(&mut self.stream, &Messages::keepalive(), &mut last_write).await;
                }
                result = Self::read_message(&mut self.stream) => {
                    match result {
                        Err(error) => {
                            tracing::info!("Peer {} disconnected: {}", self.peer.address(), error);

                            let _ = self
                                .tx
                                .send(ManagerMessage::Disconnected(self.peer.address()))
                                .await;

                            return;
                        }
                        Ok(message) => match message {
                            Messages::Have(piece_index) => {
                                Self::declare_interest(&mut self.stream, &mut self.am_interested, &mut last_write).await;

                                let _ = self.tx.try_send(ManagerMessage::PiecesAvailable(
                                    self.peer.address(),
                                    vec![piece_index],
                                ));
                            }
                            Messages::Choke => {
                                self.choked = true;

                                // A choked peer discards what it was already
                                // asked for, so nothing outstanding is coming.
                                self.download_pipeline.clear();
                                self.in_flight_requests.clear();
                                self.in_progress.clear();

                                let _ = self.tx.try_send(ManagerMessage::ChokeState(
                                    self.peer.address(),
                                    true,
                                ));
                            }
                            Messages::Unchoke => {
                                self.choked = false;

                                let _ = self.tx.try_send(ManagerMessage::ChokeState(
                                    self.peer.address(),
                                    false,
                                ));
                            }
                            Messages::Bitfield(bitfield) => {
                                let piece_indexes = Bitfield::from(bitfield, self.layout.piece_count())
                                    .get_available_pieces();

                                if !piece_indexes.is_empty() {
                                    Self::declare_interest(&mut self.stream, &mut self.am_interested, &mut last_write).await;
                                }

                                let _ = self.tx.try_send(ManagerMessage::PiecesAvailable(
                                    self.peer.address(),
                                    piece_indexes,
                                ));
                            }
                            Messages::Piece(index, begin, piece) => {
                                tracing::info!("Got piece {} {}->", index, begin);

                                if let Some(position) = self.in_flight_requests.iter().position(|req| {
                                    let (idx, bgn, _) = req.get_request_fields().unwrap();
                                    idx == index && begin == bgn
                                }) {
                                    self.in_flight_requests.remove(position);

                                    Self::fill_pipeline(
                                        &mut self.stream,
                                        &mut self.download_pipeline,
                                        &mut self.in_flight_requests,
                                        &mut last_write,
                                    )
                                    .await;
                                }


                                // NOTE: Asserting if the piece is in progress since a piece can
                                // arrive during cancelation.
                                if let Some(piece_progress) = self.in_progress.get_mut(&index) {
                                    piece_progress.add_block(begin, piece);

                                    if piece_progress.is_finished() {
                                        let _ = self.tx.try_send(ManagerMessage::PieceRecieved(
                                            self.peer.address(),
                                            index,
                                            piece_progress.piece.clone(),
                                        ));

                                        self.in_progress.remove(&index);
                                        tracing::info!("Piece {} downloaded", index);
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                }
                Some(instruction) = self.rx.recv()=> {
                    tracing::info!("Sending -> {:?}", instruction);

                    match instruction {
                        ConnectionMessage::PieceRequest(index) => {
                            Self::declare_interest(&mut self.stream, &mut self.am_interested, &mut last_write).await;

                            let blocks = self.layout.blocks(index);

                            tracing::info!("Pipelining {} requests for piece {}", blocks.len(), index);

                            self.in_progress.insert(
                                index,
                                PieceProgress::new(self.layout.piece_size(index), blocks.len()),
                            );

                            for (begin, length) in blocks {
                                self.download_pipeline
                                    .push_back(Messages::Request(index, begin, length));
                            }

                            Self::fill_pipeline(
                                &mut self.stream,
                                &mut self.download_pipeline,
                                &mut self.in_flight_requests,
                                &mut last_write,
                            )
                            .await;
                        }
                        ConnectionMessage::Cancel(index) => {
                            self.download_pipeline
                                .retain(|msg| msg.get_request_fields().unwrap().0 != index);

                            let cancelled: Vec<Messages> = self
                                .in_flight_requests
                                .iter()
                                .filter(|msg| msg.get_request_fields().unwrap().0 == index)
                                .cloned()
                                .collect();

                            for request in cancelled {
                                let (idx, begin, length) = request.get_request_fields().unwrap();
                                Self::write_message(&mut self.stream, &Messages::Cancel(idx, begin, length), &mut last_write).await;
                            }

                            self.in_flight_requests
                                .retain(|msg| msg.get_request_fields().unwrap().0 != index);
                            self.in_progress.remove(&index);

                            Self::fill_pipeline(
                                &mut self.stream,
                                &mut self.download_pipeline,
                                &mut self.in_flight_requests,
                                &mut last_write,
                            )
                            .await;
                        }
                    }

                }

            }
        }
    }

    async fn read_message(stream: &mut TcpStream) -> std::io::Result<Messages> {
        let mut length_data = vec![0u8; 4];
        stream.read_exact(&mut length_data).await?;

        let mut message: Vec<u8> =
            vec![0; u32::from_be_bytes(length_data.try_into().unwrap()) as usize];

        stream.read_exact(&mut message).await?;

        if message.len() == 0 {
            return Ok(Messages::keepalive());
        }

        Ok(Messages::from_code(
            *message.first().unwrap(),
            &message[1..message.len()],
        ))
    }

    // BEP 3 unchokes the peers that are interested, so interest has to be
    // declared as soon as the peer announces something worth having. Sending it
    // only alongside the first request leaves the peer with no reason to have
    // unchoked us yet, and a choked peer drops that request on the floor.
    async fn declare_interest(
        stream: &mut TcpStream,
        am_interested: &mut bool,
        last_write: &mut Instant,
    ) {
        if *am_interested {
            return;
        }

        Self::write_message(stream, &Messages::Interested, last_write).await;
        *am_interested = true;
    }

    // Keeps `MAX_OUTBOUND_REQUESTS` blocks on the wire so the peer always has
    // work queued; the pipeline is drained in order, which keeps every block of
    // one piece ahead of the next piece's (BEP 3 strict priority).
    async fn fill_pipeline(
        stream: &mut TcpStream,
        pipeline: &mut VecDeque<Messages>,
        in_flight: &mut Vec<Messages>,
        last_write: &mut Instant,
    ) {
        while in_flight.len() < MAX_OUTBOUND_REQUESTS {
            let Some(request) = pipeline.pop_front() else {
                return;
            };

            Self::write_message(stream, &request, last_write).await;
            in_flight.push(request);
        }
    }

    async fn write_message(stream: &mut TcpStream, message: &Messages, last_write: &mut Instant) {
        let _ = stream.write_all(&message.to_bytes()).await;

        *last_write = Instant::now();
    }

    fn construct_handshake(raw_info_hash: &[u8], raw_peer_id: &[u8]) -> Vec<u8> {
        let mut handshake = Vec::with_capacity(68);

        handshake.push(19);
        handshake.extend_from_slice(HANDSHAKE_MESSAGE);

        handshake.extend_from_slice(&[0u8; 8]);

        handshake.extend_from_slice(raw_info_hash);

        handshake.extend_from_slice(raw_peer_id);

        handshake
    }
}

#[cfg(test)]
mod tests {
    use tokio::net::TcpListener;

    use super::*;

    async fn idle_connection() -> (Connection, TcpStream, mpsc::Receiver<ManagerMessage>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();

        let stream = TcpStream::connect(address).await.unwrap();
        let (peer_side, _) = listener.accept().await.unwrap();

        let (tx, manager_rx) = mpsc::channel(32);
        let (conn_tx, rx) = mpsc::channel(32);

        let connection = Connection {
            peer: Peer {
                ip: "127.0.0.1".to_string(),
                port: address.port(),
            },
            stream,
            choked: true,
            am_interested: false,
            layout: PieceLayout::new(REQUEST_BLOCK_SIZE as u64, REQUEST_BLOCK_SIZE as u64),
            tx,
            rx,
            conn_tx,
            download_pipeline: VecDeque::new(),
            in_flight_requests: vec![],
            in_progress: HashMap::new(),
        };

        (connection, peer_side, manager_rx)
    }

    #[test]
    fn a_keep_alive_is_four_zero_bytes_on_the_wire() {
        assert_eq!(Messages::keepalive().to_bytes(), vec![0, 0, 0, 0]);
    }

    #[test]
    fn every_message_declares_the_length_that_follows_it() {
        let messages = [
            Messages::Choke,
            Messages::Interested,
            Messages::Have(300),
            Messages::Bitfield(vec![0xFF; 7]),
            Messages::Request(1, 16384, 16384),
            Messages::Piece(1, 0, vec![0; 20]),
            Messages::Cancel(1, 16384, 16384),
            Messages::KeepAlive,
        ];

        for message in messages {
            let encoded = message.to_bytes();
            let declared = u32::from_be_bytes(encoded[0..4].try_into().unwrap());

            assert_eq!(declared as usize, encoded.len() - 4, "{message:?}");
        }
    }

    // The length prefix used to be the piece index for Have and the bitfield
    // length for Bitfield, either of which desynchronises the peer's reader.
    #[test]
    fn have_and_bitfield_carry_their_own_length_not_their_contents() {
        assert_eq!(Messages::Have(300).to_bytes()[0..4], [0, 0, 0, 5]);
        assert_eq!(Messages::Bitfield(vec![0xFF; 7]).to_bytes()[0..4], [0, 0, 0, 8]);
    }

    #[tokio::test]
    async fn a_peer_that_announces_pieces_is_told_we_are_interested() {
        let (mut connection, mut peer, _manager_rx) = idle_connection().await;

        tokio::spawn(async move { connection.serve().await });

        peer.write_all(&Messages::Bitfield(vec![0b1000_0000]).to_bytes())
            .await
            .unwrap();

        let mut interested = [0u8; 5];
        peer.read_exact(&mut interested).await.unwrap();

        assert_eq!(interested, [0, 0, 0, 1, 2]);
    }

    // Paused time advances to the next timer once every task is idle, so a
    // keep-alive arriving first proves no `interested` was written before it.
    #[tokio::test(start_paused = true)]
    async fn a_peer_holding_nothing_is_not_told_we_are_interested() {
        let (mut connection, mut peer, _manager_rx) = idle_connection().await;

        tokio::spawn(async move { connection.serve().await });

        peer.write_all(&Messages::Bitfield(vec![0b0000_0000]).to_bytes())
            .await
            .unwrap();

        let mut next = [0u8; 4];
        peer.read_exact(&mut next).await.unwrap();

        assert_eq!(next, [0, 0, 0, 0]);
    }

    #[tokio::test]
    async fn interest_is_declared_once_however_many_pieces_are_announced() {
        let (mut connection, mut peer, _manager_rx) = idle_connection().await;
        let instructions = connection.conn_tx.clone();

        tokio::spawn(async move { connection.serve().await });

        peer.write_all(&Messages::Have(0).to_bytes()).await.unwrap();

        let mut interested = [0u8; 5];
        peer.read_exact(&mut interested).await.unwrap();
        assert_eq!(interested, [0, 0, 0, 1, 2]);

        peer.write_all(&Messages::Have(0).to_bytes()).await.unwrap();
        instructions
            .send(ConnectionMessage::PieceRequest(0))
            .await
            .unwrap();

        // A second `interested` would leave these five bytes unread and the
        // request would not be what arrives next.
        let mut request = [0u8; 17];
        peer.read_exact(&mut request).await.unwrap();

        assert_eq!(request[0..5], [0, 0, 0, 13, 6]);
    }

    // Paused time auto-advances to the next timer once every task is idle, so
    // asserting a keep-alive *arrives* proves nothing. Assert when it arrives.
    #[tokio::test(start_paused = true)]
    async fn an_idle_connection_is_kept_alive_after_exactly_one_interval() {
        let (mut connection, mut peer, _manager_rx) = idle_connection().await;
        let opened_at = Instant::now();

        tokio::spawn(async move { connection.serve().await });

        let mut received = [0u8; 4];
        peer.read_exact(&mut received).await.unwrap();

        assert_eq!(received, [0, 0, 0, 0]);
        assert_eq!(opened_at.elapsed(), KEEPALIVE_INTERVAL);
    }

    #[tokio::test(start_paused = true)]
    async fn a_request_postpones_the_next_keep_alive_by_a_full_interval() {
        let (mut connection, mut peer, _manager_rx) = idle_connection().await;
        let instructions = connection.conn_tx.clone();

        tokio::spawn(async move { connection.serve().await });

        tokio::time::sleep(KEEPALIVE_INTERVAL / 2).await;

        let requested_at = Instant::now();

        instructions
            .send(ConnectionMessage::PieceRequest(0))
            .await
            .unwrap();

        let mut interested_and_request = [0u8; 5 + 17];
        peer.read_exact(&mut interested_and_request).await.unwrap();

        let mut received = [0u8; 4];
        peer.read_exact(&mut received).await.unwrap();

        assert_eq!(received, [0, 0, 0, 0]);
        assert_eq!(requested_at.elapsed(), KEEPALIVE_INTERVAL);
    }
}
