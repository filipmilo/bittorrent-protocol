use std::collections::HashSet;

use crate::protocol::{
    connection_manager::Bitfield, constants::RAREST_FIRST_DOWNLOAD_COUNT_THRESHOLD,
};
use rand::seq::IndexedRandom;

#[derive(Debug)]
pub struct PieceSelection {
    piece_availability: Vec<u32>,
    pub downloaded_count: u32,
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

    /// Picks a piece for one peer out of what that peer actually holds, leaving
    /// out anything already downloaded or already asked of someone else.
    ///
    /// The first few pieces are chosen at random: the rarest piece usually has a
    /// single source, and until a complete piece exists there is nothing to
    /// trade back to the swarm. After that it is rarest first.
    pub fn select(
        &self,
        offered: &HashSet<u32>,
        have: &Bitfield,
        requested: &HashSet<u32>,
    ) -> Option<u32> {
        let candidates = offered
            .iter()
            .copied()
            .filter(|index| !have.check_piece(*index) && !requested.contains(index))
            .collect::<Vec<u32>>();

        if self.downloaded_count < RAREST_FIRST_DOWNLOAD_COUNT_THRESHOLD {
            return candidates.choose(&mut rand::rng()).copied();
        }

        self.rarest(&candidates)
    }

    // Rarest first is a swarm-health rule, so it has to break ties randomly:
    // taking the first minimum makes every client sharing a view chase the same
    // piece, which is the imbalance the rule exists to prevent.
    fn rarest(&self, candidates: &[u32]) -> Option<u32> {
        let scarcest = candidates
            .iter()
            .map(|index| self.availability(*index))
            .min()?;

        candidates
            .iter()
            .filter(|index| self.availability(**index) == scarcest)
            .copied()
            .collect::<Vec<u32>>()
            .choose(&mut rand::rng())
            .copied()
    }

    fn availability(&self, index: u32) -> u32 {
        self.piece_availability[index as usize]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn offered(pieces: &[u32]) -> HashSet<u32> {
        pieces.iter().copied().collect()
    }

    // Availability counts, one per piece, with enough pieces downloaded that
    // selection has left random-first behind.
    fn swarm(availability: &[u32]) -> PieceSelection {
        PieceSelection {
            piece_availability: availability.to_vec(),
            downloaded_count: RAREST_FIRST_DOWNLOAD_COUNT_THRESHOLD,
        }
    }

    fn nothing_downloaded(piece_count: usize) -> Bitfield {
        Bitfield::new(piece_count)
    }

    // Piece 0 is the scarcest in the swarm but this peer does not hold it, so
    // the choice falls to the scarcest of the three it does.
    #[test]
    fn picks_the_rarest_piece_the_peer_actually_holds() {
        let selection = swarm(&[1, 2, 9, 9]);

        let picked = selection.select(&offered(&[1, 2, 3]), &nothing_downloaded(4), &HashSet::new());

        assert_eq!(picked, Some(1));
    }

    #[test]
    fn never_offers_a_piece_the_peer_does_not_have() {
        let selection = swarm(&[1, 5, 5, 5]);

        let picked = selection.select(&offered(&[3]), &nothing_downloaded(4), &HashSet::new());

        assert_eq!(picked, Some(3));
    }

    #[test]
    fn skips_pieces_already_downloaded() {
        let selection = swarm(&[1, 2, 3, 4]);
        let mut have = nothing_downloaded(4);

        have.set_downloaded(0);

        let picked = selection.select(&offered(&[0, 1]), &have, &HashSet::new());

        assert_eq!(picked, Some(1));
    }

    // Without this the same rarest piece is handed to a second idle peer on the
    // next event, and two connections race to fetch identical bytes.
    #[test]
    fn skips_pieces_already_requested_from_another_peer() {
        let selection = swarm(&[1, 2, 3, 4]);

        let picked = selection.select(
            &offered(&[0, 1]),
            &nothing_downloaded(4),
            &offered(&[0]),
        );

        assert_eq!(picked, Some(1));
    }

    #[test]
    fn yields_nothing_when_every_offered_piece_is_spoken_for() {
        let selection = swarm(&[1, 2, 3, 4]);

        let picked = selection.select(
            &offered(&[0, 1]),
            &nothing_downloaded(4),
            &offered(&[0, 1]),
        );

        assert_eq!(picked, None);
    }

    #[test]
    fn yields_nothing_when_the_peer_offers_nothing() {
        let selection = swarm(&[1, 2, 3, 4]);

        let picked = selection.select(&HashSet::new(), &nothing_downloaded(4), &HashSet::new());

        assert_eq!(picked, None);
    }

    // Every client with the same view would otherwise chase the same index.
    #[test]
    fn spreads_its_choice_across_pieces_that_are_equally_rare() {
        let selection = swarm(&[2, 2, 2, 9]);
        let all = offered(&[0, 1, 2, 3]);

        let picked = (0..200)
            .filter_map(|_| selection.select(&all, &nothing_downloaded(4), &HashSet::new()))
            .collect::<HashSet<u32>>();

        assert_eq!(picked, offered(&[0, 1, 2]));
    }

    #[test]
    fn the_opening_pieces_are_chosen_at_random_rather_than_rarest() {
        let opening = PieceSelection {
            piece_availability: vec![1, 9, 9, 9],
            downloaded_count: 0,
        };
        let all = offered(&[0, 1, 2, 3]);

        let picked = (0..200)
            .filter_map(|_| opening.select(&all, &nothing_downloaded(4), &HashSet::new()))
            .collect::<HashSet<u32>>();

        assert_eq!(picked, all);
    }

    #[test]
    fn rarest_first_takes_over_once_enough_pieces_have_landed() {
        let mut selection = PieceSelection {
            piece_availability: vec![1, 9, 9, 9],
            downloaded_count: 0,
        };

        (0..RAREST_FIRST_DOWNLOAD_COUNT_THRESHOLD)
            .for_each(|_| selection.increment_download_count());

        let all = offered(&[0, 1, 2, 3]);

        let picked = (0..50)
            .filter_map(|_| selection.select(&all, &nothing_downloaded(4), &HashSet::new()))
            .collect::<HashSet<u32>>();

        assert_eq!(picked, offered(&[0]));
    }
}
