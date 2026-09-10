use std::time::Duration;

use super::stats::{format_bytes, format_duration, format_rate};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitReason {
    Completed,
    Quit,
}

#[derive(Debug)]
pub struct Summary {
    pub reason: ExitReason,
    pub downloaded_pieces: usize,
    pub total_pieces: usize,
    pub bytes: u64,
    pub elapsed: Duration,
    pub output_path: String,
}

impl Summary {
    pub fn lines(&self) -> Vec<String> {
        let (headline, destination) = match self.reason {
            ExitReason::Completed => ("Download complete", "Saved to"),
            ExitReason::Quit => ("Cancelled by user", "Partial file"),
        };

        vec![
            format!(
                "{headline} — {}/{} pieces ({:.1}%)",
                self.downloaded_pieces,
                self.total_pieces,
                self.percentage()
            ),
            format!(
                "Transferred {} in {} (avg {})",
                format_bytes(self.bytes),
                format_duration(Some(self.elapsed)),
                format_rate(self.average_rate())
            ),
            format!("{destination}: {}", self.output_path),
        ]
    }

    fn percentage(&self) -> f64 {
        if self.total_pieces == 0 {
            return 0.0;
        }

        self.downloaded_pieces as f64 / self.total_pieces as f64 * 100.0
    }

    fn average_rate(&self) -> Option<f64> {
        let elapsed = self.elapsed.as_secs_f64();

        (elapsed > 0.0).then(|| self.bytes as f64 / elapsed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary(reason: ExitReason, downloaded_pieces: usize) -> Summary {
        Summary {
            reason,
            downloaded_pieces,
            total_pieces: 100,
            bytes: downloaded_pieces as u64 * 262_144,
            elapsed: Duration::from_secs(10),
            output_path: "debian-13.2.0-amd64-netinst.iso".to_string(),
        }
    }

    fn joined(summary: &Summary) -> String {
        summary.lines().join("\n")
    }

    #[test]
    fn announces_a_finished_download_without_calling_it_cancelled() {
        let text = joined(&summary(ExitReason::Completed, 100));

        assert!(text.contains("complete"), "{text}");
        assert!(!text.to_lowercase().contains("cancel"), "{text}");
    }

    #[test]
    fn distinguishes_an_early_quit_from_a_finished_download() {
        let text = joined(&summary(ExitReason::Quit, 25));

        assert!(text.to_lowercase().contains("cancel"), "{text}");
        assert!(!text.contains("complete"), "{text}");
    }

    #[test]
    fn reports_partial_progress_when_the_user_quits_early() {
        let text = joined(&summary(ExitReason::Quit, 25));

        assert!(text.contains("25/100"), "{text}");
        assert!(text.contains("25.0%"), "{text}");
    }

    #[test]
    fn reports_elapsed_time_and_average_rate() {
        let text = joined(&summary(ExitReason::Completed, 100));

        assert!(text.contains("10s"), "{text}");
        assert!(text.contains("2.5 MiB/s"), "{text}");
    }

    #[test]
    fn always_names_the_output_file() {
        for reason in [ExitReason::Completed, ExitReason::Quit] {
            let text = joined(&summary(reason, 50));

            assert!(
                text.contains("debian-13.2.0-amd64-netinst.iso"),
                "{reason:?}: {text}"
            );
        }
    }

    #[test]
    fn survives_a_quit_before_the_torrent_metadata_arrived() {
        let empty = Summary {
            reason: ExitReason::Quit,
            downloaded_pieces: 0,
            total_pieces: 0,
            bytes: 0,
            elapsed: Duration::ZERO,
            output_path: String::new(),
        };

        assert!(empty.lines().join("\n").contains("0.0%"));
    }
}
