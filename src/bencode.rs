use std::collections::HashMap;

#[derive(Clone, Debug)]
pub enum BencodeState {
    String(Vec<u8>),
    Dictionary(HashMap<String, BencodeState>, Vec<u8>),
    List(Vec<BencodeState>),
    Int(u64),
}

pub type BencodedDictionary = HashMap<String, BencodeState>;

impl BencodeState {
    pub fn try_into_string(&self) -> Result<String, String> {
        match self {
            BencodeState::String(value) => {
                Ok(value.iter().map(|&it| char::from(it)).collect::<String>())
            }
            _ => Err(String::from("Error parsing string!")),
        }
    }

    pub fn try_into_int(&self) -> Result<u64, String> {
        match self {
            BencodeState::Int(value) => Ok(*value),
            _ => Err(String::from("Error parsing integer!")),
        }
    }

    pub fn try_into_list(&self) -> Result<Vec<BencodeState>, String> {
        match self {
            BencodeState::List(value) => Ok(value.clone()),
            _ => Err(String::from("Error parsing list!")),
        }
    }

    pub fn try_into_dict(&self) -> Result<(HashMap<String, BencodeState>, Vec<u8>), String> {
        match self {
            BencodeState::Dictionary(value, raw) => Ok((value.clone(), raw.clone())),
            _ => Err(String::from("Error parsing dictionary")),
        }
    }
}

pub struct Bencode {}

impl Bencode {
    fn parse_string(slice: &Vec<u8>, offset: usize) -> (usize, Vec<u8>) {
        let length = slice
            .iter()
            .skip(offset)
            .take_while(|&it| char::from(*it).is_numeric())
            .map(|&it| char::from(it))
            .collect::<String>();

        let new_offset = offset + length.len() + 1;

        let value = slice
            .iter()
            .skip(new_offset)
            .take(length.parse::<usize>().unwrap())
            .map(|v| v.clone())
            .collect::<Vec<u8>>();

        (new_offset + value.len(), value)
    }

    fn parse_int(slice: &Vec<u8>, offset: usize) -> (usize, u64) {
        let new_offset = offset + 1;

        let value = slice
            .iter()
            .skip(new_offset)
            .take_while(|&it| *it != b'e')
            .map(|&it| char::from(it))
            .collect::<String>();

        (new_offset + value.len() + 1, value.parse::<u64>().unwrap())
    }

    fn parse_list(slice: &Vec<u8>, offset: usize) -> (usize, Vec<BencodeState>) {
        let mut list: Vec<BencodeState> = vec![];
        let mut new_offset = offset + 1;

        loop {
            if slice[new_offset] == b'e' {
                new_offset += 1;
                break;
            }

            let (value_end, value) = match slice[new_offset] {
                b'd' => {
                    let (o, v, r) = Self::parse_dictionary(slice, new_offset);

                    (o, BencodeState::Dictionary(v, r))
                }
                b'i' => {
                    let (o, v) = Self::parse_int(slice, new_offset);

                    (o, BencodeState::Int(v))
                }
                b'l' => {
                    let (o, v) = Self::parse_list(slice, new_offset);

                    (o, BencodeState::List(v))
                }
                _ => {
                    let (o, v) = Self::parse_string(slice, new_offset);

                    (o, BencodeState::String(v))
                }
            };

            new_offset = value_end;
            list.push(value);
        }

        (new_offset, list)
    }

    fn parse_dictionary(slice: &Vec<u8>, offset: usize) -> (usize, BencodedDictionary, Vec<u8>) {
        let mut dictionary: BencodedDictionary = HashMap::new();
        let mut raw: Vec<u8> = vec![slice[offset]];

        let mut new_offset = offset + 1;

        loop {
            raw.push(slice[new_offset]);

            if slice[new_offset] == b'e' {
                new_offset += 1;
                break;
            }

            let (key_end, key) = Self::parse_string(slice, new_offset);

            let (value_end, value) = match slice[key_end] {
                b'd' => {
                    let (o, v, r) = Self::parse_dictionary(slice, key_end);

                    (o, BencodeState::Dictionary(v, r))
                }
                b'i' => {
                    let (o, v) = Self::parse_int(slice, key_end);

                    (o, BencodeState::Int(v))
                }
                b'l' => {
                    let (o, v) = Self::parse_list(slice, key_end);

                    (o, BencodeState::List(v))
                }
                _ => {
                    let (o, v) = Self::parse_string(slice, key_end);

                    (o, BencodeState::String(v))
                }
            };

            new_offset = value_end;
            dictionary.insert(
                key.iter().map(|&it| char::from(it)).collect::<String>(),
                value,
            );
        }

        (new_offset, dictionary, raw)
    }

    pub fn decode_dict(slice: Vec<u8>) -> BencodedDictionary {
        Bencode::parse_dictionary(&slice, 0).1
    }
}
