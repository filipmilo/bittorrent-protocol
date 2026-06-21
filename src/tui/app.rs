use std::time::Duration;

use color_eyre::Result;
use crossterm::event::{self, KeyCode};
use ratatui::backend::Backend;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Gauge, LineGauge, List, ListItem, Paragraph, Widget};
use ratatui::{Frame, Terminal};

use crate::tui::ProgressEvent;

pub struct App {
    total_pieces: usize,
    downloaded: usize,
    completed: bool,
    progress_rx: std::sync::mpsc::Receiver<crate::tui::ProgressEvent>,
}

impl App {
    pub fn new(progress_rx: std::sync::mpsc::Receiver<crate::tui::ProgressEvent>) -> Self {
        Self {
            progress_rx,
            total_pieces: 0,
            downloaded: 0,
            completed: false,
        }
    }

    pub fn run<B: Backend>(&mut self, terminal: &mut Terminal<B>) -> Result<()>
    where
        B::Error: Send + Sync + 'static,
    {
        loop {
            terminal.draw(|f| render(f, self))?;

            if event::poll(Duration::from_millis(200)).unwrap() {
                match event::read().unwrap() {
                    event::Event::Key(key) => {
                        if key.code == KeyCode::Esc {
                            self.completed = true;
                        }
                    }
                    _ => {}
                }
            }

            if let Ok(msg) = self.progress_rx.try_recv() {
                match msg {
                    ProgressEvent::Completed => {
                        self.completed = true;
                    }
                    ProgressEvent::Started { total_pieces } => {
                        self.total_pieces = total_pieces;
                    }
                    ProgressEvent::PieceDownloaded => {
                        self.downloaded += 1;
                    }
                }
            }

            if self.completed {
                return Ok(());
            }
        }
    }
}

fn render(frame: &mut Frame, app: &App) {
    let area = frame.area();

    let block = Block::new().title(Line::from("Torrent Download Progress").centered());
    frame.render_widget(block, area);

    let vertical = Layout::vertical([Constraint::Length(2), Constraint::Length(4)]).margin(1);
    //let horizontal = Layout::horizontal([Constraint::Percentage(20), Constraint::Percentage(80)]);
    let [progress_area, _] = area.layout(&vertical);

    // total progress
    let downloaded = app.downloaded;
    let total_pieces = app.total_pieces;
    #[expect(clippy::cast_precision_loss)]
    let progress = LineGauge::default()
        .filled_style(Style::default().fg(Color::Blue))
        .label(format!("{downloaded}/{total_pieces }"))
        .ratio(if total_pieces > 0 {
            downloaded as f64 / total_pieces as f64
        } else {
            0.0
        });

    frame.render_widget(progress, progress_area);

    // in progress downloads
    //let items: Vec<ListItem> = downloads
    //    .in_progress
    //    .values()
    //    .map(|download| {
    //        ListItem::new(Line::from(vec![
    //            Span::raw(symbols::DOT),
    //            Span::styled(
    //                format!(" download {:>2}", download.id),
    //                Style::default()
    //                    .fg(Color::LightGreen)
    //                    .add_modifier(Modifier::BOLD),
    //            ),
    //            Span::raw(format!(
    //                " ({}ms)",
    //                download.started_at.elapsed().as_millis()
    //            )),
    //        ]))
    //    })
    //    .collect();
    //let list = List::new(items);
    //frame.render_widget(list, list_area);

    //#[expect(clippy::cast_possible_truncation)]
    //for (i, (_, download)) in downloads.in_progress.iter().enumerate() {
    //    let gauge = Gauge::default()
    //        .gauge_style(Style::default().fg(Color::Yellow))
    //        .ratio(download.progress / 100.0);
    //    if gauge_area.top().saturating_add(i as u16) > area.bottom() {
    //        continue;
    //    }
    //    frame.render_widget(
    //        gauge,
    //        Rect {
    //            x: gauge_area.left(),
    //            y: gauge_area.top().saturating_add(i as u16),
    //            width: gauge_area.width,
    //            height: 1,
    //        },
    //    );
    //}
}
