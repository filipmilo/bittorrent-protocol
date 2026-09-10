use std::collections::VecDeque;
use std::time::{Duration, Instant};

const BINARY_UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
const UNKNOWN: &str = "--";

pub struct Throughput {
    window: Duration,
    started_at: Instant,
    total_bytes: u64,
    samples: VecDeque<(Instant, u64)>,
}

impl Throughput {
    pub fn new(window: Duration, started_at: Instant) -> Self {
        Self {
            window,
            started_at,
            total_bytes: 0,
            samples: VecDeque::new(),
        }
    }

    pub fn record(&mut self, now: Instant, bytes: u64) {
        self.total_bytes += bytes;
        self.samples.push_back((now, bytes));
        self.discard_expired(now);
    }

    pub fn total_bytes(&self) -> u64 {
        self.total_bytes
    }

    pub fn bytes_per_sec(&self, now: Instant) -> Option<f64> {
        if self.total_bytes == 0 {
            return None;
        }

        let elapsed = now
            .saturating_duration_since(self.started_at)
            .min(self.window)
            .as_secs_f64();

        if elapsed <= 0.0 {
            return None;
        }

        Some(self.bytes_within_window(now) as f64 / elapsed)
    }

    pub fn eta(&self, now: Instant, remaining_bytes: u64) -> Option<Duration> {
        self.bytes_per_sec(now)
            .filter(|rate| *rate > 0.0)
            .map(|rate| Duration::from_secs_f64(remaining_bytes as f64 / rate))
    }

    fn discard_expired(&mut self, now: Instant) {
        while self
            .samples
            .front()
            .is_some_and(|(at, _)| now.saturating_duration_since(*at) > self.window)
        {
            self.samples.pop_front();
        }
    }

    fn bytes_within_window(&self, now: Instant) -> u64 {
        self.samples
            .iter()
            .filter(|(at, _)| now.saturating_duration_since(*at) <= self.window)
            .map(|(_, bytes)| bytes)
            .sum()
    }
}

pub fn format_bytes(bytes: u64) -> String {
    let (scaled, unit) = BINARY_UNITS.iter().enumerate().fold(
        (bytes as f64, BINARY_UNITS[0]),
        |(value, unit), (step, next)| {
            let divisor = 1024_f64.powi(step as i32);

            if bytes as f64 >= divisor {
                (bytes as f64 / divisor, *next)
            } else {
                (value, unit)
            }
        },
    );

    if unit == BINARY_UNITS[0] {
        format!("{bytes} {unit}")
    } else {
        format!("{scaled:.1} {unit}")
    }
}

pub fn format_rate(bytes_per_sec: Option<f64>) -> String {
    bytes_per_sec.map_or_else(
        || UNKNOWN.to_string(),
        |rate| format!("{}/s", format_bytes(rate as u64)),
    )
}

pub fn format_duration(duration: Option<Duration>) -> String {
    let Some(duration) = duration else {
        return UNKNOWN.to_string();
    };

    let total = duration.as_secs();
    let (hours, minutes, seconds) = (total / 3600, (total % 3600) / 60, total % 60);

    match (hours, minutes) {
        (0, 0) => format!("{seconds}s"),
        (0, _) => format!("{minutes}m {seconds}s"),
        _ => format!("{hours}h {minutes}m {seconds}s"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PIECE: u64 = 262_144;

    fn throughput_at(start: Instant) -> Throughput {
        Throughput::new(Duration::from_secs(5), start)
    }

    #[test]
    fn reports_no_rate_before_any_piece_lands() {
        let start = Instant::now();
        let throughput = throughput_at(start);

        assert_eq!(
            throughput.bytes_per_sec(start + Duration::from_secs(1)),
            None
        );
    }

    #[test]
    fn divides_by_elapsed_time_while_shorter_than_the_window() {
        let start = Instant::now();
        let mut throughput = throughput_at(start);

        throughput.record(start, PIECE);
        throughput.record(start, PIECE);

        let rate = throughput
            .bytes_per_sec(start + Duration::from_secs(1))
            .unwrap();

        assert!(
            (rate - (2 * PIECE) as f64).abs() < 1.0,
            "expected ~{} got {rate}",
            2 * PIECE
        );
    }

    #[test]
    fn divides_by_the_window_once_elapsed_time_exceeds_it() {
        let start = Instant::now();
        let mut throughput = throughput_at(start);

        throughput.record(start + Duration::from_secs(18), PIECE);
        throughput.record(start + Duration::from_secs(19), PIECE);

        let rate = throughput
            .bytes_per_sec(start + Duration::from_secs(20))
            .unwrap();
        let expected = (2 * PIECE) as f64 / 5.0;

        assert!(
            (rate - expected).abs() < 1.0,
            "expected ~{expected} got {rate}"
        );
    }

    #[test]
    fn drops_samples_that_fall_outside_the_window() {
        let start = Instant::now();
        let mut throughput = throughput_at(start);

        throughput.record(start, PIECE);

        assert_eq!(
            throughput.bytes_per_sec(start + Duration::from_secs(30)),
            Some(0.0)
        );
    }

    #[test]
    fn estimates_remaining_time_from_the_current_rate() {
        let start = Instant::now();
        let mut throughput = throughput_at(start);

        throughput.record(start, PIECE);

        let eta = throughput
            .eta(start + Duration::from_secs(1), PIECE * 10)
            .unwrap();

        assert_eq!(eta.as_secs(), 10);
    }

    #[test]
    fn has_no_estimate_while_the_transfer_is_stalled() {
        let start = Instant::now();
        let mut throughput = throughput_at(start);

        throughput.record(start, PIECE);

        assert_eq!(throughput.eta(start + Duration::from_secs(30), PIECE), None);
    }

    #[test]
    fn scales_byte_counts_to_binary_units() {
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(1024), "1.0 KiB");
        assert_eq!(format_bytes(1_572_864), "1.5 MiB");
        assert_eq!(format_bytes(822_083_584), "784.0 MiB");
    }

    #[test]
    fn renders_an_unknown_rate_as_a_dash() {
        assert_eq!(format_rate(None), "--");
        assert_eq!(format_rate(Some(1_572_864.0)), "1.5 MiB/s");
    }

    #[test]
    fn renders_durations_without_leading_zero_units() {
        assert_eq!(format_duration(None), "--");
        assert_eq!(format_duration(Some(Duration::from_secs(45))), "45s");
        assert_eq!(format_duration(Some(Duration::from_secs(133))), "2m 13s");
        assert_eq!(format_duration(Some(Duration::from_secs(3725))), "1h 2m 5s");
    }
}
