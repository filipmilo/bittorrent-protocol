use tokio::{
    io::{AsyncReadExt, AsyncWriteExt, ReadHalf},
    net::TcpStream,
    sync::mpsc,
};

use crate::{connection_manager::ManagerMessage, tracker::Peer};

#[derive(Debug)]
enum Messages {
    Choke,
    Unchoke,
    Interested,
    NotInterested,
    Have(usize),
    Bitfield,
    Request,
    Piece,
    Cancel,
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
            5 => Self::Bitfield,
            6 => Self::Request,
            7 => Self::Piece,
            8 => Self::Cancel,
            _ => Self::KeepAlive,
        }
    }

    fn to_bytes(&self) -> Vec<u8> {
        let (header, payload): ([u8; 4], Vec<u8>) = match self {
            Self::Choke => ([0, 0, 0, 1], vec![0]),
            Self::Unchoke => ([0, 0, 0, 1], vec![1]),
            Self::Interested => ([0, 0, 0, 1], vec![2]),
            Self::NotInterested => ([0, 0, 0, 1], vec![3]),

            _ => todo!(),
        };

        header.into_iter().chain(payload).collect()
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
pub struct Connection {
    peer: Peer,
    stream: TcpStream,
    choked: bool,
    not_interested: bool,
    available_pieces: Vec<usize>,

    tx: mpsc::Sender<ManagerMessage>,

    rx: mpsc::Receiver<ConnectionMessage>,
    conn_tx: mpsc::Sender<ConnectionMessage>,
}

impl Connection {
    pub async fn initialize(
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
            tx,
            conn_tx,
            rx,
            stream,
            peer,
            choked: true,
            not_interested: true,
            available_pieces: vec![],
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
                                    piece_index,
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

                            _ => {}
                        }
                    }
                }
                Some(instruction) = self.rx.recv()=> {
                    println!("Sending -> {:?}", instruction);

                    match instruction {
                        ConnectionMessage::PieceRequest(index) => {
                            Self::write_message(&mut self.stream, Messages::Interested).await;

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

        Ok(Messages::from_code(
            *message.first().unwrap(),
            &message[1..message.len()],
        ))
    }

    async fn write_message(stream: &mut TcpStream, message: Messages) {
        let _ = stream.write_all(&message.to_bytes()).await;
    }

    fn construct_handshake(raw_info_hash: &[u8], raw_peer_id: &[u8]) -> Vec<u8> {
        let mut handshake = Vec::with_capacity(68);

        handshake.push(19);
        handshake.extend_from_slice(b"BitTorrent protocol");

        handshake.extend_from_slice(&[0u8; 8]);

        handshake.extend_from_slice(raw_info_hash);

        handshake.extend_from_slice(raw_peer_id);

        handshake
    }
}
