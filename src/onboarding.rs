use crate::config::{AiTool, Config};
use crate::git;
use anyhow::Result;
use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph},
    Frame, Terminal,
};
use std::io::stdout;
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// ASCII art frames for animation
const ASCII_FRAMES: &[&str] = &[
    r#"
       . : : : : : : .
    . : : : : : : : : : .
  . : : : : : : : : : : : : .
  . : | | - - - - - - - - - | : .
. : | - - = = = = = = = = - - | : .
. | - - = = = = = = = = = = - - | .
. | - = = = = + + + + + + + + = = - | .
. | | - = = = = + + + + + + + + + = = - | | .
. | | - - = = = = + + + + + + + + + = = = - - | | .
  . | | - - = = = = + + + + + + + + + + + + = = = - - | | .
  . | | - - - = = = = = = = + + + + + + + + + = = = - - | | .
    . | | - - - - = = = = = = = = = + + + + + + = = - - | | .
    . : | | - - - - - - = = = = = = = = + + + + = = - - | | : .
      . : | | - - - - - - - - - - = = = = = = = = - - | | : .
        . : : | | | | | | : - - - - - - - - - - : | | : .
          . . . : : : : : : : : : : - - : : : : . .
              . . . . . : : : : : . . .
                    . . . . . .
                      . . . .
"#,
    r#"
         . . . . . . . .
      . . . : : : : : . . . .
    . . : : : : : : : : : : . .
    . : | | - - - - - - - - | : .
  . : | - = = = = = = = = = - | : .
  . | - = = = = = = = = = = = - | .
. | - = = = + + + + + + + + + = = - | .
. | | - = = = + + + + + + + + + = = - | | .
  . | | - = = = = + + + + + + + + + = = = - | | .
  . | | - - = = = = + + + + + + + + + + + = = - - | | .
    . | | - - = = = = = = + + + + + + + + = = = - | | .
    . : | | - - - = = = = = = = = + + + + + = = - | | : .
      . : | | - - - - = = = = = = = = + + + = = - | | : .
        . : | | - - - - - - = = = = = = = = - - | | : .
          . : : | | | | | - - - - - - - - - | | : : .
            . . . : : : : : : : - - : : : : . . .
                . . . . . : : : : : . . .
                      . . . . . . .
                        . . . .
"#,
    r#"
           . . . . . . .
        . . : : : : : : . .
      . : : : : : : : : : : .
    . : | | - - - - - - - | : .
    . | - = = = = = = = = - | .
  . | - = = = = = = = = = = - | .
  . | - = = + + + + + + + + = = - | .
  . | | - = = + + + + + + + + = = - | | .
    . | | - = = = + + + + + + + + = = - | | .
    . | | - - = = = + + + + + + + + + = = - | | .
    . : | | - - = = = = + + + + + + + + = = - | | : .
      . : | | - - = = = = = = + + + + + = = - | | : .
        . : | | - - - = = = = = = + + + = = - | | : .
          . : | | - - - - = = = = = = - - | | : .
            . : : | | | | - - - - - - | | : : .
              . . . : : : : : : : : : . . .
                  . . . . . : : . . . .
                        . . . .
"#,
];

/// Current screen/mode
#[derive(PartialEq)]
enum Screen {
    ToolSelection,
    PathSelection,
}

/// Onboarding application state
struct OnboardingApp {
    tools: Vec<AiTool>,
    list_state: ListState,
    frame_index: usize,
    last_frame_time: Instant,
    should_exit: bool,
    selected_tool: Option<AiTool>,
    screen: Screen,
    path_input: String,
    cursor_position: usize,
    selected_path: Option<PathBuf>,
    in_git_repo: bool,
}

impl OnboardingApp {
    fn new() -> Self {
        let mut list_state = ListState::default();
        list_state.select(Some(0));

        // Check if we're in a git repo
        let in_git_repo = git::get_root(None).is_ok();

        // Default path to home directory
        let default_path = dirs::home_dir()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|| "~".to_string());

