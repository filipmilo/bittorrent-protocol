use std::collections::{HashMap, VecDeque};

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

    fn to_bytes(&self) -> Vec<u8> {
        let (header, payload): ([u8; 4], Vec<u8>) = match self {
            Self::Choke => ([0, 0, 0, 1], vec![0]),
            Self::Unchoke => ([0, 0, 0, 1], vec![1]),
            Self::Interested => ([0, 0, 0, 1], vec![2]),
            Self::NotInterested => ([0, 0, 0, 1], vec![3]),
            Self::Have(piece_index) => (
                (*piece_index as u32).to_be_bytes(),
                vec![4]
                    .iter()
                    .chain((*piece_index as u32).to_be_bytes().iter())
                    .cloned()
                    .collect(),
            ),

            Self::Bitfield(bitfield) => (
                (bitfield.len() as u32).to_be_bytes(),
                vec![5].iter().chain(bitfield.iter()).cloned().collect(),
            ),

            Self::Request(index, begin, length) => {
                let index_bytes = index.to_be_bytes();
                let begin_bytes = begin.to_be_bytes();
                let length_bytes = length.to_be_bytes();

                let payload_length = index_bytes.len() + begin_bytes.len() + length_bytes.len() + 1;
                let mut payload = Vec::with_capacity(payload_length);

                payload.extend_from_slice(&[6]);
                payload.extend_from_slice(&index_bytes);
                payload.extend_from_slice(&begin_bytes);
                payload.extend_from_slice(&length_bytes);

                ((payload_length as u32).to_be_bytes(), payload)
            }
            Self::Piece(index, begin, piece) => {
                let index_bytes = index.to_be_bytes();
                let begin_bytes = begin.to_be_bytes();

                let payload_length = index_bytes.len() + begin_bytes.len() + piece.len() + 1;
                let mut payload = Vec::with_capacity(payload_length);

                payload.extend_from_slice(&[7]);
                payload.extend_from_slice(&index_bytes);
                payload.extend_from_slice(&begin_bytes);
                payload.extend_from_slice(&piece);

                ((payload_length as u32).to_be_bytes(), payload)
            }
            Self::Cancel(index, begin, length) => {
                let index_bytes = index.to_be_bytes();
                let begin_bytes = begin.to_be_bytes();
                let length_bytes = length.to_be_bytes();

                let payload_length = index_bytes.len() + begin_bytes.len() + length_bytes.len() + 1;
                let mut payload = Vec::with_capacity(payload_length);

                payload.extend_from_slice(&[8]);
                payload.extend_from_slice(&index_bytes);
                payload.extend_from_slice(&begin_bytes);
                payload.extend_from_slice(&length_bytes);

                ((payload_length as u32).to_be_bytes(), payload)
            }

            Self::KeepAlive => ([0, 0, 0, 0], vec![]),
        };

        header.into_iter().chain(payload).collect()
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
    pub available_pieces: Vec<u32>,

    pub tx: mpsc::Sender<ConnectionMessage>,
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
    not_interested: bool,
    layout: PieceLayout,
    available_pieces: Vec<u32>,

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
            not_interested: true,
            available_pieces: vec![],
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
            available_pieces: self.available_pieces.clone(),

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
                            return;
                        }
                        Ok(message) => match message {
                            Messages::Have(piece_index) => {
                                self.available_pieces.push(piece_index);

                                let _ = self.tx.try_send(ManagerMessage::PiecesAvailable(
                                    self.peer.address(),
                                    vec![piece_index],
                                ));
                            }
                            Messages::Choke => {
                                self.choked = true;

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
                            Messages::Interested => {
                                self.not_interested = false;
                            }
                            Messages::NotInterested => {
                                self.not_interested = true;
                            }
                            Messages::Bitfield(bitfield) => {
                                let piece_indexes = Bitfield::from(bitfield, self.layout.piece_count())
                                    .get_available_pieces();

                                self.available_pieces.extend(&piece_indexes);

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
                            Self::write_message(&mut self.stream, &Messages::Interested, &mut last_write).await;
                            self.not_interested = false;

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
            not_interested: true,
            layout: PieceLayout::new(REQUEST_BLOCK_SIZE as u64, REQUEST_BLOCK_SIZE as u64),
            available_pieces: vec![],
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
