use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
};

fn construct_handshake(raw_info_hash: &[u8], raw_peer_id: &[u8]) -> Vec<u8> {
    let mut handshake = Vec::with_capacity(68);

    handshake.push(19);
    handshake.extend_from_slice(b"BitTorrent protocol");

    handshake.extend_from_slice(&[0u8; 8]);

    handshake.extend_from_slice(raw_info_hash);

    handshake.extend_from_slice(raw_peer_id);

    handshake
}

pub async fn start_connection(
    raw_info_hash: &[u8],
    raw_peer_id: &[u8],
    ip: &String,
    port: &u64,
) -> std::io::Result<()> {
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

    Ok(())
}
