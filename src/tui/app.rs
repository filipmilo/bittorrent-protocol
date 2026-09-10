use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use color_eyre::Result;
use crossterm::event::{self, KeyCode};
use ratatui::backend::Backend;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Cell, LineGauge, Padding, Paragraph, Row, Table};
use ratatui::{Frame, Terminal};

use super::event_log::EventLog;
use super::stats::{Throughput, format_bytes, format_duration, format_rate};
use super::summary::{ExitReason, Summary};
use super::{PeerPhase, PeerRow, ProgressEvent};

const RATE_WINDOW: Duration = Duration::from_secs(5);
const EVENT_LOG_CAPACITY: usize = 64;
const FRAME_INTERVAL: Duration = Duration::from_millis(200);

pub struct App {
    total_pieces: usize,
    piece_length: u64,
    output_path: String,
    downloaded: usize,
    peers: Vec<PeerRow>,
    events: EventLog,
    throughput: Throughput,
    started_at: Instant,
    exit_reason: Option<ExitReason>,
    progress_rx: Receiver<ProgressEvent>,
}

impl App {
    pub fn new(progress_rx: Receiver<ProgressEvent>, started_at: Instant) -> Self {
        Self {
            progress_rx,
            started_at,
            total_pieces: 0,
            piece_length: 0,
            output_path: String::new(),
            downloaded: 0,
            peers: Vec::new(),
            events: EventLog::new(EVENT_LOG_CAPACITY),
            throughput: Throughput::new(RATE_WINDOW, started_at),
            exit_reason: None,
        }
    }

    pub fn apply_pending_events(&mut self, now: Instant) {
        while let Ok(event) = self.progress_rx.try_recv() {
            match event {
                ProgressEvent::Started {
                    total_pieces,
                    piece_length,
                    output_path,
                } => {
                    self.total_pieces = total_pieces;
                    self.piece_length = piece_length;
                    self.output_path = output_path;
                }
                ProgressEvent::TrackerQuery => {
                    self.events.push("Contacting tracker".to_string());
                }
                ProgressEvent::TrackerPeers { count } => {
                    self.events.push(format!("Tracker returned {count} peers"));
                }
                ProgressEvent::PeersDiscovered { count } => {
                    self.events
                        .push(format!("Re-announced, {count} new peers"));
                }
                ProgressEvent::PieceDownloaded => {
                    self.downloaded += 1;
                    self.throughput.record(now, self.piece_length);
                }
                ProgressEvent::Peers(peers) => self.peers = peers,
                ProgressEvent::HashMismatch { index } => self
                    .events
                    .push(format!("Hash mismatch on piece {index}, re-requesting")),
                ProgressEvent::EndGame => {
                    self.events.push("Entered end game mode".to_string());
                }
                ProgressEvent::Completed => self.exit_reason = Some(ExitReason::Completed),
            }
        }
    }

    fn remaining_bytes(&self) -> u64 {
        self.total_pieces.saturating_sub(self.downloaded) as u64 * self.piece_length
    }

    pub fn quit(&mut self) {
        self.exit_reason = Some(ExitReason::Quit);
    }

    pub fn summary(&self, now: Instant) -> Summary {
        Summary {
            reason: self.exit_reason.unwrap_or(ExitReason::Quit),
            downloaded_pieces: self.downloaded,
            total_pieces: self.total_pieces,
            bytes: self.throughput.total_bytes(),
            elapsed: now.saturating_duration_since(self.started_at),
            output_path: self.output_path.clone(),
        }
    }

    pub fn run<B: Backend>(&mut self, terminal: &mut Terminal<B>) -> Result<Summary>
    where
        B::Error: Send + Sync + 'static,
    {
        while self.exit_reason.is_none() {
            let now = Instant::now();

            terminal.draw(|frame| render(frame, self, now))?;

            if event::poll(FRAME_INTERVAL)?
                && let event::Event::Key(key) = event::read()?
                && key.code == KeyCode::Esc
            {
                self.quit();
            }

            self.apply_pending_events(Instant::now());
        }

        Ok(self.summary(Instant::now()))
    }
}

