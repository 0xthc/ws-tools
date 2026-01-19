use crate::config::{AiTool, Config, ExplorerTool, GitTool};
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

/// Reaction-diffusion simulation for organic plasma animation
/// Based on Gray-Scott model - simulates two chemicals that create natural patterns
struct ReactionDiffusion {
    width: usize,
    height: usize,
    u: Vec<Vec<f64>>, // Chemical U concentration
    v: Vec<Vec<f64>>, // Chemical V concentration
    // Parameters tuned for pulsing blob pattern
    du: f64,   // Diffusion rate of U
    dv: f64,   // Diffusion rate of V
    f: f64,    // Feed rate
    k: f64,    // Kill rate
    time: f64, // For oscillation
}

impl ReactionDiffusion {
    fn new(width: usize, height: usize) -> Self {
        let mut u = vec![vec![1.0; width]; height];
        let mut v = vec![vec![0.0; width]; height];

        // Seed the center with chemical V to start the reaction
        let cx = width / 2;
        let cy = height / 2;
        let radius = (width.min(height) / 6) as i32;

        for y in 0..height {
            for x in 0..width {
                let dx = x as i32 - cx as i32;
                let dy = (y as i32 - cy as i32) * 2; // Stretch vertically for terminal aspect ratio
                let dist = ((dx * dx + dy * dy) as f64).sqrt();
                if dist < radius as f64 {
                    u[y][x] = 0.5;
                    v[y][x] = 0.25;
                }
            }
        }

        Self {
            width,
            height,
            u,
            v,
            du: 0.16,  // Diffusion rate U
            dv: 0.08,  // Diffusion rate V
            f: 0.035,  // Feed rate - controls pattern type
            k: 0.065,  // Kill rate - controls pattern density
            time: 0.0,
        }
    }

    fn laplacian(grid: &[Vec<f64>], x: usize, y: usize, width: usize, height: usize) -> f64 {
        let x_prev = if x == 0 { width - 1 } else { x - 1 };
        let x_next = if x == width - 1 { 0 } else { x + 1 };
        let y_prev = if y == 0 { height - 1 } else { y - 1 };
        let y_next = if y == height - 1 { 0 } else { y + 1 };

        // 5-point stencil Laplacian
        grid[y_prev][x] + grid[y_next][x] + grid[y][x_prev] + grid[y][x_next] - 4.0 * grid[y][x]
    }

    fn step(&mut self) {
        let mut new_u = self.u.clone();
        let mut new_v = self.v.clone();

        // Oscillating parameters for pulsing effect
        self.time += 0.08;
        let pulse = (self.time.sin() * 0.5 + 0.5) * 0.01;
        let f = self.f + pulse;
        let k = self.k - pulse * 0.5;

        // Pacemaker: continuously inject chemical at center with oscillating radius
        let cx = self.width / 2;
        let cy = self.height / 2;
        let base_radius = (self.width.min(self.height) / 8) as f64;
        let breath = (self.time * 0.5).sin() * 0.4 + 0.6; // Oscillates 0.2 to 1.0
        let current_radius = base_radius * breath;

        for y in 0..self.height {
            for x in 0..self.width {
                let u = self.u[y][x];
                let v = self.v[y][x];
                let uvv = u * v * v;

                let lap_u = Self::laplacian(&self.u, x, y, self.width, self.height);
                let lap_v = Self::laplacian(&self.v, x, y, self.width, self.height);

                // Gray-Scott reaction-diffusion equations
                new_u[y][x] = u + self.du * lap_u - uvv + f * (1.0 - u);
                new_v[y][x] = v + self.dv * lap_v + uvv - (f + k) * v;

                // Pacemaker injection at center - keeps the bubble alive and pulsing
                let dx = x as f64 - cx as f64;
                let dy = (y as f64 - cy as f64) * 2.0; // Aspect ratio correction
                let dist = (dx * dx + dy * dy).sqrt();
                if dist < current_radius {
                    // Inject V chemical, reduce U - creates the active pattern
                    let strength = 1.0 - (dist / current_radius);
                    new_u[y][x] = (new_u[y][x] - 0.1 * strength).max(0.0);
                    new_v[y][x] = (new_v[y][x] + 0.1 * strength).min(1.0);
                }

                // Clamp values
                new_u[y][x] = new_u[y][x].clamp(0.0, 1.0);
                new_v[y][x] = new_v[y][x].clamp(0.0, 1.0);
            }
        }

        self.u = new_u;
        self.v = new_v;
    }

