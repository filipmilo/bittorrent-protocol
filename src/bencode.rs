use std::collections::HashMap;

#[derive(Clone)]
pub enum BencodeState {
    String(String),
    Dictionary(HashMap<String, BencodeState>),
    List(Vec<BencodeState>),
    Int(i32),
}

pub type BencodedDictionary = HashMap<String, BencodeState>;

impl BencodeState {
    pub fn try_into_string(&self) -> Result<String, String> {
        match self {
            BencodeState::String(value) => Ok(value.clone()),
            _ => Err(String::from("Error parsing string!")),
        }
    }

    pub fn try_into_int(&self) -> Result<i32, String> {
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

    pub fn try_into_dict(&self) -> Result<BencodedDictionary, String> {
        match self {
            BencodeState::Dictionary(value) => Ok(value.clone()),
            _ => Err(String::from("Error parsing dictionary")),
        }
    }
}

pub struct Bencode {}

impl Bencode {
    fn parse_string(slice: &Vec<char>, offset: usize) -> (usize, String) {
        let length = slice
            .iter()
            .skip(offset)
            .take_while(|&it| it.is_numeric())
            .collect::<String>();

        let new_offset = offset + length.len();

        let value = slice
            .iter()
            .skip(new_offset)
            .take(length.parse::<usize>().unwrap())
            .collect::<String>();

        (new_offset + value.len(), value)
    }

    fn parse_int(slice: &Vec<char>, offset: usize) -> (usize, i32) {
        let new_offset = offset + 1;

        let value = slice
            .iter()
            .skip(new_offset)
            .take_while(|&it| it.is_numeric())
            .collect::<String>();

        (new_offset + value.len() + 1, value.parse::<i32>().unwrap())
    }

    fn parse_list(slice: &Vec<char>, offset: usize) -> (usize, Vec<BencodeState>) {
        todo!()
    }

    fn parse_dictionary(slice: &Vec<char>, offset: usize) -> (usize, BencodedDictionary) {
        todo!()
    }

    pub fn decode_dict(slice: Vec<char>) -> BencodedDictionary {
        Bencode::parse_dictionary(&slice, 0).1
    }
}