pub fn render(frame: &mut Frame, app: &App, now: Instant) {
    let outer = Block::bordered()
        .border_style(Style::default().fg(Color::DarkGray))
        .title(Line::from(" 🦀 rustorrent ".bold()).centered())
        .title_bottom(Line::from(" Esc to quit ".dim()).centered());
    let inner = outer.inner(frame.area());

    frame.render_widget(outer, frame.area());

    let [gauge_area, stats_area, peers_area, events_area] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(3),
        Constraint::Max(10),
    ])
    .spacing(1)
    .areas(inner);

    frame.render_widget(piece_gauge(app), gauge_area);
    frame.render_widget(transfer_stats(app, now), stats_area);
    frame.render_widget(peer_table(app), peers_area);
    frame.render_widget(event_log(app), events_area);
}

fn piece_gauge(app: &App) -> LineGauge<'_> {
    let ratio = if app.total_pieces > 0 {
        app.downloaded as f64 / app.total_pieces as f64
    } else {
        0.0
    };

    LineGauge::default()
        .filled_style(Style::default().fg(Color::Green))
        .unfilled_style(Style::default().fg(Color::DarkGray))
        .label(format!("{}/{}", app.downloaded, app.total_pieces))
        .ratio(ratio.clamp(0.0, 1.0))
}

fn transfer_stats(app: &App, now: Instant) -> Paragraph<'_> {
    let live_peers = app.peers.iter().filter(|peer| peer.phase.is_live()).count();
    let unchoked = app
        .peers
        .iter()
        .filter(|peer| peer.phase.is_live() && peer.phase != PeerPhase::Choked)
        .count();

    Paragraph::new(Line::from(vec![
        Span::styled(
            format_rate(app.throughput.bytes_per_sec(now)),
            Style::default().fg(Color::Cyan),
        ),
        Span::raw("   ETA "),
        Span::styled(
            format_duration(app.throughput.eta(now, app.remaining_bytes())),
            Style::default().fg(Color::Cyan),
        ),
        Span::raw("   "),
        Span::styled(
            format!("{live_peers} peers"),
            Style::default().fg(Color::Cyan),
        ),
        Span::styled(format!(" ({unchoked} unchoked)"), Style::default().dim()),
        Span::raw("   "),
        Span::styled(
            format_bytes(app.throughput.total_bytes()),
            Style::default().dim(),
        ),
    ]))
}

fn phase_style(phase: PeerPhase) -> Style {
    match phase {
        PeerPhase::Downloading => Style::default().fg(Color::Green),
        PeerPhase::Idle => Style::default().fg(Color::DarkGray),
        PeerPhase::Choked => Style::default().fg(Color::Red),
        PeerPhase::Handshaking => Style::default().fg(Color::Yellow),
        PeerPhase::Connecting => Style::default().fg(Color::Blue),
        PeerPhase::Failed => Style::default().dim(),
    }
}

fn peer_table(app: &App) -> Table<'_> {
    let rows = app.peers.iter().map(|peer| {
        let dash = || "-".to_string();

        Row::new(vec![
            Cell::from(peer.ip.clone()),
            Cell::from(peer.phase.label()).style(phase_style(peer.phase)),
            Cell::from(peer.in_flight.map_or_else(dash, |index| index.to_string())),
            Cell::from(match peer.phase.is_live() {
                true => peer.available_pieces.to_string(),
                false => dash(),
            }),
        ])
    });

    Table::new(
        rows,
        [
            Constraint::Length(23),
            Constraint::Length(13),
            Constraint::Length(10),
            Constraint::Min(5),
        ],
    )
    .header(
        Row::new(vec!["PEER", "STATE", "IN FLIGHT", "HAS"]).style(
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        ),
    )
    .block(
        Block::default()
            .title(Line::from(vec![
                format!("Connections ({})", app.peers.len()).bold(),
                Span::styled(connection_tally(app), Style::default().dim()),
            ]))
            .padding(Padding::left(1)),
    )
}

