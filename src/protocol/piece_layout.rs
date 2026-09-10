use super::constants::REQUEST_BLOCK_SIZE;

/// Every piece is `piece_length` bytes except the final one, which carries
/// whatever remains. The same raggedness reaches the last block of that piece,
/// so both counts and lengths are derived here rather than assumed anywhere else.
#[derive(Debug, Clone, Copy)]
pub struct PieceLayout {
    piece_length: u64,
    total_length: u64,
}

impl PieceLayout {
    pub fn new(piece_length: u64, total_length: u64) -> Self {
        Self {
            piece_length,
            total_length,
        }
    }

    pub fn piece_count(&self) -> usize {
        self.total_length.div_ceil(self.piece_length) as usize
    }

    pub fn offset(&self, index: u32) -> u64 {
        index as u64 * self.piece_length
    }

    pub fn piece_size(&self, index: u32) -> usize {
        self.total_length
            .saturating_sub(self.offset(index))
            .min(self.piece_length) as usize
    }

    pub fn block_count(&self, index: u32) -> usize {
        self.piece_size(index).div_ceil(REQUEST_BLOCK_SIZE)
    }

    /// `(begin, length)` for each block of `index`, the last one truncated.
    pub fn blocks(&self, index: u32) -> Vec<(u32, u32)> {
        let size = self.piece_size(index);

        (0..size)
            .step_by(REQUEST_BLOCK_SIZE)
            .map(|begin| (begin as u32, REQUEST_BLOCK_SIZE.min(size - begin) as u32))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PIECE: u64 = 262144;

    // The Debian torrent in `torrents/`: divides evenly, which is why it never
    // exposed the ragged tail.
    fn even() -> PieceLayout {
        PieceLayout::new(PIECE, 822083584)
    }

    // The Ubuntu torrent in `torrents/`: 21754 pieces, final piece 102400 bytes.
    fn ragged() -> PieceLayout {
        PieceLayout::new(PIECE, 5702520832)
    }

    #[test]
    fn an_evenly_divided_torrent_has_a_full_final_piece() {
        assert_eq!(even().piece_count(), 3136);
        assert_eq!(even().piece_size(3135), PIECE as usize);
        assert_eq!(even().block_count(3135), 16);
    }

    #[test]
    fn a_ragged_torrent_reports_its_short_final_piece() {
        assert_eq!(ragged().piece_count(), 21754);
        assert_eq!(ragged().piece_size(21753), 102400);
        assert_eq!(ragged().piece_size(21752), PIECE as usize);
    }

    #[test]
    fn a_short_final_piece_needs_fewer_blocks_than_a_full_one() {
        assert_eq!(ragged().block_count(21752), 16);
        assert_eq!(ragged().block_count(21753), 7);
    }

    #[test]
    fn the_final_block_of_a_short_piece_is_truncated() {
        let blocks = ragged().blocks(21753);

        assert_eq!(blocks.len(), 7);
        assert_eq!(blocks[0], (0, REQUEST_BLOCK_SIZE as u32));
        assert_eq!(blocks[5], (81920, REQUEST_BLOCK_SIZE as u32));
        assert_eq!(blocks[6], (98304, 4096));
    }

    #[test]
    fn blocks_of_a_short_piece_sum_to_its_real_length() {
        let total: u32 = ragged().blocks(21753).iter().map(|(_, len)| len).sum();

        assert_eq!(total as usize, ragged().piece_size(21753));
    }

    #[test]
    fn every_block_of_a_full_piece_is_the_request_size() {
        let blocks = ragged().blocks(0);

        assert_eq!(blocks.len(), 16);
        assert!(blocks.iter().all(|(_, len)| *len == REQUEST_BLOCK_SIZE as u32));
    }

    #[test]
    fn pieces_are_seeked_by_nominal_length_even_when_the_last_is_short() {
        assert_eq!(ragged().offset(21753), 21753 * PIECE);
        assert_eq!(
            ragged().offset(21753) + ragged().piece_size(21753) as u64,
            5702520832
        );
    }

    #[test]
    fn a_torrent_smaller_than_one_piece_is_a_single_short_piece() {
        let tiny = PieceLayout::new(PIECE, 1000);

        assert_eq!(tiny.piece_count(), 1);
        assert_eq!(tiny.piece_size(0), 1000);
        assert_eq!(tiny.blocks(0), vec![(0, 1000)]);
    }
}
