use reqwest::Error;

use crate::bencode::{Bencode, BencodedDictionary};

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

#[derive(Debug)]
struct Peer {
    peer_id: String,
    ip: String,
    port: u64,
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
            peer_id: value.get("peer id").unwrap().try_into_string()?,
            ip: value.get("ip").unwrap().try_into_string()?,
            port: value.get("port").unwrap().try_into_int()?,
        })
    }
}

#[derive(Debug)]
struct PeerInfo {
    interval: u64,
    peers: Vec<Peer>,
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
                .try_into_list()?
                .iter()
                .filter_map(|bencoded_peer| match bencoded_peer.try_into_dict() {
                    Ok((val, _)) => match Peer::try_from(val) {
                        Ok(v) => Some(v),
                        Err(_) => None,
                    },
                    Err(_) => None,
                })
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
}

impl TrackerRequest {
    pub fn from(url: String, info_hash: String, peer_id: String, port: u32) -> Self {
        Self {
            url,
            info_hash,
            peer_id,
            port,
            uploaded: "0".into(),
            downloaded: "0".into(),
            left: "0".into(),
            event: None,
        }
    }

    pub async fn fetch_peer_info(&self) -> Result<TrackerResponse, Error> {
        let mut url = format!(
            "{}?info_hash={}&peer_id={}&port={}&uploaded={}&downloaded={}&left={}&compact=1",
            self.url,
            self.info_hash,
            self.peer_id,
            self.port,
            self.uploaded,
            self.downloaded,
            self.left
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
