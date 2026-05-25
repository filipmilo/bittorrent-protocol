pub mod app;

// TODO(human): Define the ProgressEvent enum here
//
pub enum ProgressEvent {
    Started { total_pieces: usize },
    PieceDownloaded,
    Completed,
}
