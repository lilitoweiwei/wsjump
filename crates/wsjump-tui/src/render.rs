//! Picker rendering: draws the input line, candidate list and status line
//! into the inline viewport.

use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{List, ListItem, ListState, Paragraph};
use ratatui::Frame;

use crate::app::App;
use wsjump_core::workspace::Workspace;

/// Draw the whole picker UI for the current frame.
pub fn draw(frame: &mut Frame, app: &App) {
    // input line (1) + candidate list (fills) + status line (1).
    let [input_area, list_area, status_area] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(1),
    ])
    .areas(frame.area());

    draw_input(frame, input_area, app);
    draw_list(frame, list_area, app);
    draw_status(frame, status_area, app);
}

/// Draw the query input line with a leading prompt and place the cursor.
fn draw_input(frame: &mut Frame, area: Rect, app: &App) {
    const PROMPT: &str = "> ";
    let line = format!("{}{}", PROMPT, app.query());
    frame.render_widget(Paragraph::new(line), area);

    // Position the terminal cursor after the prompt at the input's cursor.
    let x = area.x + PROMPT.len() as u16 + app.cursor() as u16;
    frame.set_cursor_position(Position::new(x, area.y));
}

/// Draw the candidate list, highlighting the selected row.
fn draw_list(frame: &mut Frame, area: Rect, app: &App) {
    let items: Vec<ListItem> = app.matched().map(|ws| ListItem::new(item_line(ws))).collect();

    let list = List::new(items).highlight_style(
        Style::default()
            .add_modifier(Modifier::REVERSED)
            .add_modifier(Modifier::BOLD),
    );

    let mut state = ListState::default();
    if app.match_count() > 0 {
        state.select(Some(app.selected()));
    }
    frame.render_stateful_widget(list, area, &mut state);
}

/// Build a single-line display for one workspace: id + tags + desc.
fn item_line(ws: &Workspace) -> Line<'static> {
    let mut spans = vec![Span::raw(ws.id.clone())];
    if !ws.tags.is_empty() {
        let tags = ws
            .tags
            .iter()
            .map(|t| format!(":{}", t))
            .collect::<Vec<_>>()
            .join(" ");
        spans.push(Span::raw(format!("  {}", tags)));
    }
    let desc = ws.desc.replace('\n', " ");
    let desc = desc.trim();
    if !desc.is_empty() {
        spans.push(Span::raw(format!("  {}", desc)));
    }
    Line::from(spans)
}

/// Draw the bottom status / help line.
fn draw_status(frame: &mut Frame, area: Rect, app: &App) {
    let position = if app.match_count() == 0 {
        "0/0".to_string()
    } else {
        format!("{}/{}", app.selected() + 1, app.match_count())
    };
    let text = format!("{}   up/down move   enter jump   esc quit", position);
    frame.render_widget(Paragraph::new(text).style(Style::default()), area);
}
