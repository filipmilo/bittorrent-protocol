use std::net::IpAddr;

use nanoid::nanoid;
use sha1::{Digest, Sha1};

use crate::{
    bencode::{Bencode, BencodedDictionary},
    protocol::start_connection,
    tracker::{TrackerRequest, TrackerResponse},
};

mod bencode;
mod protocol;
mod tracker;

#[derive(Debug)]
struct TorrentFile {
    announce: String,
    info: Info,
    info_raw: Vec<u8>,
}

impl TryFrom<BencodedDictionary> for TorrentFile {
    type Error = String;

    fn try_from(value: BencodedDictionary) -> Result<Self, Self::Error> {
        if !value.contains_key("announce") || !value.contains_key("info") {
            return Err(String::from("Error parsing TorrentFile, not valid."));
        }

        let (info, raw) = value.get("info").unwrap().try_into_dict()?;

        Ok(TorrentFile {
            announce: value.get("announce").unwrap().try_into_string()?,
            info: Info::try_from(info)?,
            info_raw: raw,
        })
    }
}

#[derive(Debug)]
struct Info {
    name: String,
    piece_length: u64,
    pieces: String,
    length: Option<u64>,
    files: Option<Files>,
}

impl TryFrom<BencodedDictionary> for Info {
    type Error = String;

    fn try_from(value: BencodedDictionary) -> Result<Self, Self::Error> {
        if !value.contains_key("name")
            || !value.contains_key("piece length")
            || !value.contains_key("pieces")
        {
            return Err(String::from("Error parsing Info, not valid."));
        }

        Ok(Info {
            name: value.get("name").unwrap().try_into_string()?,
            piece_length: value.get("piece length").unwrap().try_into_int()?,
            pieces: value.get("pieces").unwrap().try_into_string()?,
            length: match value.get("length") {
                Some(val) => Some(val.try_into_int()?),
                None => None,
            },
            files: match value.get("files") {
                Some(val) => {
                    let (files, _) = val.try_into_dict()?;
                    Some(Files::try_from(files)?)
                }
                None => None,
            },
        })
    }
}

#[derive(Debug)]
struct Files {
    length: u64,
    path: String,
}

impl TryFrom<BencodedDictionary> for Files {
    type Error = String;

    fn try_from(value: BencodedDictionary) -> Result<Self, Self::Error> {
        if !value.contains_key("length") || !value.contains_key("path") {
            return Err(String::from("Error parsing Files, not valid."));
        }

        Ok(Files {
            length: value.get("length").unwrap().try_into_int()?,
            path: value.get("path").unwrap().try_into_string()?,
        })
    }
}

fn parse_file(file: Vec<u8>) -> Result<TorrentFile, String> {
    let decoded_dictionary = Bencode::decode_dict(file);

    TorrentFile::try_from(decoded_dictionary)
}

fn perform_hashing(candidate: Vec<u8>) -> (Vec<u8>, String) {
    let mut hasher = Sha1::new();

    hasher.update(candidate);

    let result = hasher.finalize();

    (
        result.iter().map(|val| val.clone()).collect::<Vec<u8>>(),
        result
            .iter()
            .map(|&byte| format!("%{:02x}", byte))
            .collect::<String>(),
    )
}

#[tokio::main]
async fn main() {
    let file = std::fs::read("./torrents/ubuntu-25.10-desktop-amd64.iso.torrent")
        .expect("Can't open torrent file.")
        .iter()
        .map(|it| it.clone())
        .collect::<Vec<u8>>();

    let torrent = parse_file(file);

    if let Ok(torr) = torrent {
        let (raw_info_hash, info_hash) = perform_hashing(torr.info_raw);

        let peer_id = format!("-RS0001-{}", nanoid!(12));

        let tracker_request = TrackerRequest::from(
            torr.announce,
            info_hash,
            peer_id.clone(),
            6881,
            torr.info.length.unwrap(),
        );

        let response = tracker_request.fetch_peer_info().await;

        if let Ok(resp) = response {
            match resp {
                TrackerResponse::Success(peer_info) => {
                    println!("Interval -> {}", peer_info.interval);

                    let ip_v4_peers = peer_info.peers.iter().filter(|peer| !peer.ip.contains(":"));

                    for peer in ip_v4_peers {
                        println!("Connecting to -> {:#?}", peer);

                        let _ = start_connection(
                            &raw_info_hash,
                            peer_id.as_bytes(),
                            &peer.ip,
                            &peer.port,
                        )
                        .await;
                    }
                }

                TrackerResponse::Failure(err) => {
                    println!("{:?}", err);
                }
            }
        }
    }
}
