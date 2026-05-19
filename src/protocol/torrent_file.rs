use super::bencode::BencodedDictionary;

#[derive(Debug)]
pub struct TorrentFile {
    pub announce: String,
    pub info: Info,
    pub info_raw: Vec<u8>,
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
pub struct Info {
    pub name: String,
    pub piece_length: u64,
    pub length: Option<u64>,
    pub pieces: Vec<u8>,
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
            pieces: value.get("pieces").unwrap().try_into_string_vec()?,
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
