use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span},
    widgets::Widget,
};

use crate::tui::App;

/// Render a single line at (area.x, y) if y < area.y + area.height.
fn render_at(line: Line, area: Rect, y: u16, buf: &mut Buffer) {
    if y >= area.y + area.height {
        return;
    }
    line.render(
        Rect {
            x: area.x,
            y,
            width: area.width,
            height: 1,
        },
        buf,
    );
}

/// A widget that renders the preview pane, delegating to either the file view
/// (syntax-highlighted source) or the diff view based on `app.preview_mode`.
pub struct PreviewPane<'a> {
    pub app: &'a App,
    pub file_content: Option<(&'a str, &'a str)>, // (content, filename)
    pub highlighter: Option<&'a crate::tui::syntax::Highlighter>,
    pub changed_lines: &'a std::collections::HashSet<usize>,
}

impl<'a> PreviewPane<'a> {
    pub fn new(
        app: &'a App,
        file_content: Option<(&'a str, &'a str)>,
        highlighter: Option<&'a crate::tui::syntax::Highlighter>,
        changed_lines: &'a std::collections::HashSet<usize>,
    ) -> Self {
        Self {
            app,
            file_content,
            highlighter,
            changed_lines,
        }
    }
}

impl PreviewPane<'_> {
    /// Render the existing diff view (unified diff with colored +/- lines).
    fn render_diff(&self, area: Rect, buf: &mut Buffer) {
        let edit = match self.app.current_edit() {
            Some(e) => e,
            None => {
                render_unselected_state(area, buf, self.app);
                return;
            }
        };

        let t = &self.app.theme;
        let color_header_value: Color = t.fg;
        let color_intent: Color = t.accent_warm;
        let color_add: Color = t.accent_green;
        let color_remove: Color = t.accent_red;
        let color_hunk: Color = t.accent_blue;

        let max_y = area.y + area.height;
        let mut row = area.y;

        // Header: merged format with filename, diff label, and line counts
        render_at(
            Line::from(vec![
                Span::styled(" ", Style::default()),
                Span::styled(
                    edit.file.clone(),
                    Style::default()
                        .fg(color_header_value)
                        .add_modifier(ratatui::style::Modifier::BOLD),
                ),
                Span::styled(" \u{2502} ", Style::default().fg(t.separator)),
                Span::styled("diff", Style::default().fg(t.accent_warm)),
                Span::styled(" \u{2502} ", Style::default().fg(t.separator)),
                Span::styled(
                    format!("+{}", edit.lines_added),
                    Style::default().fg(color_add),
                ),
                Span::styled(" ", Style::default()),
                Span::styled(
                    format!("-{}", edit.lines_removed),
                    Style::default().fg(color_remove),
                ),
            ]),
            area,
            row,
            buf,
        );
        row += 1;

        // Intent line (if available).
        if row < max_y {
            if let Some(intent) = &edit.intent {
                render_at(
                    Line::from(vec![
                        Span::styled("intent: ", Style::default().fg(color_intent)),
                        Span::styled(intent.clone(), Style::default().fg(color_intent)),
                    ]),
                    area,
                    row,
                    buf,
                );
                row += 1;
            }
        }

        // Diff lines with line numbers.
        let mut old_line: usize = 0;
        let mut new_line: usize = 0;

        for diff_line in edit.patch.lines() {
            if row >= max_y {
                break;
            }

            if diff_line.starts_with("@@") {
                if let Some(minus_pos) = diff_line.find('-') {
                    let after_minus = &diff_line[minus_pos + 1..];
                    let num_str: String = after_minus
                        .chars()
                        .take_while(|c| c.is_ascii_digit())
                        .collect();
                    old_line = num_str.parse::<usize>().unwrap_or(0);
                }
                if let Some(plus_pos) = diff_line.find('+') {
                    let after_plus = &diff_line[plus_pos + 1..];
                    let num_str: String = after_plus
                        .chars()
                        .take_while(|c| c.is_ascii_digit())
                        .collect();
                    new_line = num_str.parse::<usize>().unwrap_or(0);
                }

                render_at(
                    Line::from(vec![
                        Span::styled(format!("{:>5} ", ""), Style::default().fg(t.fg_dim)),
                        Span::styled(diff_line.to_string(), Style::default().fg(color_hunk)),
                    ]),
                    area,
                    row,
                    buf,
                );
                row += 1;
                continue;
            }

            let (gutter, color) = if diff_line.starts_with('+') {
                let g = format!("{:>5} ", new_line);
                new_line += 1;
                (g, color_add)
            } else if diff_line.starts_with('-') {
                let g = format!("{:>5} ", old_line);
                old_line += 1;
                (g, color_remove)
            } else {
                let g = format!("{:>5} ", new_line);
                old_line += 1;
                new_line += 1;
                (g, Color::Reset)
            };

            render_at(
                Line::from(vec![
                    Span::styled(gutter, Style::default().fg(t.fg_dim)),
                    Span::styled(diff_line.to_string(), Style::default().fg(color)),
                ]),
                area,
                row,
                buf,
            );
            row += 1;
        }
    }
}

