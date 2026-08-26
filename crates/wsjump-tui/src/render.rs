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

/// Draw the candidate list, highlighting the selected row. When there are no
/// matches, show a hint instead.
fn draw_list(frame: &mut Frame, area: Rect, app: &App) {
    if app.loaded_count() == 0 {
        frame.render_widget(
            Paragraph::new("no matching workspace")
                .style(Style::default().add_modifier(Modifier::DIM)),
            area,
        );
        return;
    }

    let hscroll = app.hscroll();
    let items: Vec<ListItem> = app
        .results()
        .iter()
        .map(|ws| ListItem::new(item_line(ws, hscroll)))
        .collect();

    let list = List::new(items).highlight_style(
        Style::default()
            .add_modifier(Modifier::REVERSED)
            .add_modifier(Modifier::BOLD),
    );

    let mut state = ListState::default();
    state.select(Some(app.selected()));
    frame.render_stateful_widget(list, area, &mut state);
}

/// Build a single-line display for one workspace: id + tags + desc, shifted
/// left by `hscroll` characters so truncated content can be revealed.
fn item_line(ws: &Workspace, hscroll: usize) -> Line<'static> {
    let mut text = ws.id.clone();
    if !ws.tags.is_empty() {
        let tags = ws
            .tags
            .iter()
            .map(|t| format!(":{}", t))
            .collect::<Vec<_>>()
            .join(" ");
        text.push_str(&format!("  {}", tags));
    }
    let desc = ws.desc.replace('\n', " ");
    let desc = desc.trim();
    if !desc.is_empty() {
        text.push_str(&format!("  {}", desc));
    }

    // Apply horizontal scroll by dropping the first `hscroll` characters.
    let shifted: String = text.chars().skip(hscroll).collect();
    Line::from(Span::raw(shifted))
}

/// Draw the bottom status / help line.
fn draw_status(frame: &mut Frame, area: Rect, app: &App) {
    let text = format!(
        "{}   {}   up/down move   shift+left/right scroll{}   enter jump   esc quit",
        position(app),
        app.sort().as_str(),
        // Only advertise the toggle when it can actually do something.
        if app.can_sort_by_recency() {
            "   ctrl+s sort"
        } else {
            ""
        },
    );
    frame.render_widget(Paragraph::new(text).style(Style::default()), area);
}

/// The "which row of how many" indicator.
///
/// The total is how many results have been fetched, not how many match: knowing
/// the latter would need a second query counting the whole result set, which is
/// exactly what paging avoids. A '+' marks that more exist, so the number is
/// never mistaken for a total.
fn position(app: &App) -> String {
    if app.loaded_count() == 0 {
        return "0/0".to_string();
    }
    format!(
        "{}/{}{}",
        app.selected() + 1,
        app.loaded_count(),
        if app.has_more() { "+" } else { "" }
    )
}
