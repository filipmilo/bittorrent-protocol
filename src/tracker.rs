use std::ffi::os_str::Display;

use reqwest::Error;

use crate::{
    bencode::{Bencode, BencodedDictionary},
    constants::NUMBER_OF_WANTED_PEERS,
};

enum Event {
    Started,
    Stopped,
    Completed,
}

impl Event {
    pub fn to_string(&self) -> &str {
        match self {
            Self::Started => "started",
            Self::Stopped => "stopped",
            Self::Completed => "completed",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Peer {
    pub ip: String,
    pub port: u16,
}

impl TryFrom<BencodedDictionary> for Peer {
    type Error = String;

    fn try_from(value: BencodedDictionary) -> Result<Self, Self::Error> {
        if !value.contains_key("peer id")
            || !value.contains_key("ip")
            || !value.contains_key("port")
        {
            return Err(String::from("Error parsing Peer, not valid."));
        }

        Ok(Peer {
            ip: value.get("ip").unwrap().try_into_string()?,
            port: value.get("port").unwrap().try_into_int()? as u16,
        })
    }
}

impl TryFrom<&[u8]> for Peer {
    type Error = String;

    fn try_from(chunk: &[u8]) -> Result<Self, Self::Error> {
        let ip = &chunk[0..4];
        let port = &chunk[4..6];

        Ok(Peer {
            ip: format!("{}.{}.{}.{}", ip[0], ip[1], ip[2], ip[3]),
            port: u16::from_be_bytes(port.try_into().unwrap()),
        })
    }
}

#[derive(Debug)]
pub struct PeerInfo {
    pub interval: u64,
    pub peers: Vec<Peer>,
}

impl TryFrom<BencodedDictionary> for PeerInfo {
    type Error = String;

    fn try_from(value: BencodedDictionary) -> Result<Self, Self::Error> {
        if !value.contains_key("interval") || !value.contains_key("peers") {
            return Err(String::from("Error parsing PeerInfo, not valid."));
        }

        Ok(PeerInfo {
            interval: value.get("interval").unwrap().try_into_int()?,
            peers: value
                .get("peers")
                .unwrap()
                .try_into_string_vec()
                .unwrap()
                .chunks(6)
                .map(|chunk| Peer::try_from(chunk).unwrap())
                .collect::<Vec<Peer>>(),
        })
    }
}

#[derive(Debug)]
pub enum TrackerResponse {
    Failure(String),
    Success(PeerInfo),
}

pub struct TrackerRequest {
    url: String,
    info_hash: String,
    peer_id: String,
    port: u32,
    uploaded: String,
    downloaded: String,
    left: String,
    event: Option<Event>,
    compact: bool,
}

impl TrackerRequest {
    pub fn from(url: String, info_hash: String, peer_id: String, port: u32, left: u64) -> Self {
        Self {
            url,
            info_hash,
            peer_id,
            port,
            left: left.to_string(),
            uploaded: "0".into(),
            downloaded: "0".into(),
            event: None,

            // NOTE: Default to compact format to support larger palette of peers.
            compact: true,
        }
    }

    pub async fn fetch_peer_info(&self) -> Result<TrackerResponse, Error> {
        let mut url = format!(
            "{}?info_hash={}&peer_id={}&port={}&uploaded={}&downloaded={}&left={}&compact={}&numwant={}",
            self.url,
            self.info_hash,
            self.peer_id,
            self.port,
            self.uploaded,
            self.downloaded,
            self.left,
            if self.compact { 1 } else { 0 },
            NUMBER_OF_WANTED_PEERS,
        );

        if let Some(event) = &self.event {
            url.push_str(&format!("&event={}", event.to_string()));
        }

        let response = reqwest::get(url).await?.bytes().await?;

        let decoded_response = Bencode::decode_dict(
            response
                .iter()
                .map(|byte| byte.clone())
                .collect::<Vec<u8>>(),
        );

        if decoded_response.contains_key("failure reason") {
            return Ok(TrackerResponse::Failure(
                match decoded_response
                    .get("failure reason")
                    .unwrap()
                    .try_into_string()
                {
                    Ok(val) => val,
                    Err(_) => "Failed to parse TrackerResponse.".to_string(),
                },
            ));
        }

        Ok(match PeerInfo::try_from(decoded_response) {
            Ok(parsed_info) => TrackerResponse::Success(parsed_info),
            Err(err) => TrackerResponse::Failure(err),
        })
    }
}