impl Widget for PreviewPane<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height == 0 {
            return;
        }

        if self.app.current_edit().is_none() {
            render_unselected_state(area, buf, self.app);
            return;
        }

        match self.app.preview_mode {
            crate::tui::app::PreviewMode::File => {
                if let (Some((content, filename)), Some(highlighter)) =
                    (self.file_content, self.highlighter)
                {
                    super::file_view::FileView::new(
                        self.app,
                        content,
                        filename,
                        highlighter,
                        self.changed_lines,
                    )
                    .render(area, buf);
                } else if let Some(edit) = self.app.current_edit() {
                    // The frame exists but its snapshot could not be loaded
                    // (missing or unreadable) -- say so instead of pretending
                    // no edits were ever recorded.
                    render_centered_state(
                        area,
                        buf,
                        &self.app.theme,
                        &format!("snapshot unavailable for frame #{}", edit.id),
                        &edit.file,
                        "",
                    );
                } else {
                    render_unselected_state(area, buf, self.app);
                }
            }
            crate::tui::app::PreviewMode::Diff => {
                self.render_diff(area, buf);
            }
        }
    }
}

/// Render the empty/welcome state when no edits have been tracked yet.
fn render_empty_state(area: Rect, buf: &mut Buffer, theme: &crate::theme::Theme) {
    render_centered_state(
        area,
        buf,
        theme,
        "waiting for edits",
        "start coding in another pane",
        "aitrace will track every change automatically",
    );
}

/// Render a centered informational state: same layout as the welcome
/// screen, but with a contextual headline (edits exist but none selected,
/// snapshot unavailable, ...). One message per cause -- never claim
/// "waiting for edits" when edits exist.
fn render_centered_state(
    area: Rect,
    buf: &mut Buffer,
    theme: &crate::theme::Theme,
    headline: &str,
    sub1: &str,
    sub2: &str,
) {
    if area.height < 5 || area.width < 30 {
        return;
    }

    let logo = [
        r"       _ _",
        r"  __ _(_) |_ _ __ __ _  ___ ___",
        r" / _` | | __| '__/ _` |/ __/ _ \",
        r"| (_| | | |_| | | (_| | (_|  __/",
        r" \__,_|_|\__|_|  \__,_|\___\___|",
    ];

    let hints = [
        ("", ""),
        (headline, ""),
        ("", ""),
        (sub1, ""),
        (sub2, ""),
        ("", ""),
        ("left/right", "scrub through edits"),
        ("Space", "play / pause replay"),
        ("R", "restore to playhead"),
        ("c", "create checkpoint"),
        ("g", "toggle command view"),
        ("t", "cycle theme"),
        ("?", "all keybindings"),
    ];

    let color_warm = theme.fg_dim;
    let color_subtle = theme.fg_dim;
    let color_dim = theme.separator;
    let color_empty = theme.fg_muted;

    // Center vertically
    let total_height = logo.len() + 2 + hints.len();
    let start_y = area.y + area.height.saturating_sub(total_height as u16) / 2;

    // Render logo
    for (i, line) in logo.iter().enumerate() {
        let y = start_y + i as u16;
        if y >= area.y + area.height {
            break;
        }
        let x = area.x + area.width.saturating_sub(line.len() as u16) / 2;
        buf.set_string(x, y, *line, Style::default().fg(color_warm));
    }

    // Render hints
    let hints_start = start_y + logo.len() as u16 + 2;
    for (i, (key, desc)) in hints.iter().enumerate() {
        let y = hints_start + i as u16;
        if y >= area.y + area.height {
            break;
        }

        if key.is_empty() && desc.is_empty() {
            continue;
        }

        if desc.is_empty() {
            // It's a section label
            let x = area.x + area.width.saturating_sub(key.len() as u16) / 2;
            let color = if *key == headline {
                color_subtle
            } else {
                color_dim
            };
            buf.set_string(x, y, *key, Style::default().fg(color));
        } else {
            // Key + description
            let text = format!("{:>14}  {}", key, desc);
            let x = area.x + area.width.saturating_sub(text.len() as u16) / 2;
            // Render key part brighter, desc part dimmer
            buf.set_string(
                x,
                y,
                format!("{:>14}", key),
                Style::default().fg(color_empty),
            );
            buf.set_string(x + 16, y, *desc, Style::default().fg(color_subtle));
        }
    }
}

