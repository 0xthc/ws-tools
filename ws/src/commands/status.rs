use super::get_session_name;
use crate::git;
use crate::tmux;
use anyhow::{Context, Result};
use crossterm::{
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Layout},
    style::{Color as RatColor, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Cell as RatCell, Row, Table as RatTable},
    Terminal,
};
use std::io::stdout;

/// Show status dashboard with worktrees and sessions using ratatui
pub fn status() -> Result<()> {
    let git_root = git::get_root(None).context("Not in a git repository")?;
    let worktrees = git::list_worktrees(&git_root)?;
    let active_sessions = tmux::get_active_sessions();

    // Get repo name for session matching
    let repo_name = git_root
        .file_name()
        .context("Invalid git root")?
        .to_string_lossy()
        .to_string();

    // Build linked pairs and find orphans
    let mut all_entries: Vec<(String, String, String, bool, bool)> = Vec::new(); // (session, branch, path, is_main, has_session)
    let mut worktree_sessions: std::collections::HashSet<String> = std::collections::HashSet::new();

    for wt in &worktrees {
        if let Ok(session_name) = get_session_name(&wt.path) {
            let has_session = active_sessions.contains(&session_name);
            let is_main = wt.path == git_root;

            if has_session {
                worktree_sessions.insert(session_name.clone());
            }

            all_entries.push((
                session_name,
                wt.branch.clone(),
                wt.path.display().to_string(),
                is_main,
                has_session,
            ));
        }
    }

    // Find orphaned sessions (sessions without worktrees)
    let orphaned_sessions: Vec<String> = active_sessions
        .iter()
        .filter(|s| s.starts_with(&format!("{}-", repo_name)) && !worktree_sessions.contains(*s))
        .cloned()
        .collect();

    // Find orphaned worktrees (worktrees without sessions, excluding main)
    let orphaned_worktrees: Vec<String> = worktrees
        .iter()
        .filter(|wt| {
            if wt.path == git_root {
                return false;
            }
            if let Ok(name) = get_session_name(&wt.path) {
                !active_sessions.contains(&name)
            } else {
                false
            }
        })
        .map(|wt| wt.branch.clone())
        .collect();

    let has_orphans = !orphaned_sessions.is_empty() || !orphaned_worktrees.is_empty();

    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Draw the UI
    terminal.draw(|frame| {
        let area = frame.area();

        // Main vertical layout
        let chunks = if has_orphans {
            Layout::vertical([
                Constraint::Length(2), // Title
                Constraint::Min(5),    // Main table
                Constraint::Length(6), // Orphan tables
                Constraint::Length(2), // Tips
            ])
            .split(area)
        } else {
            Layout::vertical([
                Constraint::Length(2), // Title
                Constraint::Min(5),    // Main table
                Constraint::Length(2), // Success message
            ])
            .split(area)
        };

        // Title
        let title = Line::from(vec![
            Span::styled(
                format!(" {} ", repo_name),
                Style::default()
                    .fg(RatColor::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("Workspace Status"),
        ]);
        frame.render_widget(ratatui::widgets::Paragraph::new(title), chunks[0]);

        // Main table
        let header = Row::new(vec![
            RatCell::from("").style(Style::default()),
            RatCell::from("Session").style(Style::default().fg(RatColor::Cyan)),
            RatCell::from("Branch").style(Style::default().fg(RatColor::Cyan)),
            RatCell::from("Path").style(Style::default().fg(RatColor::Cyan)),
        ])
        .height(1);

        let rows: Vec<Row> = all_entries
            .iter()
            .map(|(session, branch, path, is_main, has_session)| {
                let status = if *has_session { "●" } else { "○" };
                let status_style = if *has_session {
                    Style::default().fg(RatColor::Green)
                } else {
                    Style::default().fg(RatColor::Yellow)
                };
                let main_marker = if *is_main { " (main)" } else { "" };
                let dim_style = Style::default().fg(RatColor::DarkGray);

                Row::new(vec![
                    RatCell::from(status).style(status_style),
                    RatCell::from(session.as_str()).style(if *has_session {
                        Style::default()
                    } else {
                        dim_style
                    }),
                    RatCell::from(format!("{}{}", branch, main_marker)),
                    RatCell::from(path.as_str()).style(dim_style),
                ])
            })
            .collect();

        let table = RatTable::new(
            rows,
            [
                Constraint::Length(2),
                Constraint::Percentage(30),
                Constraint::Percentage(25),
                Constraint::Percentage(45),
            ],
        )
        .header(header)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Worktrees & Sessions ")
                .border_style(Style::default().fg(RatColor::DarkGray)),
        );
        frame.render_widget(table, chunks[1]);

        if has_orphans {
            // Split bottom area for two orphan tables
            let orphan_chunks =
                Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)])
                    .split(chunks[2]);

            // Orphaned sessions table
            let session_rows: Vec<Row> = if orphaned_sessions.is_empty() {
                vec![Row::new(vec![
                    RatCell::from("None").style(Style::default().fg(RatColor::DarkGray))
                ])]
            } else {
                orphaned_sessions
                    .iter()
                    .map(|s| Row::new(vec![RatCell::from(s.as_str())]))
                    .collect()
            };
            let sessions_table = RatTable::new(session_rows, [Constraint::Percentage(100)]).block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(Span::styled(
                        " Orphaned Sessions ",
                        Style::default().fg(RatColor::Red),
                    ))
                    .border_style(Style::default().fg(RatColor::DarkGray)),
            );
            frame.render_widget(sessions_table, orphan_chunks[0]);

            // Orphaned worktrees table
            let worktree_rows: Vec<Row> = if orphaned_worktrees.is_empty() {
                vec![Row::new(vec![
                    RatCell::from("None").style(Style::default().fg(RatColor::DarkGray))
                ])]
            } else {
                orphaned_worktrees
                    .iter()
                    .map(|b| Row::new(vec![RatCell::from(b.as_str())]))
                    .collect()
            };
            let worktrees_table = RatTable::new(worktree_rows, [Constraint::Percentage(100)])
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title(Span::styled(
                            " Orphaned Worktrees ",
                            Style::default().fg(RatColor::Yellow),
                        ))
                        .border_style(Style::default().fg(RatColor::DarkGray)),
                );
            frame.render_widget(worktrees_table, orphan_chunks[1]);

            // Tips
            let tips = ratatui::widgets::Paragraph::new(Line::from(vec![
                Span::styled(" Tip: ", Style::default().fg(RatColor::DarkGray)),
                Span::styled("ws sync --create", Style::default().fg(RatColor::Cyan)),
                Span::raw(" to create sessions  "),
                Span::styled("ws sync --delete", Style::default().fg(RatColor::Cyan)),
                Span::raw(" to delete worktrees  "),
                Span::styled(
                    "q",
                    Style::default()
                        .fg(RatColor::Cyan)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(" to quit"),
            ]));
            frame.render_widget(tips, chunks[3]);
        } else {
            // Success message
            let success = ratatui::widgets::Paragraph::new(Line::from(vec![
                Span::styled(" ✓ ", Style::default().fg(RatColor::Green)),
                Span::raw("All worktrees and sessions are in sync  "),
                Span::styled(
                    "q",
                    Style::default()
                        .fg(RatColor::Cyan)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(" to quit"),
            ]));
            frame.render_widget(success, chunks[2]);
        }
    })?;

    // Wait for 'q' to quit
    loop {
        if crossterm::event::poll(std::time::Duration::from_millis(100))? {
            if let crossterm::event::Event::Key(key) = crossterm::event::read()? {
                if key.code == crossterm::event::KeyCode::Char('q')
                    || key.code == crossterm::event::KeyCode::Esc
                {
                    break;
                }
            }
        }
    }

    // Restore terminal
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;

    Ok(())
}
