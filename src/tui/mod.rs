mod diff_view;
mod editor;
mod review;
mod syntax;

use anyhow::{Context, Result};
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use std::path::PathBuf;
use std::time::Duration;

use crate::diff::FileDiff;
use crate::github::PrMetadata;
use crate::session::Session;
use crate::tui::diff_view::Side;

pub fn run(
    meta: PrMetadata,
    diffs: Vec<FileDiff>,
    session: Session,
    repo_root: PathBuf,
    token: String,
) -> Result<()> {
    let mut terminal = ratatui::init();
    let result = event_loop(&mut terminal, meta, diffs, session, repo_root, token);
    ratatui::restore();
    result
}

fn event_loop(
    terminal: &mut ratatui::DefaultTerminal,
    meta: PrMetadata,
    diffs: Vec<FileDiff>,
    session: Session,
    repo_root: PathBuf,
    token: String,
) -> Result<()> {
    let mut state = review::ReviewState::new(meta, diffs, session, repo_root, token);

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
            KeyCode::Char('e') => open_in_editor(terminal, &state, Side::Head)?,
            KeyCode::Char('E') => open_in_editor(terminal, &state, Side::Base)?,
            KeyCode::Char('v') => state.toggle_viewed(),
            KeyCode::Char('s') => state.toggle_skipped(),
            _ => {}
        }
    }
}

fn open_in_editor(
    terminal: &mut ratatui::DefaultTerminal,
    state: &review::ReviewState,
    side: Side,
) -> Result<()> {
    let Some(target) = state.editor_target(side) else {
        return Ok(());
    };
    ratatui::restore();
    let result = editor::open(&target.file, target.line);
    *terminal = ratatui::init();
    terminal.clear().ok();
    result
}