        Self {
            tools: AiTool::all().to_vec(),
            list_state,
            frame_index: 0,
            last_frame_time: Instant::now(),
            should_exit: false,
            selected_tool: None,
            screen: Screen::ToolSelection,
            path_input: default_path.clone(),
            cursor_position: default_path.len(),
            selected_path: None,
            in_git_repo,
        }
    }

    fn next_tool(&mut self) {
        let i = match self.list_state.selected() {
            Some(i) => {
                if i >= self.tools.len() - 1 {
                    0
                } else {
                    i + 1
                }
            }
            None => 0,
        };
        self.list_state.select(Some(i));
    }

    fn previous_tool(&mut self) {
        let i = match self.list_state.selected() {
            Some(i) => {
                if i == 0 {
                    self.tools.len() - 1
                } else {
                    i - 1
                }
            }
            None => 0,
        };
        self.list_state.select(Some(i));
    }

    fn select_current_tool(&mut self) {
        if let Some(i) = self.list_state.selected() {
            self.selected_tool = Some(self.tools[i]);

            // If not in a git repo, show path selection modal
            if !self.in_git_repo {
                self.screen = Screen::PathSelection;
            } else {
                self.should_exit = true;
            }
        }
    }

    fn confirm_path(&mut self) {
        let path = PathBuf::from(&self.path_input);
        if path.exists() && path.is_dir() {
            self.selected_path = Some(path);
            self.should_exit = true;
        }
    }

    fn update_animation(&mut self) {
        if self.last_frame_time.elapsed() >= Duration::from_millis(300) {
            self.frame_index = (self.frame_index + 1) % ASCII_FRAMES.len();
            self.last_frame_time = Instant::now();
        }
    }

    fn handle_key(&mut self, key: KeyCode) {
        match self.screen {
            Screen::ToolSelection => match key {
                KeyCode::Up | KeyCode::Char('k') => self.previous_tool(),
                KeyCode::Down | KeyCode::Char('j') => self.next_tool(),
                KeyCode::Enter | KeyCode::Char(' ') => self.select_current_tool(),
                KeyCode::Esc | KeyCode::Char('q') => self.should_exit = true,
                _ => {}
            },
            Screen::PathSelection => match key {
                KeyCode::Enter => self.confirm_path(),
                KeyCode::Esc => {
                    // Go back to tool selection
                    self.screen = Screen::ToolSelection;
                    self.selected_tool = None;
                }
                KeyCode::Backspace => {
                    if self.cursor_position > 0 {
                        self.path_input.remove(self.cursor_position - 1);
                        self.cursor_position -= 1;
                    }
                }
                KeyCode::Delete => {
                    if self.cursor_position < self.path_input.len() {
                        self.path_input.remove(self.cursor_position);
                    }
                }
                KeyCode::Left => {
                    if self.cursor_position > 0 {
                        self.cursor_position -= 1;
                    }
                }
                KeyCode::Right => {
                    if self.cursor_position < self.path_input.len() {
                        self.cursor_position += 1;
                    }
                }
                KeyCode::Home => {
                    self.cursor_position = 0;
                }
                KeyCode::End => {
                    self.cursor_position = self.path_input.len();
                }
                KeyCode::Char(c) => {
                    self.path_input.insert(self.cursor_position, c);
                    self.cursor_position += 1;
                }
                _ => {}
            },
        }
    }
}

