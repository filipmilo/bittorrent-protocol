use crate::protocol::{
    connection_manager::Bitfield, constants::RAREST_FIRST_DOWNLOAD_COUNT_THRESHOLD,
};
use rand::seq::IndexedRandom;

#[derive(Debug)]
pub struct PieceSelection {
    piece_availability: Vec<u32>,
    downloaded_count: u32,
}

impl PieceSelection {
    pub fn from(piece_number: usize) -> Self {
        Self {
            piece_availability: vec![0; piece_number],
            downloaded_count: 0,
        }
    }

    pub fn increment_piece(&mut self, index: usize) {
        self.piece_availability[index] += 1;
    }

    pub fn increment_download_count(&mut self) {
        self.downloaded_count += 1;
    }

    pub fn get_next_piece_index(&self, bitfield: &Bitfield) -> usize {
        if self.downloaded_count < RAREST_FIRST_DOWNLOAD_COUNT_THRESHOLD {
            return self.get_random_piece(bitfield);
        }

        self.get_rarest_piece(bitfield)
    }

    fn get_random_piece(&self, bitfield: &Bitfield) -> usize {
        let pool = self
            .piece_availability
            .iter()
            .enumerate()
            .filter_map(|(index, &available)| {
                if available > 0 && !bitfield.check_piece(index as u32) {
                    Some(index)
                } else {
                    None
                }
            })
            .collect::<Vec<usize>>();

        match pool.choose(&mut rand::rng()) {
            Some(&val) => val,
            None => panic!("No pieces available."),
        }
    }

    fn get_rarest_piece(&self, bitfield: &Bitfield) -> usize {
        self.piece_availability
            .iter()
            .enumerate()
            .filter(|(index, available)| **available > 0 && !bitfield.check_piece(*index as u32))
            .min_by_key(|(_, available)| *available)
            .map(|(index, _)| index)
            .unwrap_or(0)
    }
}
