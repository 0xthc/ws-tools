use crate::config::{AiTool, Config};
use anyhow::{Context, Result};
use colored::*;
use crossterm::{
    cursor::{MoveTo, Show},
    event::{self, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{
        disable_raw_mode, enable_raw_mode, Clear as TermClear, ClearType, EnterAlternateScreen,
        LeaveAlternateScreen,
    },
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Layout},
    style::{Color as RatColor, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, ListState},
    Frame, Terminal,
};
use std::io::{stdout, Write};
use std::process::Command;

/// Switch AI tool in current tmux session
pub fn ai(tool_name: Option<String>) -> Result<()> {
    // Get current config
    let mut cfg = Config::load()?;

    // Determine which tool to use
    let tool = if let Some(name) = tool_name {
        AiTool::from_str(&name).context(format!(
            "Unknown AI tool: {}. Valid options: droid, claude, codex, gemini, copilot",
            name
        ))?
    } else {
        // Show TUI selector
        match run_ai_selector(cfg.ai_tool)? {
            Some(t) => t,
            None => return Ok(()), // User cancelled
        }
    };

    // Update config
    cfg.ai_tool = tool;
    cfg.save()?;

    // Check if we're in a tmux session
    if std::env::var("TMUX").is_err() {
        println!("{} AI tool set to {}", "::".green().bold(), tool.name());
        println!(
            "{} Not in tmux session, config updated for next session",
            "::".yellow().bold()
        );
        return Ok(());
    }

    // Get current session name (works even in popup)
    let session_output = Command::new("tmux")
        .args(["display-message", "-p", "#{session_name}"])
        .output()
        .context("Failed to get tmux session")?;

    let session = String::from_utf8_lossy(&session_output.stdout)
        .trim()
        .to_string();

    // If we're in a popup, get the client's session instead
    let session = if session.starts_with("popup") {
        let client_output = Command::new("tmux")
            .args(["display-message", "-p", "-t", "{last}", "#{session_name}"])
            .output()
            .context("Failed to get client session")?;
        let client_session = String::from_utf8_lossy(&client_output.stdout)
            .trim()
            .to_string();
        if client_session.is_empty() || client_session.starts_with("popup") {
            session
        } else {
            client_session
        }
    } else {
        session
    };

    if session.is_empty() {
        anyhow::bail!("Could not determine current tmux session");
    }

    // The AI pane is always pane 2 in our layout (both large and small)
    let target = format!("{}:0.2", session);

    // Check if target pane exists
    let check_pane = Command::new("tmux")
        .args(["has-session", "-t", &target])
        .output();

    if check_pane.map(|o| !o.status.success()).unwrap_or(true) {
        println!("{} AI tool set to {}", "::".green().bold(), tool.name());
        println!(
            "{} Pane {} not found, config updated for next session",
            "::".yellow().bold(),
            target
        );
        return Ok(());
    }

    // Kill any running process in the pane properly
    kill_pane_processes(&target)?;

    // Small delay to let the process exit and shell reset
    std::thread::sleep(std::time::Duration::from_millis(100));

    // Clear terminal and command line, then start new tool
    Command::new("tmux")
        .args(["send-keys", "-t", &target, "C-u", "clear", "Enter"])
        .output()
        .context("Failed to clear terminal")?;

    // Small delay for clear to complete
    std::thread::sleep(std::time::Duration::from_millis(50));

    Command::new("tmux")
        .args(["send-keys", "-t", &target, tool.command(), "Enter"])
        .output()
        .context("Failed to send new command")?;

    println!(
        "{} Switched to {} in pane {}",
        "::".green().bold(),
        tool.name(),
        target
    );

    Ok(())
}

/// Kill any running process in a tmux pane
fn kill_pane_processes(target: &str) -> Result<()> {
    // Get the pane's shell PID
    let pane_pid_output = Command::new("tmux")
        .args(["display-message", "-p", "-t", target, "#{pane_pid}"])
        .output()
        .context("Failed to get pane PID")?;

    let pane_pid = String::from_utf8_lossy(&pane_pid_output.stdout)
        .trim()
        .to_string();

    if pane_pid.is_empty() {
        return Ok(());
    }

    // Find child processes of the shell
    let children_output = Command::new("pgrep").args(["-P", &pane_pid]).output();

    if let Ok(output) = children_output {
        let children = String::from_utf8_lossy(&output.stdout);
        for child_pid in children.lines() {
            let child_pid = child_pid.trim();
            if !child_pid.is_empty() {
                // Send SIGTERM first (graceful shutdown)
                let _ = Command::new("kill").args(["-TERM", child_pid]).output();
            }
        }
    }

    // Also send Ctrl+C as fallback (in case process ignores SIGTERM briefly)
    let _ = Command::new("tmux")
        .args(["send-keys", "-t", target, "C-c"])
        .output();

    Ok(())
}