fn draw_ui(frame: &mut Frame, app: &mut OnboardingApp) {
    let area = frame.area();

    // Main horizontal layout: ASCII logo (left) | Content (right)
    let main_layout = Layout::horizontal([
        Constraint::Percentage(45), // ASCII art
        Constraint::Percentage(55), // Content
    ])
    .split(area);

    // Draw ASCII animation - centered vertically
    let ascii_text = ASCII_FRAMES[app.frame_index];
    let ascii_lines: Vec<Line> = ascii_text
        .lines()
        .filter(|line| !line.is_empty())
        .map(|line| {
            Line::from(Span::styled(
                line.to_string(),
                Style::default().fg(Color::Green),
            ))
        })
        .collect();

    let ascii_height = ascii_lines.len() as u16;
    let available_height = main_layout[0].height;
    let vertical_padding = available_height.saturating_sub(ascii_height) / 2;

    // Create a centered area for the ASCII art
    let ascii_area = Rect {
        x: main_layout[0].x,
        y: main_layout[0].y + vertical_padding,
        width: main_layout[0].width,
        height: ascii_height.min(available_height),
    };

    let ascii_widget = Paragraph::new(ascii_lines).alignment(Alignment::Center);
    frame.render_widget(ascii_widget, ascii_area);

    // Right side: wrap everything in a bordered block
    let right_block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray))
        .title(" Setup ");

    let inner_area = right_block.inner(main_layout[1]);
    frame.render_widget(right_block, main_layout[1]);

    // Content layout inside the block
    let content_layout = Layout::vertical([
        Constraint::Length(2), // Title spacing
        Constraint::Length(2), // Welcome message
        Constraint::Length(1), // Subtitle
        Constraint::Length(2), // Spacing
        Constraint::Length(2), // Instructions
        Constraint::Min(8),    // Tool list
        Constraint::Length(2), // Footer
    ])
    .split(inner_area);

    // Welcome message
    let welcome = Paragraph::new(Line::from(vec![
        Span::styled("Welcome to ", Style::default().fg(Color::White)),
        Span::styled(
            "ws",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
    ]))
    .alignment(Alignment::Center);
    frame.render_widget(welcome, content_layout[1]);

    // Subtitle
    let subtitle = Paragraph::new(Line::from(Span::styled(
        "Workspace CLI for git worktrees with tmux",
        Style::default().fg(Color::DarkGray),
    )))
    .alignment(Alignment::Center);
    frame.render_widget(subtitle, content_layout[2]);

    // Instructions
    let instructions = Paragraph::new(vec![Line::from(Span::styled(
        "Select your preferred AI coding assistant:",
        Style::default().fg(Color::Yellow),
    ))])
    .alignment(Alignment::Center);
    frame.render_widget(instructions, content_layout[4]);

    // Tool list
    let items: Vec<ListItem> = app
        .tools
        .iter()
        .map(|tool| {
            let installed = which::which(tool.binary()).is_ok();
            let status = if installed {
                Span::styled(" [installed] ", Style::default().fg(Color::Green))
            } else {
                Span::styled(" [not found] ", Style::default().fg(Color::Red))
            };

            ListItem::new(Line::from(vec![
                Span::styled(
                    format!(" {} ", tool.name()),
                    Style::default().fg(Color::White),
                ),
                status,
            ]))
        })
        .collect();

    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::DarkGray))
                .title(" AI Tool "),
        )
        .highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("> ");

    frame.render_stateful_widget(list, content_layout[5], &mut app.list_state);

    // Footer with keybindings
    let footer = Paragraph::new(Line::from(vec![
        Span::styled("j/k", Style::default().fg(Color::Cyan)),
        Span::raw(" navigate  "),
        Span::styled("Enter", Style::default().fg(Color::Cyan)),
        Span::raw(" select  "),
        Span::styled("q", Style::default().fg(Color::Cyan)),
        Span::raw(" quit"),
    ]))
    .alignment(Alignment::Center);
    frame.render_widget(footer, content_layout[6]);

    // Draw path selection modal if active
    if app.screen == Screen::PathSelection {
        draw_path_modal(frame, app);
    }
}