fn connection_tally(app: &App) -> String {
    let count = |predicate: fn(&PeerPhase) -> bool| {
        app.peers
            .iter()
            .filter(|peer| predicate(&peer.phase))
            .count()
    };

    format!(
        "   {} live · {} pending · {} failed",
        count(PeerPhase::is_live),
        count(PeerPhase::is_pending),
        count(|phase| *phase == PeerPhase::Failed),
    )
}

fn event_log(app: &App) -> Paragraph<'_> {
    let lines = app
        .events
        .entries()
        .rev()
        .map(|entry| Line::from(Span::styled(entry, Style::default().fg(Color::Yellow))))
        .collect::<Vec<_>>();

    Paragraph::new(lines).block(
        Block::default()
            .title(Line::from("Events".bold()))
            .padding(Padding::left(1)),
    )
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc::{Sender, channel};

    use super::*;

    const PIECE_LENGTH: u64 = 262_144;

    fn app_with_events(events: Vec<ProgressEvent>) -> (App, Sender<ProgressEvent>, Instant) {
        let (tx, rx) = channel();
        let start = Instant::now();

        for event in events {
            tx.send(event).unwrap();
        }

        (App::new(rx, start), tx, start)
    }

    fn started() -> ProgressEvent {
        ProgressEvent::Started {
            total_pieces: 100,
            piece_length: PIECE_LENGTH,
            output_path: "debian.iso".to_string(),
        }
    }

    #[test]
    fn drains_every_queued_event_in_a_single_pass() {
        let (mut app, _tx, start) = app_with_events(vec![
            started(),
            ProgressEvent::PieceDownloaded,
            ProgressEvent::PieceDownloaded,
            ProgressEvent::PieceDownloaded,
        ]);

        app.apply_pending_events(start);

        assert_eq!(app.downloaded, 3);
        assert_eq!(app.total_pieces, 100);
    }

    #[test]
    fn converts_downloaded_pieces_into_bytes_for_the_rate() {
        let (mut app, _tx, start) =
            app_with_events(vec![started(), ProgressEvent::PieceDownloaded]);

        app.apply_pending_events(start);

        let rate = app
            .throughput
            .bytes_per_sec(start + Duration::from_secs(1))
            .unwrap();

        assert!((rate - PIECE_LENGTH as f64).abs() < 1.0, "got {rate}");
    }

    #[test]
    fn surfaces_hash_mismatches_in_the_event_log() {
        let (mut app, _tx, start) =
            app_with_events(vec![started(), ProgressEvent::HashMismatch { index: 42 }]);

        app.apply_pending_events(start);

        let log = app.events.entries().collect::<Vec<_>>().join("\n");

        assert!(log.contains("42"), "{log}");
        assert!(log.to_lowercase().contains("hash"), "{log}");
    }

    #[test]
    fn surfaces_end_game_entry_in_the_event_log() {
        let (mut app, _tx, start) = app_with_events(vec![started(), ProgressEvent::EndGame]);

        app.apply_pending_events(start);

        let log = app.events.entries().collect::<Vec<_>>().join("\n");

        assert!(log.to_lowercase().contains("end game"), "{log}");
    }

    #[test]
    fn replaces_the_peer_table_with_the_latest_snapshot() {
        let peer = PeerRow {
            ip: "10.0.0.1".to_string(),
            phase: PeerPhase::Downloading,
            available_pieces: 12,
            in_flight: Some(7),
        };
        let (mut app, _tx, start) =
            app_with_events(vec![started(), ProgressEvent::Peers(vec![peer.clone()])]);

        app.apply_pending_events(start);

        assert_eq!(app.peers, vec![peer]);
    }

    #[test]
    fn a_completed_download_exits_for_a_different_reason_than_a_quit() {
        let (mut app, _tx, start) = app_with_events(vec![started(), ProgressEvent::Completed]);

        app.apply_pending_events(start);

        assert_eq!(app.exit_reason, Some(ExitReason::Completed));
    }

    #[test]
    fn quitting_mid_download_does_not_report_completion() {
        let (mut app, _tx, start) = app_with_events(vec![started()]);

        app.apply_pending_events(start);
        app.quit();

        assert_eq!(app.exit_reason, Some(ExitReason::Quit));
    }

    fn rendered_text(app: &App, now: Instant, width: u16, height: u16) -> String {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| render(frame, app, now)).unwrap();

        let buffer = terminal.backend().buffer();

        (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .filter_map(|x| buffer.cell((x, y)).map(ratatui::buffer::Cell::symbol))
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn populated_app() -> (App, Instant) {
        let peers = vec![
            PeerRow {
                ip: "10.0.0.1".to_string(),
                phase: PeerPhase::Downloading,
                available_pieces: 12,
                in_flight: Some(7),
            },
            PeerRow {
                ip: "10.0.0.2".to_string(),
                phase: PeerPhase::Choked,
                available_pieces: 90,
                in_flight: None,
            },
        ];
        let (mut app, _tx, start) = app_with_events(vec![
            started(),
            ProgressEvent::PieceDownloaded,
            ProgressEvent::Peers(peers),
            ProgressEvent::HashMismatch { index: 42 },
            ProgressEvent::EndGame,
        ]);

        app.apply_pending_events(start);

        (app, start + Duration::from_secs(1))
    }

    #[test]
    fn draws_piece_progress_and_transfer_rate() {
        let (app, now) = populated_app();

        let text = rendered_text(&app, now, 100, 30);

        assert!(text.contains("1/100"), "{text}");
        assert!(text.contains("256.0 KiB/s"), "{text}");
    }

    #[test]
    fn draws_a_row_for_every_connected_peer() {
        let (app, now) = populated_app();

        let text = rendered_text(&app, now, 100, 30);

        assert!(text.contains("10.0.0.1"), "{text}");
        assert!(text.contains("10.0.0.2"), "{text}");
    }

    #[test]
    fn draws_the_event_log() {
        let (app, now) = populated_app();

        let text = rendered_text(&app, now, 100, 30);

        assert!(text.contains("42"), "{text}");
        assert!(text.to_lowercase().contains("end game"), "{text}");
    }

    #[test]
    fn draws_the_connected_peer_count() {
        let (app, now) = populated_app();

        let text = rendered_text(&app, now, 100, 30);

        assert!(text.contains("2"), "{text}");
        assert!(text.to_lowercase().contains("peer"), "{text}");
    }

    #[test]
    fn names_the_peer_count_on_the_connections_block_so_it_survives_clipping() {
        let (app, now) = populated_app();

        let text = rendered_text(&app, now, 100, 30);

        assert!(text.contains("Connections (2)"), "{text}");
    }

    fn peer(ip: &str, phase: PeerPhase) -> PeerRow {
        PeerRow {
            ip: ip.to_string(),
            phase,
            available_pieces: 0,
            in_flight: None,
        }
    }

    fn app_showing(peers: Vec<PeerRow>) -> (App, Instant) {
        let (mut app, _tx, start) =
            app_with_events(vec![started(), ProgressEvent::Peers(peers)]);

        app.apply_pending_events(start);

        (app, start)
    }

    #[test]
    fn draws_peers_that_have_not_finished_connecting_yet() {
        let (app, now) = app_showing(vec![
            peer("10.0.0.1", PeerPhase::Connecting),
            peer("10.0.0.2", PeerPhase::Handshaking),
        ]);

        let text = rendered_text(&app, now, 100, 30);

        assert!(text.contains("10.0.0.1"), "{text}");
        assert!(text.contains("connecting"), "{text}");
        assert!(text.contains("handshaking"), "{text}");
    }

    #[test]
    fn draws_peers_that_failed_to_connect() {
        let (app, now) = app_showing(vec![peer("10.0.0.9", PeerPhase::Failed)]);

        let text = rendered_text(&app, now, 100, 30);

        assert!(text.contains("10.0.0.9"), "{text}");
        assert!(text.contains("failed"), "{text}");
    }

    #[test]
    fn tallies_live_pending_and_failed_peers_separately() {
        let (app, now) = app_showing(vec![
            peer("10.0.0.1", PeerPhase::Downloading),
            peer("10.0.0.2", PeerPhase::Idle),
            peer("10.0.0.3", PeerPhase::Connecting),
            peer("10.0.0.4", PeerPhase::Handshaking),
            peer("10.0.0.5", PeerPhase::Connecting),
            peer("10.0.0.6", PeerPhase::Failed),
        ]);

        let text = rendered_text(&app, now, 100, 30);

        assert!(text.contains("2 live"), "{text}");
        assert!(text.contains("3 pending"), "{text}");
        assert!(text.contains("1 failed"), "{text}");
    }

    #[test]
    fn counts_only_live_peers_in_the_transfer_line() {
        let (app, now) = app_showing(vec![
            peer("10.0.0.1", PeerPhase::Downloading),
            peer("10.0.0.2", PeerPhase::Choked),
            peer("10.0.0.3", PeerPhase::Connecting),
            peer("10.0.0.4", PeerPhase::Failed),
        ]);

        let text = rendered_text(&app, now, 100, 30);

        assert!(text.contains("2 peers"), "{text}");
        assert!(text.contains("(1 unchoked)"), "{text}");
    }

    #[test]
    fn a_pending_peer_shows_no_piece_count() {
        let (app, now) = app_showing(vec![PeerRow {
            ip: "10.0.0.1".to_string(),
            phase: PeerPhase::Connecting,
            available_pieces: 0,
            in_flight: None,
        }]);

        let text = rendered_text(&app, now, 100, 30);
        let row = text
            .lines()
            .find(|line| line.contains("10.0.0.1"))
            .expect("peer row");

        assert!(row.contains('-'), "{row}");
    }

    #[test]
    fn logs_the_tracker_handshake_before_any_peer_is_known() {
        let (mut app, _tx, start) = app_with_events(vec![
            started(),
            ProgressEvent::TrackerQuery,
            ProgressEvent::TrackerPeers { count: 47 },
        ]);

        app.apply_pending_events(start);

        let log = app.events.entries().collect::<Vec<_>>().join("\n");

        assert!(log.contains("tracker"), "{log}");
        assert!(log.contains("47 peers"), "{log}");
    }

    #[test]
    fn survives_a_terminal_too_small_to_hold_the_layout() {
        let (app, now) = populated_app();

        rendered_text(&app, now, 20, 4);
    }

    #[test]
    fn draws_before_the_torrent_metadata_arrives() {
        let (tx, rx) = channel();
        let start = Instant::now();
        drop(tx);

        rendered_text(&App::new(rx, start), start, 80, 24);
    }

    #[test]
    fn summarises_the_run_from_the_events_it_received() {
        let (mut app, _tx, start) = app_with_events(vec![
            started(),
            ProgressEvent::PieceDownloaded,
            ProgressEvent::PieceDownloaded,
        ]);

        app.apply_pending_events(start);
        app.quit();

        let summary = app.summary(start + Duration::from_secs(4));

        assert_eq!(summary.reason, ExitReason::Quit);
        assert_eq!(summary.downloaded_pieces, 2);
        assert_eq!(summary.total_pieces, 100);
        assert_eq!(summary.bytes, 2 * PIECE_LENGTH);
        assert_eq!(summary.elapsed.as_secs(), 4);
        assert_eq!(summary.output_path, "debian.iso");
    }
}
