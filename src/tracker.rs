use crate::bencode::BencodedDictionary;

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
    pub async fn fetch_peer_info(&self) -> Result<BencodedDictionary, Error> {
        let mut request = reqwest::get(
            format!(
                "{url}?info_hash={info_hash}&peer_id={peer_id}&port={port}&uploaded={uploaded}&downloaded={downloaded}&left={left}&event={event}",
                url=self.url, 
                info_hash=self.info_hash, 
                peer_id=self.peer_id, 
                port=self.port, 
                uploaded= self.uploaded, 
                downloaded=self.downloaded, 
                left = self.left, 
                event=self.event.as_ref().unwrap().to_string()
                )
            ).await?.bytes().await?;

        Ok(Bencode::decode_dict(request.iter().map(|byte| byte.clone()).collect::<Vec<u8>>()))
    }
}