fn draw_path_modal(frame: &mut Frame, app: &OnboardingApp) {
    let area = frame.area();

    // Calculate centered modal area (60% width, 9 lines height)
    let modal_width = (area.width as f32 * 0.6) as u16;
    let modal_height = 9;
    let modal_x = (area.width - modal_width) / 2;
    let modal_y = (area.height - modal_height) / 2;

    let modal_area = Rect {
        x: modal_x,
        y: modal_y,
        width: modal_width,
        height: modal_height,
    };

    // Clear the area behind the modal
    frame.render_widget(Clear, modal_area);

    // Modal block
    let modal_block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan))
        .title(" Select Workspace Folder ")
        .style(Style::default().bg(Color::Black));

    let inner_area = modal_block.inner(modal_area);
    frame.render_widget(modal_block, modal_area);

    // Modal content layout
    let modal_layout = Layout::vertical([
        Constraint::Length(2), // Message
        Constraint::Length(1), // Spacing
        Constraint::Length(3), // Input field
        Constraint::Length(1), // Footer
    ])
    .split(inner_area);

    // Message
    let message = Paragraph::new(Line::from(Span::styled(
        "Not in a git repository. Enter a path to open:",
        Style::default().fg(Color::Yellow),
    )))
    .alignment(Alignment::Center);
    frame.render_widget(message, modal_layout[0]);

    // Path input with cursor
    let path_display = if app.cursor_position < app.path_input.len() {
        let (before, after) = app.path_input.split_at(app.cursor_position);
        let (cursor_char, rest) = after.split_at(1);
        Line::from(vec![
            Span::raw(before),
            Span::styled(
                cursor_char,
                Style::default()
                    .bg(Color::White)
                    .fg(Color::Black)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(rest),
        ])
    } else {
        Line::from(vec![
            Span::raw(&app.path_input),
            Span::styled(
                " ",
                Style::default()
                    .bg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
        ])
    };

    let input = Paragraph::new(path_display).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray))
            .title(" Path "),
    );
    frame.render_widget(input, modal_layout[2]);

    // Validate path and show status
    let path = PathBuf::from(&app.path_input);
    let (status_text, status_color) = if path.exists() && path.is_dir() {
        ("Valid directory", Color::Green)
    } else if path.exists() {
        ("Not a directory", Color::Red)
    } else {
        ("Path does not exist", Color::Red)
    };

    // Footer with status and keybindings
    let footer = Paragraph::new(Line::from(vec![
        Span::styled(status_text, Style::default().fg(status_color)),
        Span::raw("  "),
        Span::styled("Enter", Style::default().fg(Color::Cyan)),
        Span::raw(" confirm  "),
        Span::styled("Esc", Style::default().fg(Color::Cyan)),
        Span::raw(" back"),
    ]))
    .alignment(Alignment::Center);
    frame.render_widget(footer, modal_layout[3]);
}

/// Onboarding result
pub struct OnboardingResult {
    pub tool: AiTool,
    pub path: Option<PathBuf>,
}

/// Run the onboarding TUI and return the selected tool and optional path
pub fn run_onboarding() -> Result<Option<OnboardingResult>> {
    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = OnboardingApp::new();

    // Main loop
    while !app.should_exit {
        // Update animation
        app.update_animation();

        // Draw
        terminal.draw(|frame| draw_ui(frame, &mut app))?;

        // Handle events with timeout for animation
        if event::poll(Duration::from_millis(50))? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    app.handle_key(key.code);
                }
            }
        }
    }

    // Restore terminal
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;

    Ok(app.selected_tool.map(|tool| OnboardingResult {
        tool,
        path: app.selected_path,
    }))
}

/// Check if onboarding is needed and run it
/// Returns Some(path) if a path was selected, None otherwise
pub fn check_and_run_onboarding() -> Result<Option<PathBuf>> {
    let config_path = Config::path()?;

    // If config file exists, no onboarding needed
    if config_path.exists() {
        return Ok(None);
    }

    // Run onboarding
    if let Some(result) = run_onboarding()? {
        let config = Config {
            ai_tool: result.tool,
        };
        config.save()?;

        println!();
        println!(
            "Configuration saved! Using {} as your AI tool.",
            result.tool.name()
        );
        println!();

        return Ok(result.path);
    }

    Ok(None)
}
