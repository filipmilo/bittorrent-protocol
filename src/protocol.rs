use std::{fmt::write, io::Bytes};

use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
};

enum Messages {
    Choke,
    Unchoke,
    Interested,
    NotInterested,
    Have,
    Bitfield,
    Request,
    Piece,
    Cancel,
    KeepAlive,
}

impl Messages {
    fn from_code(code: u8) -> Self {
        match code {
            0 => Self::Choke,
            1 => Self::Unchoke,
            2 => Self::Interested,
            3 => Self::NotInterested,
            4 => Self::Have,
            5 => Self::Bitfield,
            6 => Self::Request,
            7 => Self::Piece,
            8 => Self::Cancel,
            _ => Self::KeepAlive,
        }
    }
}

pub struct Connection {
    stream: TcpStream,
    choked: bool,
    not_interested: bool,
    available_pieces: Vec<usize>,
}

impl Connection {
    pub async fn initialize(
        raw_info_hash: &[u8],
        raw_peer_id: &[u8],
        ip: &String,
        port: &u64,
    ) -> std::io::Result<Connection> {
        let mut stream = TcpStream::connect(format!("{}:{}", ip, port)).await?;

        let handshake = construct_handshake(raw_info_hash, raw_peer_id);
        let mut data = vec![0; 68];

        stream.write(&handshake).await?;

        stream.read(&mut data).await?;

        let success_message = if data[28..48] == handshake[28..48] {
            "-> Success"
        } else {
            "-> Failure"
        };

        println!("{}", success_message);

        Ok(Connection {
            stream,
            choked: true,
            not_interested: true,
            available_pieces: vec![],
        })
    }

    pub async fn read_message(&mut self) -> std::io::Result<()> {
        loop {
            let mut length_data = vec![0u8; 4];

            self.stream.read_exact(&mut length_data).await?;

            println!("{:?}", length_data);

            let bytes: [u8; 4] = length_data.clone().try_into().unwrap();

            let mut message: Vec<u8> = vec![0; u32::from_be_bytes(bytes) as usize];

            self.stream.read_exact(&mut message).await?;

            println!("{:?}", message);

            if !message.len() == 0 {
                return Ok(());
            }
        }
    }
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

// TODO: Create state machine that will download a single piece after
//
