use crate::bencode::{Bencode, BencodedDictionary};

mod bencode;

#[derive(Debug)]
struct TorrentFile {
    announce: String,
    info: Info,
}

impl TryFrom<BencodedDictionary> for TorrentFile {
    type Error = String;

    fn try_from(value: BencodedDictionary) -> Result<Self, Self::Error> {
        if !value.contains_key("announce") || !value.contains_key("info") {
            return Err(String::from("Error parsing TorrentFile, not valid."));
        }

        Ok(TorrentFile {
            announce: value.get("announce").unwrap().try_into_string()?,
            info: Info::try_from(value.get("info").unwrap().try_into_dict()?)?,
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
                Some(val) => Some(Files::try_from(val.try_into_dict()?)?),
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

fn main() {
    let file = std::fs::read("./torrents/ubuntu-25.10-desktop-amd64.iso.torrent")
        .expect("Can't open torrent file.")
        .iter()
        .map(|it| it.clone())
        .collect::<Vec<u8>>();

    let torrent = parse_file(file);

    match torrent {
        Ok(value) => println!("{:?}", value),
        Err(err) => println!("Error: {:?}", err),
    }
}
