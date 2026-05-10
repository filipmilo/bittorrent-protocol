use std::collections::{HashMap, VecDeque};

use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    sync::mpsc,
};

use crate::{
    connection_manager::Bitfield,
    constants::{HANDSHAKE_MESSAGE, MAX_OUTBOUND_REQUESTS, REQUEST_BLOCK_SIZE},
};

use crate::{connection_manager::ManagerMessage, tracker::Peer};

#[derive(Debug)]
enum Messages {
    Choke,
    Unchoke,
    Interested,
    NotInterested,
    Have(usize),
    Bitfield(Vec<u8>),
    Request(usize, usize, usize),
    Piece(usize, usize, Vec<u8>),
    Cancel(usize, usize, usize),
    KeepAlive,
}

impl Messages {
    fn from_code(code: u8, payload: &[u8]) -> Self {
        match code {
            0 => Self::Choke,
            1 => Self::Unchoke,
            2 => Self::Interested,
            3 => Self::NotInterested,
            4 => Self::Have(u32::from_be_bytes(payload.try_into().unwrap()) as usize),
            5 => Self::Bitfield(payload.iter().map(|&val| val).collect()),
            6 => {
                let index = usize::from_be_bytes(payload[0..4].try_into().unwrap());
                let begin = usize::from_be_bytes(payload[4..8].try_into().unwrap());
                let length = usize::from_be_bytes(payload[8..].try_into().unwrap());

                Self::Request(index, begin, length)
            }
            7 => {
                let index = usize::from_be_bytes(payload[0..4].try_into().unwrap());
                let begin = usize::from_be_bytes(payload[4..8].try_into().unwrap());

                let piece: Vec<u8> = payload[8..].iter().map(|&val| val).collect();

                Self::Piece(index, begin, piece)
            }
            8 => {
                let index = usize::from_be_bytes(payload[0..4].try_into().unwrap());
                let begin = usize::from_be_bytes(payload[4..8].try_into().unwrap());
                let length = usize::from_be_bytes(payload[8..].try_into().unwrap());

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

    fn get_request_fields(&self) -> Option<(usize, usize, usize)> {
        if let Messages::Request(index, begin, length) = self {
            return Some((*index, *begin, *length));
        }

        return None;
    }
}

#[derive(Debug)]
pub enum ConnectionMessage {
    PieceRequest(usize),
}

#[derive(Debug)]
pub struct ConnectionHandle {
    pub peer_id: String,
    pub choked: bool,
    pub not_interested: bool,
    pub is_downloading: bool,
    pub available_pieces: Vec<usize>,

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

    fn add_block(&mut self, begin: usize, block: Vec<u8>) -> bool {
        if begin + block.len() < self.piece.len() && self.progress[begin] {
            self.piece.splice(begin..begin, block.iter().cloned());
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
    piece_length: usize,
    available_pieces: Vec<usize>,

    tx: mpsc::Sender<ManagerMessage>,

    rx: mpsc::Receiver<ConnectionMessage>,
    conn_tx: mpsc::Sender<ConnectionMessage>,

    // NOTE: Should only be a chain of Messages::Request type
    download_pipeline: VecDeque<Messages>,
    in_flight_requests: Vec<Messages>,
    request_block_count: usize,

    in_progress: HashMap<usize, PieceProgress>,
}

impl Connection {
    pub async fn initialize(
        piece_length: usize,
        raw_info_hash: &[u8],
        raw_peer_id: &[u8],
        peer: Peer,
        tx: mpsc::Sender<ManagerMessage>,
    ) -> std::io::Result<Self> {
        let mut stream = TcpStream::connect(format!("{}:{}", peer.ip, peer.port)).await?;

        let handshake = Self::construct_handshake(raw_info_hash, raw_peer_id);
        let mut data = vec![0; 68];

        stream.write(&handshake).await?;

        stream.read(&mut data).await?;

        let success_message = if data[28..48] == handshake[28..48] {
            "-> Success"
        } else {
            "-> Failure"
        };

        println!("{}", success_message);

        let (conn_tx, rx) = mpsc::channel::<ConnectionMessage>(100);

        Ok(Connection {
            request_block_count: (piece_length) / REQUEST_BLOCK_SIZE,
            piece_length,
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
            peer_id: self.peer.peer_id.clone(),
            choked: self.choked,
            not_interested: self.not_interested,
            is_downloading: false,
            available_pieces: self.available_pieces.clone(),

            tx: self.conn_tx.clone(),
        }
    }

    pub async fn serve(&mut self) {
        loop {
            tokio::select! {
                result = Self::read_message(&mut self.stream) => {
                    if let Ok(message) = result {
                        println!("Recieved -> {:?}", message);
                        match message {
                            Messages::Have(piece_index) => {
                                self.available_pieces.push(piece_index);

                                let _ = self.tx.try_send(ManagerMessage::PiecesAvailable(
                                    self.peer.peer_id.clone(),
                                    vec![piece_index],
                                ));
                            }
                            Messages::Choke => {
                                self.choked = true;
                            }
                            Messages::Unchoke => {
                                self.choked = false;
                            }
                            Messages::Interested => {
                                self.not_interested = false;
                            }
                            Messages::NotInterested => {
                                self.not_interested = true;
                            }
                            Messages::Bitfield(bitfield) => {
                                let piece_indexes = Bitfield::from(bitfield).get_available_pieces();

                                self.available_pieces.extend(&piece_indexes);

                                let _ = self.tx.try_send(ManagerMessage::PiecesAvailable(
                                    self.peer.peer_id.clone(),
                                    piece_indexes,
                                ));
                            }
                            Messages::Piece(index, begin, piece) => {
                                dbg!("Got piece ->", index, begin, &piece);

                                if let Some(position) = self.in_flight_requests.iter().position(|req| {
                                    let (idx, bgn, _) = req.get_request_fields().unwrap();
                                    idx == index && begin == bgn
                                }) {
                                    self.in_flight_requests.remove(position);

                                    let request = self.download_pipeline.pop_back().unwrap();

                                    Self::write_message(&mut self.stream, &request).await;
                                    self.in_flight_requests.push(request);
                                }


                                let piece_progress = self.in_progress.get_mut(&index).unwrap();
                                piece_progress.add_block(begin, piece);

                                if piece_progress.is_finished() {
                                    let _ = self.tx.try_send(ManagerMessage::PieceRecieved(
                                        index,
                                        piece_progress.piece.clone(),
                                    ));

                                    dbg!(format!("Piece {} downloaded", index));
                                }
                            }
                            _ => {}
                        }
                    }
                }
                Some(instruction) = self.rx.recv()=> {
                    println!("Sending -> {:?}", instruction);

                    match instruction {
                        ConnectionMessage::PieceRequest(index) => {
                            Self::write_message(&mut self.stream, &Messages::Interested).await;

                            self.in_progress.insert(index, PieceProgress::new(self.piece_length, self.request_block_count));

                            let requests = (0..self.request_block_count).map(|val| Messages::Request(index, val * REQUEST_BLOCK_SIZE, REQUEST_BLOCK_SIZE)).rev();

                            println!("Pipelining {} requests for piece {}", requests.len(), index);

                            for request in requests {
                                self.download_pipeline.push_front(request);
                            }

                            while self.in_flight_requests.len() < MAX_OUTBOUND_REQUESTS && self.download_pipeline.len() > 0 {
                                let request = self.download_pipeline.pop_back().unwrap();

                                Self::write_message(&mut self.stream, &request).await;
                                self.in_flight_requests.push(request);
                            }
                        }
                    }

                }

            }
        }
    }

    pub fn get_peer(&self) -> Peer {
        self.peer.clone()
    }

    async fn read_message(stream: &mut TcpStream) -> std::io::Result<Messages> {
        let mut length_data = vec![0u8; 4];
        stream.read_exact(&mut length_data).await?;

        let mut message: Vec<u8> =
            vec![0; u32::from_be_bytes(length_data.try_into().unwrap()) as usize];

        stream.read_exact(&mut message).await?;

        Ok(Messages::from_code(
            *message.first().unwrap(),
            &message[1..message.len()],
        ))
    }

    async fn write_message(stream: &mut TcpStream, message: &Messages) {
        let _ = stream.write_all(&message.to_bytes()).await;
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
