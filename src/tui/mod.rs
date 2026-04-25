mod diff_view;
mod review;
mod syntax;

use anyhow::{Context, Result};
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use std::time::Duration;

use crate::diff::FileDiff;
use crate::github::PrMetadata;

pub fn run(meta: PrMetadata, diffs: Vec<FileDiff>) -> Result<()> {
    let mut terminal = ratatui::init();
    let result = event_loop(&mut terminal, meta, diffs);
    ratatui::restore();
    result
}

fn event_loop(
    terminal: &mut ratatui::DefaultTerminal,
    meta: PrMetadata,
    diffs: Vec<FileDiff>,
) -> Result<()> {
    let mut state = review::ReviewState::new(meta, diffs);

    loop {
        terminal
            .draw(|frame| review::render(frame, &mut state))
            .context("failed to draw frame")?;

        if !event::poll(Duration::from_millis(250)).context("failed to poll terminal events")? {
            continue;
        }

        let Event::Key(key) = event::read().context("failed to read terminal event")? else {
            continue;
        };

        if key.kind != KeyEventKind::Press {
            continue;
        }

        match key.code {
            KeyCode::Char('q') => return Ok(()),
            KeyCode::Tab => state.cycle_focus(),
            KeyCode::Char('j') | KeyCode::Down => state.move_down(),
            KeyCode::Char('k') | KeyCode::Up => state.move_up(),
            KeyCode::Char(']') => state.next_hunk(),
            KeyCode::Char('[') => state.prev_hunk(),
            _ => {}
        }
    }
}