    fn render(&self) -> Vec<String> {
        // ASCII density ramp from sparse to dense
        let chars = [' ', '·', '-', '=', '+', '*', '#', '@'];

        self.v
            .iter()
            .map(|row| {
                row.iter()
                    .map(|&val| {
                        // Map V concentration to character index
                        let idx = (val * (chars.len() - 1) as f64).round() as usize;
                        chars[idx.min(chars.len() - 1)]
                    })
                    .collect()
            })
            .collect()
    }
}

/// Current screen/mode
#[derive(PartialEq, Clone)]
#[allow(clippy::enum_variant_names)]
enum Screen {
    SelectAiTool,
    SelectGitTool,
    SelectExplorer,
    SelectPath,
}

/// Onboarding application state
struct OnboardingApp {
    // AI tools
    ai_tools: Vec<AiTool>,
    ai_list_state: ListState,
    selected_ai_tool: Option<AiTool>,
    // Git tools
    git_tools: Vec<GitTool>,
    git_list_state: ListState,
    selected_git_tool: Option<GitTool>,
    // Explorer tools
    explorer_tools: Vec<ExplorerTool>,
    explorer_list_state: ListState,
    selected_explorer_tool: Option<ExplorerTool>,
    // Animation - reaction-diffusion simulation
    plasma: ReactionDiffusion,
    last_frame_time: Instant,
    // State
    should_exit: bool,
    screen: Screen,
    path_input: String,
    cursor_position: usize,
    selected_path: Option<PathBuf>,
    in_git_repo: bool,
}

impl OnboardingApp {
    fn new() -> Self {
        let mut ai_list_state = ListState::default();
        ai_list_state.select(Some(0));
        let mut git_list_state = ListState::default();
        git_list_state.select(Some(0));
        let mut explorer_list_state = ListState::default();
        explorer_list_state.select(Some(0));

        // Check if we're in a git repo
        let in_git_repo = git::get_root(None).is_ok();

        // Default path to home directory
        let default_path = dirs::home_dir()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|| "~".to_string());

        // Initialize plasma simulation - size will be adjusted on first render
        let plasma = ReactionDiffusion::new(80, 40);