/// TUI selector for AI tools
fn run_ai_selector(current: AiTool) -> Result<Option<AiTool>> {
    let tools = AiTool::all();
    let current_idx = tools.iter().position(|t| *t == current).unwrap_or(0);

    let mut list_state = ListState::default();
    list_state.select(Some(current_idx));

    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut selected: Option<AiTool> = None;

    loop {
        terminal.draw(|frame| {
            draw_ai_selector(frame, tools, &mut list_state, current);
        })?;

        if event::poll(std::time::Duration::from_millis(100))? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    match key.code {
                        KeyCode::Up | KeyCode::Char('k') => {
                            let i = list_state.selected().unwrap_or(0);
                            let new_i = if i == 0 { tools.len() - 1 } else { i - 1 };
                            list_state.select(Some(new_i));
                        }
                        KeyCode::Down | KeyCode::Char('j') => {
                            let i = list_state.selected().unwrap_or(0);
                            let new_i = if i >= tools.len() - 1 { 0 } else { i + 1 };
                            list_state.select(Some(new_i));
                        }
                        KeyCode::Enter | KeyCode::Char(' ') => {
                            if let Some(i) = list_state.selected() {
                                selected = Some(tools[i]);
                            }
                            break;
                        }
                        KeyCode::Esc | KeyCode::Char('q') => {
                            break;
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    // Restore terminal with full cleanup
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        TermClear(ClearType::All),
        MoveTo(0, 0),
        Show
    )?;
    // Flush to ensure all escape sequences are written before popup closes
    terminal.backend_mut().flush()?;

    Ok(selected)
}

fn draw_ai_selector(
    frame: &mut Frame,
    tools: &[AiTool],
    list_state: &mut ListState,
    current: AiTool,
) {
    let area = frame.area();

    // Calculate centered popup area
    let popup_width = 50.min(area.width.saturating_sub(4));
    let popup_height = (tools.len() as u16 + 6).min(area.height.saturating_sub(4));
    let popup_x = (area.width - popup_width) / 2;
    let popup_y = (area.height - popup_height) / 2;

    let popup_area = ratatui::layout::Rect {
        x: popup_x,
        y: popup_y,
        width: popup_width,
        height: popup_height,
    };

    // Clear background
    frame.render_widget(Clear, popup_area);

    // Build list items
    let items: Vec<ListItem> = tools
        .iter()
        .map(|tool| {
            let installed = which::which(tool.binary()).is_ok();
            let is_current = *tool == current;

            let marker = if is_current { "*" } else { " " };
            let status = if installed {
                Span::styled(" [installed]", Style::default().fg(RatColor::Green))
            } else {
                Span::styled(" [not found]", Style::default().fg(RatColor::Red))
            };

            ListItem::new(Line::from(vec![
                Span::raw(format!("{} ", marker)),
                Span::styled(tool.name(), Style::default().fg(RatColor::White)),
                status,
            ]))
        })
        .collect();

    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(RatColor::Cyan))
                .title(" Switch AI Tool ")
                .style(Style::default().bg(RatColor::Black)),
        )
        .highlight_style(
            Style::default()
                .bg(RatColor::DarkGray)
                .fg(RatColor::Cyan)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("> ");

    // Split popup for list and footer
    let chunks = Layout::vertical([Constraint::Min(3), Constraint::Length(2)]).split(popup_area);

    frame.render_stateful_widget(list, chunks[0], list_state);

    // Footer
    let footer = ratatui::widgets::Paragraph::new(Line::from(vec![
        Span::styled("j/k", Style::default().fg(RatColor::Cyan)),
        Span::raw(" navigate  "),
        Span::styled("Enter", Style::default().fg(RatColor::Cyan)),
        Span::raw(" select  "),
        Span::styled("q", Style::default().fg(RatColor::Cyan)),
        Span::raw(" cancel"),
    ]))
    .alignment(ratatui::layout::Alignment::Center)
    .style(Style::default().bg(RatColor::Black));
    frame.render_widget(footer, chunks[1]);
}
