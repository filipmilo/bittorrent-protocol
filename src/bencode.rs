use std::collections::HashMap;

#[derive(Clone, Debug)]
pub enum BencodeState {
    String(Vec<u8>, Vec<u8>),
    Dictionary(HashMap<String, BencodeState>, Vec<u8>),
    List(Vec<BencodeState>, Vec<u8>),
    Int(u64, Vec<u8>),
}

pub type BencodedDictionary = HashMap<String, BencodeState>;

impl BencodeState {
    pub fn try_into_string(&self) -> Result<String, String> {
        match self {
            BencodeState::String(value, _) => {
                Ok(value.iter().map(|&it| char::from(it)).collect::<String>())
            }
            _ => Err(String::from("Error parsing string!")),
        }
    }

    pub fn try_into_int(&self) -> Result<u64, String> {
        match self {
            BencodeState::Int(value, _) => Ok(*value),
            _ => Err(String::from("Error parsing integer!")),
        }
    }

    pub fn try_into_list(&self) -> Result<Vec<BencodeState>, String> {
        match self {
            BencodeState::List(value, _) => Ok(value.clone()),
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
    fn parse_string(slice: &Vec<u8>, offset: usize) -> (usize, Vec<u8>, Vec<u8>) {
        let raw_length = slice
            .iter()
            .skip(offset)
            .take_while(|&it| char::from(*it).is_numeric())
            .collect::<Vec<&u8>>();

        let mut raw: Vec<u8> = raw_length.iter().map(|&byte| byte.clone()).collect();
        raw.push(b':');

        let length = raw_length
            .iter()
            .map(|&it| char::from(*it))
            .collect::<String>();

        let new_offset = offset + length.len() + 1;

        let value = slice
            .iter()
            .skip(new_offset)
            .take(length.parse::<usize>().unwrap())
            .map(|v| v.clone())
            .collect::<Vec<u8>>();

        raw.extend(&value);

        (new_offset + value.len(), value, raw)
    }

    fn parse_int(slice: &Vec<u8>, offset: usize) -> (usize, u64, Vec<u8>) {
        let new_offset = offset + 1;
        let mut raw: Vec<u8> = vec![slice[offset]];

        let raw_value = slice
            .iter()
            .skip(new_offset)
            .take_while(|&it| *it != b'e')
            .collect::<Vec<&u8>>();

        raw.extend(raw_value.clone());

        let value = raw_value
            .iter()
            .map(|&it| char::from(*it))
            .collect::<String>();

        raw.push(b'e');

        (
            new_offset + value.len() + 1,
            value.parse::<u64>().unwrap(),
            raw,
        )
    }

    fn parse_list(slice: &Vec<u8>, offset: usize) -> (usize, Vec<BencodeState>, Vec<u8>) {
        let mut list: Vec<BencodeState> = vec![];
        let mut raw: Vec<u8> = vec![slice[offset]];

        let mut new_offset = offset + 1;

        loop {
            if slice[new_offset] == b'e' {
                raw.push(slice[new_offset]);
                new_offset += 1;
                break;
            }

            let (value_end, value, raw_value) = match slice[new_offset] {
                b'd' => {
                    let (o, v, r) = Self::parse_dictionary(slice, new_offset);

                    (o, BencodeState::Dictionary(v, r.clone()), r)
                }
                b'i' => {
                    let (o, v, r) = Self::parse_int(slice, new_offset);

                    (o, BencodeState::Int(v, r.clone()), r)
                }
                b'l' => {
                    let (o, v, r) = Self::parse_list(slice, new_offset);

                    (o, BencodeState::List(v, r.clone()), r)
                }
                _ => {
                    let (o, v, r) = Self::parse_string(slice, new_offset);

                    (o, BencodeState::String(v.clone(), r.clone()), r)
                }
            };

            new_offset = value_end;

            raw.extend(raw_value);
            list.push(value);
        }

        (new_offset, list, raw)
    }

    fn parse_dictionary(slice: &Vec<u8>, offset: usize) -> (usize, BencodedDictionary, Vec<u8>) {
        let mut dictionary: BencodedDictionary = HashMap::new();
        let mut raw: Vec<u8> = vec![slice[offset]];

        let mut new_offset = offset + 1;

        loop {
            if slice[new_offset] == b'e' {
                raw.push(slice[new_offset]);
                new_offset += 1;
                break;
            }

            let (key_end, key, raw_key) = Self::parse_string(slice, new_offset);

            let (value_end, value, raw_value) = match slice[key_end] {
                b'd' => {
                    let (o, v, r) = Self::parse_dictionary(slice, key_end);

                    (o, BencodeState::Dictionary(v, r.clone()), r)
                }
                b'i' => {
                    let (o, v, r) = Self::parse_int(slice, key_end);

                    (o, BencodeState::Int(v, r.clone()), r)
                }
                b'l' => {
                    let (o, v, r) = Self::parse_list(slice, key_end);

                    (o, BencodeState::List(v, r.clone()), r)
                }
                _ => {
                    let (o, v, r) = Self::parse_string(slice, key_end);

                    (o, BencodeState::String(v.clone(), r.clone()), r)
                }
            };

            new_offset = value_end;

            raw.extend(raw_key);
            raw.extend(raw_value);

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
