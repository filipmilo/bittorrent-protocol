use core::panic;

use crate::bencode::{Bencode, BencodeState, BencodedDictionary};

mod bencode;

fn parse_bencode_result<T>(result: BencodeState) -> T {}

struct TorrentFile {
    announce: String,
    info: Info,
}

impl TorrentFile {
    fn parse(object: BencodedDictionary) -> TorrentFile {
        if !object.contains_key("announce") || !object.contains_key("info") {
            panic!("Error parsing TorrentFile, not valid.");
        }

        TorrentFile {
            announce: object.get("announce").unwrap(),
            info: Info::parse(object.get("info").unwrap()),
        }
    }
}

struct Info {
    name: String,
    piece_length: i32,
    pieces: String,
    length: Option<i32>,
    files: Option<Files>,
}

impl Info {
    fn parse(object: BencodedDictionary) -> Info {
        if !object.contains_key("name")
            || !object.contains_key("piece_length")
            || !object.contains_key("pieces")
        {
            panic!("Error parsing TorrentFile.Info, not valid.");
        }

        Info {
            name: object.get("name").unwrap(),
            piece_length: object.get("piece_length").unwrap(),
            pieces: object.get("pieces").unwrap(),
        }
    }
}

struct Files {
    length: i32,
    path: String,
}

impl Files {
    fn parse(object: BencodedDictionary) -> Files {
        if !object.contains_key("length") || !object.contains_key("path") {
            panic!("Error parsing TorrentFile.Info.Files, not valid.");
        }

        Files {
            length: object.get("length").unwrap(),
            path: object.get("path").unwrap(),
        }
    }
}

fn parse_file(file: Vec<char>) -> TorrentFile {
    let decoded_dictionary = Bencode::decode_dict(file);

    TorrentFile::parse(decoded_dictionary)
}

fn main() {
    println!("Hello, world!");

    let file = std::fs::read_to_string("../torrents/archlinux-2025.10.01-x86_64.iso.torrent")
        .expect("Can't open torrent file.")
        .chars()
        .collect::<Vec<char>>();

    let torrent = parse_file(file);
}