/// Nothing is selected at the playhead: either the session truly has no
/// edits yet (welcome state) or edits exist and the user just has not
/// scrubbed to one -- very different situations, different messages.
fn render_unselected_state(area: Rect, buf: &mut Buffer, app: &App) {
    if app.edits.is_empty() {
        render_empty_state(area, buf, &app.theme);
    } else {
        render_centered_state(
            area,
            buf,
            &app.theme,
            &format!("{} edits recorded", app.edits.len()),
            "press Right to select a frame",
            "",
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{EditEvent, EditKind};
    use crate::tui::app::PreviewMode;

    fn sample_edit() -> EditEvent {
        EditEvent {
            id: 7,
            ts: 0,
            file: "src/ghost.rs".to_string(),
            kind: EditKind::Modify,
            patch: String::new(),
            before_hash: None,
            after_hash: "h".to_string(),
            intent: None,
            tool: None,
            lines_added: 1,
            lines_removed: 0,
            agent_id: None,
            agent_label: None,
            operation_id: None,
            operation_intent: None,
            tool_name: None,
            restore_id: None,
        }
    }

    fn render_to_string(app: &App, file_content: Option<(&str, &str)>) -> String {
        let area = Rect::new(0, 0, 100, 30);
        let mut buf = Buffer::empty(area);
        let changed = std::collections::HashSet::new();
        PreviewPane::new(app, file_content, None, &changed).render(area, &mut buf);
        buf.content.iter().map(|c| c.symbol()).collect()
    }

    #[test]
    fn snapshot_failure_is_not_waiting_for_edits() {
        // Regression: a present frame whose snapshot cannot be loaded used
        // to render the "waiting for edits" welcome screen.
        let mut app = App::new();
        app.push_edit(sample_edit());
        app.playhead = 0;
        app.preview_mode = PreviewMode::File;

        let text = render_to_string(&app, None);
        assert!(
            text.contains("snapshot unavailable for frame #7"),
            "got: {text}"
        );
        assert!(text.contains("src/ghost.rs"));
        assert!(!text.contains("waiting for edits"));
    }

    #[test]
    fn edits_without_selection_offer_scrub_not_waiting() {
        let mut app = App::new();
        app.push_edit(sample_edit());
        app.playhead = 1; // past the end: nothing selected
        app.preview_mode = PreviewMode::File;

        let text = render_to_string(&app, None);
        assert!(text.contains("1 edits recorded"), "got: {text}");
        assert!(text.contains("press Right"));
        assert!(!text.contains("waiting for edits"));
    }

    #[test]
    fn truly_empty_session_still_shows_welcome() {
        let app = App::new();
        let text = render_to_string(&app, None);
        assert!(text.contains("waiting for edits"));
    }
}