        Self {
            ai_tools: AiTool::all().to_vec(),
            ai_list_state,
            selected_ai_tool: None,
            git_tools: GitTool::all().to_vec(),
            git_list_state,
            selected_git_tool: None,
            explorer_tools: ExplorerTool::all().to_vec(),
            explorer_list_state,
            selected_explorer_tool: None,
            plasma,
            last_frame_time: Instant::now(),
            should_exit: false,
            screen: Screen::SelectAiTool,
            path_input: default_path.clone(),
            cursor_position: default_path.len(),
            selected_path: None,
            in_git_repo,
        }
    }

    fn current_list_len(&self) -> usize {
        match self.screen {
            Screen::SelectAiTool => self.ai_tools.len(),
            Screen::SelectGitTool => self.git_tools.len(),
            Screen::SelectExplorer => self.explorer_tools.len(),
            Screen::SelectPath => 0,
        }
    }

    fn current_list_state(&mut self) -> &mut ListState {
        match self.screen {
            Screen::SelectAiTool => &mut self.ai_list_state,
            Screen::SelectGitTool => &mut self.git_list_state,
            Screen::SelectExplorer => &mut self.explorer_list_state,
            Screen::SelectPath => &mut self.ai_list_state, // unused
        }
    }

    fn next_item(&mut self) {
        let len = self.current_list_len();
        if len == 0 {
            return;
        }
        let state = self.current_list_state();
        let i = match state.selected() {
            Some(i) => {
                if i >= len - 1 {
                    0
                } else {
                    i + 1
                }
            }
            None => 0,
        };
        state.select(Some(i));
    }

    fn previous_item(&mut self) {
        let len = self.current_list_len();
        if len == 0 {
            return;
        }
        let state = self.current_list_state();
        let i = match state.selected() {
            Some(i) => {
                if i == 0 {
                    len - 1
                } else {
                    i - 1
                }
            }
            None => 0,
        };
        state.select(Some(i));
    }

    fn select_current(&mut self) {
        match self.screen {
            Screen::SelectAiTool => {
                if let Some(i) = self.ai_list_state.selected() {
                    self.selected_ai_tool = Some(self.ai_tools[i]);
                    self.screen = Screen::SelectGitTool;
                }
            }
            Screen::SelectGitTool => {
                if let Some(i) = self.git_list_state.selected() {
                    self.selected_git_tool = Some(self.git_tools[i].clone());
                    self.screen = Screen::SelectExplorer;
                }
            }
            Screen::SelectExplorer => {
                if let Some(i) = self.explorer_list_state.selected() {
                    self.selected_explorer_tool = Some(self.explorer_tools[i].clone());
                    if !self.in_git_repo {
                        self.screen = Screen::SelectPath;
                    } else {
                        self.should_exit = true;
                    }
                }
            }
            Screen::SelectPath => {}
        }
    }

    fn go_back(&mut self) {
        match self.screen {
            Screen::SelectAiTool => self.should_exit = true,
            Screen::SelectGitTool => {
                self.screen = Screen::SelectAiTool;
                self.selected_ai_tool = None;
            }
            Screen::SelectExplorer => {
                self.screen = Screen::SelectGitTool;
                self.selected_git_tool = None;
            }
            Screen::SelectPath => {
                self.screen = Screen::SelectExplorer;
                self.selected_explorer_tool = None;
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
        if self.last_frame_time.elapsed() >= Duration::from_millis(50) {
            // Run multiple simulation steps for smoother animation
            for _ in 0..4 {
                self.plasma.step();
            }
            self.last_frame_time = Instant::now();
        }
    }

    fn resize_plasma(&mut self, width: usize, height: usize) {
        if self.plasma.width != width || self.plasma.height != height {
            self.plasma = ReactionDiffusion::new(width, height);
        }
    }

    fn handle_key(&mut self, key: KeyCode) {
        match self.screen {
            Screen::SelectAiTool | Screen::SelectGitTool | Screen::SelectExplorer => match key {
                KeyCode::Up | KeyCode::Char('k') => self.previous_item(),
                KeyCode::Down | KeyCode::Char('j') => self.next_item(),
                KeyCode::Enter | KeyCode::Char(' ') => self.select_current(),
                KeyCode::Esc | KeyCode::Char('q') => self.go_back(),
                _ => {}
            },
            Screen::SelectPath => match key {
                KeyCode::Enter => self.confirm_path(),
                KeyCode::Esc => self.go_back(),
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

    // Resize plasma to fit the available area
    let plasma_width = main_layout[0].width as usize;
    let plasma_height = main_layout[0].height as usize;
    app.resize_plasma(plasma_width.max(10), plasma_height.max(10));

    // Render plasma simulation
    let plasma_lines = app.plasma.render();
    let ascii_lines: Vec<Line> = plasma_lines
        .iter()
        .map(|line| {
            Line::from(Span::styled(
                line.clone(),
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

    let ascii_widget = Paragraph::new(ascii_lines);
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

    // Dynamic content based on screen
    let (instruction_text, block_title, items, list_state) = match &app.screen {
        Screen::SelectAiTool => {
            let items: Vec<ListItem> = app
                .ai_tools
                .iter()
                .map(|tool| {
                    let installed = which::which(tool.binary()).is_ok();
                    let status = if installed {
                        Span::styled(" [installed]", Style::default().fg(Color::Green))
                    } else {
                        Span::styled(" [not found]", Style::default().fg(Color::DarkGray))
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
            (
                "Select your AI coding assistant:",
                " AI Tool (1/3) ",
                items,
                &mut app.ai_list_state,
            )
        }
        Screen::SelectGitTool => {
            let items: Vec<ListItem> = app
                .git_tools
                .iter()
                .map(|tool| {
                    let installed = which::which(tool.binary()).is_ok();
                    let status = if installed {
                        Span::styled(" [installed]", Style::default().fg(Color::Green))
                    } else {
                        Span::styled(" [not found]", Style::default().fg(Color::DarkGray))
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
            (
                "Select your git TUI (top-left panel):",
                " Git Tool (2/3) ",
                items,
                &mut app.git_list_state,
            )
        }
        Screen::SelectExplorer => {
            let items: Vec<ListItem> = app
                .explorer_tools
                .iter()
                .map(|tool| {
                    let installed = which::which(tool.binary()).is_ok();
                    let status = if installed {
                        Span::styled(" [installed]", Style::default().fg(Color::Green))
                    } else {
                        Span::styled(" [not found]", Style::default().fg(Color::DarkGray))
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
            (
                "Select your file explorer (bottom-left panel):",
                " Explorer (3/3) ",
                items,
                &mut app.explorer_list_state,
            )
        }
        Screen::SelectPath => {
            // Path selection is handled separately
            draw_path_modal(frame, app);
            return;
        }
    };

    // Instructions
    let instructions = Paragraph::new(vec![Line::from(Span::styled(
        instruction_text,
        Style::default().fg(Color::Yellow),
    ))])
    .alignment(Alignment::Center);
    frame.render_widget(instructions, content_layout[4]);

    // Tool list
    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::DarkGray))
                .title(block_title),
        )
        .highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("> ");

    frame.render_stateful_widget(list, content_layout[5], list_state);

    // Footer with keybindings
    let footer = Paragraph::new(Line::from(vec![
        Span::styled("j/k", Style::default().fg(Color::Cyan)),
        Span::raw(" navigate  "),
        Span::styled("Enter", Style::default().fg(Color::Cyan)),
        Span::raw(" next  "),
        Span::styled("Esc", Style::default().fg(Color::Cyan)),
        Span::raw(" back"),
    ]))
    .alignment(Alignment::Center);
    frame.render_widget(footer, content_layout[6]);
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
    pub ai_tool: AiTool,
    pub git_tool: GitTool,
    pub explorer_tool: ExplorerTool,
    pub path: Option<PathBuf>,
}

/// Run the onboarding TUI and return the selected tools and optional path
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

    // Only return result if all tools were selected
    if let (Some(ai_tool), Some(git_tool), Some(explorer_tool)) = (
        app.selected_ai_tool,
        app.selected_git_tool,
        app.selected_explorer_tool,
    ) {
        Ok(Some(OnboardingResult {
            ai_tool,
            git_tool,
            explorer_tool,
            path: app.selected_path,
        }))
    } else {
        Ok(None)
    }
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
        // Save names before moving
        let ai_name = result.ai_tool.name().to_string();
        let git_name = result.git_tool.name().to_string();
        let explorer_name = result.explorer_tool.name().to_string();

        let config = Config {
            ai_tool: result.ai_tool,
            git_tool: result.git_tool,
            explorer_tool: result.explorer_tool,
        };
        config.save()?;

        println!();
        println!(
            "Configuration saved! AI: {}, Git: {}, Explorer: {}",
            ai_name, git_name, explorer_name
        );
        println!();

        return Ok(result.path);
    }

    Ok(None)
}
