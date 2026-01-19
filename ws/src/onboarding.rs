use crate::config::{AiTool, Config, ExplorerTool, GitTool};
use crate::git;
use crate::tmux;
use anyhow::{Context, Result};
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
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph},
    Frame, Terminal,
};
use std::io::stdout;
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// Workspace metrics used to create unique plasma patterns
#[derive(Clone)]
pub struct WorkspaceMetrics {
    pub repo_name: String,
    pub num_worktrees: usize,
    pub num_commits: usize,
    pub num_branches: usize,
    pub active_sessions: usize,
}

impl Default for WorkspaceMetrics {
    fn default() -> Self {
        Self {
            repo_name: "default".to_string(),
            num_worktrees: 1,
            num_commits: 100,
            num_branches: 1,
            active_sessions: 1,
        }
    }
}

impl WorkspaceMetrics {
    /// Create metrics from current git repository
    pub fn from_current_repo() -> Self {
        let mut metrics = Self::default();

        // Get repo name
        if let Ok(root) = git::get_root(None) {
            if let Some(name) = root.file_name() {
                metrics.repo_name = name.to_string_lossy().to_string();
            }

            // Get worktrees
            if let Ok(worktrees) = git::list_worktrees(&root) {
                metrics.num_worktrees = worktrees.len();
            }

            // Get commit count (approximate, last 1000)
            if let Ok(output) = std::process::Command::new("git")
                .args(["rev-list", "--count", "HEAD"])
                .current_dir(&root)
                .output()
            {
                if output.status.success() {
                    if let Ok(count) = String::from_utf8_lossy(&output.stdout).trim().parse() {
                        metrics.num_commits = count;
                    }
                }
            }

            // Get branch count
            if let Ok(output) = std::process::Command::new("git")
                .args(["branch", "-a", "--list"])
                .current_dir(&root)
                .output()
            {
                if output.status.success() {
                    metrics.num_branches = String::from_utf8_lossy(&output.stdout).lines().count();
                }
            }
        }

        // Get active tmux sessions for this repo
        let sessions = tmux::get_active_sessions();
        metrics.active_sessions = sessions
            .iter()
            .filter(|s| s.starts_with(&format!("{}-", metrics.repo_name)))
            .count()
            .max(1);

        metrics
    }

    /// Generate a hash from repo name for consistent randomization
    fn name_hash(&self) -> u64 {
        self.repo_name
            .bytes()
            .fold(0u64, |acc, b| acc.wrapping_mul(31).wrapping_add(b as u64))
    }
}

/// Reaction-diffusion simulation for organic plasma animation
/// Based on Gray-Scott model - simulates two chemicals that create natural patterns
/// Parameters are derived from workspace metrics to create unique patterns per repo
struct ReactionDiffusion {
    width: usize,
    height: usize,
    u: Vec<Vec<f64>>, // Chemical U concentration
    v: Vec<Vec<f64>>, // Chemical V concentration
    // Parameters derived from workspace metrics
    du: f64,          // Diffusion rate of U
    dv: f64,          // Diffusion rate of V
    f: f64,           // Feed rate
    k: f64,           // Kill rate
    time: f64,        // For oscillation
    pulse_speed: f64, // How fast the pattern pulses
    num_seeds: usize, // Number of seed points (nucleation centers)
    seed_positions: Vec<(usize, usize)>, // Positions of seed points
    metrics: WorkspaceMetrics,
}

impl ReactionDiffusion {
    fn new(width: usize, height: usize) -> Self {
        Self::with_metrics(width, height, WorkspaceMetrics::default())
    }

    fn with_metrics(width: usize, height: usize, metrics: WorkspaceMetrics) -> Self {
        let hash = metrics.name_hash();

        // Derive parameters from metrics
        // Feed rate: 0.030-0.050 based on repo name hash
        let f = 0.030 + ((hash % 20) as f64) * 0.001;
        // Kill rate: 0.057-0.072 based on hash
        let k = 0.057 + (((hash >> 8) % 15) as f64) * 0.001;
        // Diffusion rates slightly varied
        let du = 0.14 + ((hash >> 16) % 5) as f64 * 0.01;
        let dv = 0.06 + ((hash >> 24) % 4) as f64 * 0.01;

        // Pulse speed based on commit activity (more commits = faster pulse)
        let pulse_speed = 0.06 + (metrics.num_commits.min(1000) as f64 / 1000.0) * 0.04;

        // Number of seed points based on active sessions (1-5)
        let num_seeds = metrics.active_sessions.clamp(1, 5);

        // Generate seed positions based on hash and number of seeds
        let mut seed_positions = Vec::new();
        let cx = width / 2;
        let cy = height / 2;

        if num_seeds == 1 {
            // Single center seed
            seed_positions.push((cx, cy));
        } else {
            // Distribute seeds in a pattern based on hash
            let angle_offset = ((hash >> 32) % 360) as f64 * std::f64::consts::PI / 180.0;
            let radius = (width.min(height) / 4) as f64;

            for i in 0..num_seeds {
                let angle = angle_offset + (i as f64 * 2.0 * std::f64::consts::PI / num_seeds as f64);
                let sx = (cx as f64 + angle.cos() * radius * 0.5) as usize;
                let sy = (cy as f64 + angle.sin() * radius * 0.25) as usize; // Aspect ratio
                seed_positions.push((sx.clamp(1, width - 2), sy.clamp(1, height - 2)));
            }
        }

        // Initialize grids
        let mut u = vec![vec![1.0; width]; height];
        let mut v = vec![vec![0.0; width]; height];

        // Seed initial pattern at each seed position
        let seed_radius = (width.min(height) / (8 + num_seeds * 2)) as i32;
        for &(sx, sy) in &seed_positions {
            for y in 0..height {
                for x in 0..width {
                    let dx = x as i32 - sx as i32;
                    let dy = (y as i32 - sy as i32) * 2;
                    let dist = ((dx * dx + dy * dy) as f64).sqrt();
                    if dist < seed_radius as f64 {
                        u[y][x] = 0.5;
                        v[y][x] = 0.25;
                    }
                }
            }
        }

        Self {
            width,
            height,
            u,
            v,
            du,
            dv,
            f,
            k,
            time: 0.0,
            pulse_speed,
            num_seeds,
            seed_positions,
            metrics,
        }
    }

    fn resize_with_metrics(&mut self, width: usize, height: usize) {
        if self.width != width || self.height != height {
            *self = Self::with_metrics(width, height, self.metrics.clone());
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
        self.time += self.pulse_speed;
        let pulse = (self.time.sin() * 0.5 + 0.5) * 0.01;
        let f = self.f + pulse;
        let k = self.k - pulse * 0.5;

        // Pacemaker parameters
        let base_radius = (self.width.min(self.height) / (8 + self.num_seeds)) as f64;
        let breath = (self.time * 0.5).sin() * 0.4 + 0.6;
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

                // Pacemaker injection at each seed point
                for &(sx, sy) in &self.seed_positions {
                    let dx = x as f64 - sx as f64;
                    let dy = (y as f64 - sy as f64) * 2.0;
                    let dist = (dx * dx + dy * dy).sqrt();
                    if dist < current_radius {
                        let strength = 1.0 - (dist / current_radius);
                        new_u[y][x] = (new_u[y][x] - 0.1 * strength).max(0.0);
                        new_v[y][x] = (new_v[y][x] + 0.1 * strength).min(1.0);
                    }
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
    SelectPath,     // First screen when not in git repo
    SelectAiTool,
    SelectGitTool,
    SelectExplorer,
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

        // Start with path selection if not in git repo, otherwise AI tool selection
        let initial_screen = if in_git_repo {
            Screen::SelectAiTool
        } else {
            Screen::SelectPath
        };

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
            screen: initial_screen,
            path_input: default_path.clone(),
            cursor_position: default_path.len(),
            selected_path: None,
            in_git_repo,
        }
    }

    fn current_list_len(&self) -> usize {
        match self.screen {
            Screen::SelectPath => 0,
            Screen::SelectAiTool => self.ai_tools.len(),
            Screen::SelectGitTool => self.git_tools.len(),
            Screen::SelectExplorer => self.explorer_tools.len(),
        }
    }

    fn current_list_state(&mut self) -> &mut ListState {
        match self.screen {
            Screen::SelectPath => &mut self.ai_list_state, // unused
            Screen::SelectAiTool => &mut self.ai_list_state,
            Screen::SelectGitTool => &mut self.git_list_state,
            Screen::SelectExplorer => &mut self.explorer_list_state,
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
            Screen::SelectPath => {} // Handled by confirm_path
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
                    self.should_exit = true;
                }
            }
        }
    }

    fn go_back(&mut self) {
        match self.screen {
            Screen::SelectPath => self.should_exit = true,
            Screen::SelectAiTool => {
                if self.in_git_repo {
                    self.should_exit = true;
                } else {
                    self.screen = Screen::SelectPath;
                    self.selected_path = None;
                }
            }
            Screen::SelectGitTool => {
                self.screen = Screen::SelectAiTool;
                self.selected_ai_tool = None;
            }
            Screen::SelectExplorer => {
                self.screen = Screen::SelectGitTool;
                self.selected_git_tool = None;
            }
        }
    }

    fn confirm_path(&mut self) {
        let path = PathBuf::from(&self.path_input);
        if path.exists() && path.is_dir() {
            self.selected_path = Some(path);
            // After selecting path, go to AI tool selection
            self.screen = Screen::SelectAiTool;
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
            // Path selection - draw full screen version
            draw_path_screen(frame, app, main_layout[1]);
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

/// Draw full-screen path selection (when not in git repo)
fn draw_path_screen(frame: &mut Frame, app: &OnboardingApp, area: Rect) {
    // Right side block
    let right_block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray))
        .title(" Setup ");

    let inner_area = right_block.inner(area);
    frame.render_widget(right_block, area);

    // Content layout
    let content_layout = Layout::vertical([
        Constraint::Length(2), // Title spacing
        Constraint::Length(2), // Welcome message
        Constraint::Length(1), // Subtitle
        Constraint::Length(2), // Spacing
        Constraint::Length(2), // Instructions
        Constraint::Length(1), // Spacing
        Constraint::Length(3), // Input field
        Constraint::Length(1), // Status
        Constraint::Min(1),    // Spacer
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
    let instructions = Paragraph::new(Line::from(Span::styled(
        "Enter a path to a git repository:",
        Style::default().fg(Color::Yellow),
    )))
    .alignment(Alignment::Center);
    frame.render_widget(instructions, content_layout[4]);

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
    frame.render_widget(input, content_layout[6]);

    // Validate path and show status
    let path = PathBuf::from(&app.path_input);
    let (status_text, status_color) = if path.exists() && path.is_dir() {
        // Check if it's a git repo
        if path.join(".git").exists() {
            ("Valid git repository", Color::Green)
        } else {
            ("Directory exists (not a git repo)", Color::Yellow)
        }
    } else if path.exists() {
        ("Not a directory", Color::Red)
    } else {
        ("Path does not exist", Color::Red)
    };

    let status = Paragraph::new(Line::from(Span::styled(
        status_text,
        Style::default().fg(status_color),
    )))
    .alignment(Alignment::Center);
    frame.render_widget(status, content_layout[7]);

    // Footer with keybindings
    let footer = Paragraph::new(Line::from(vec![
        Span::styled("Enter", Style::default().fg(Color::Cyan)),
        Span::raw(" continue  "),
        Span::styled("Esc", Style::default().fg(Color::Cyan)),
        Span::raw(" quit"),
    ]))
    .alignment(Alignment::Center);
    frame.render_widget(footer, content_layout[9]);
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

/// Session entry for dashboard
struct SessionEntry {
    #[allow(dead_code)]
    name: String,
    branch: String,
    path: PathBuf,
    has_session: bool,
    is_current: bool,
}

/// Dashboard application state (for existing config)
struct DashboardApp {
    sessions: Vec<SessionEntry>,
    list_state: ListState,
    plasma: ReactionDiffusion,
    last_frame_time: Instant,
    should_exit: bool,
    selected_session: Option<String>,
    repo_name: String,
}

impl DashboardApp {
    fn new() -> Result<Self> {
        let git_root = git::get_root(None).context("Not in a git repository")?;
        let worktrees = git::list_worktrees(&git_root)?;
        let active_sessions = tmux::get_active_sessions();
        let current_session = tmux::get_current_session();

        let repo_name = git_root
            .file_name()
            .context("Invalid git root")?
            .to_string_lossy()
            .to_string();

        let mut sessions: Vec<SessionEntry> = Vec::new();

        for wt in &worktrees {
            let session_name = format!(
                "{}-{}",
                repo_name,
                wt.path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| wt.branch.clone())
            );
            let has_session = active_sessions.contains(&session_name);
            let is_current = current_session.as_ref() == Some(&session_name);

            sessions.push(SessionEntry {
                name: session_name,
                branch: wt.branch.clone(),
                path: wt.path.clone(),
                has_session,
                is_current,
            });
        }

        // Sort: current first, then active sessions, then inactive
        sessions.sort_by(|a, b| {
            match (a.is_current, b.is_current) {
                (true, false) => std::cmp::Ordering::Less,
                (false, true) => std::cmp::Ordering::Greater,
                _ => match (a.has_session, b.has_session) {
                    (true, false) => std::cmp::Ordering::Less,
                    (false, true) => std::cmp::Ordering::Greater,
                    _ => a.branch.cmp(&b.branch),
                }
            }
        });

        let mut list_state = ListState::default();
        if !sessions.is_empty() {
            list_state.select(Some(0));
        }

        // Create workspace metrics for unique plasma pattern
        let metrics = WorkspaceMetrics::from_current_repo();
        let plasma = ReactionDiffusion::with_metrics(80, 40, metrics.clone());

        Ok(Self {
            sessions,
            list_state,
            plasma,
            last_frame_time: Instant::now(),
            should_exit: false,
            selected_session: None,
            repo_name,
        })
    }

    fn update_animation(&mut self) {
        if self.last_frame_time.elapsed() >= Duration::from_millis(50) {
            for _ in 0..4 {
                self.plasma.step();
            }
            self.last_frame_time = Instant::now();
        }
    }

    fn resize_plasma(&mut self, width: usize, height: usize) {
        self.plasma.resize_with_metrics(width, height);
    }

    fn next_item(&mut self) {
        if self.sessions.is_empty() {
            return;
        }
        let i = match self.list_state.selected() {
            Some(i) => {
                if i >= self.sessions.len() - 1 {
                    0
                } else {
                    i + 1
                }
            }
            None => 0,
        };
        self.list_state.select(Some(i));
    }

    fn previous_item(&mut self) {
        if self.sessions.is_empty() {
            return;
        }
        let i = match self.list_state.selected() {
            Some(i) => {
                if i == 0 {
                    self.sessions.len() - 1
                } else {
                    i - 1
                }
            }
            None => 0,
        };
        self.list_state.select(Some(i));
    }

    fn select_current(&mut self) {
        if let Some(i) = self.list_state.selected() {
            if i < self.sessions.len() {
                self.selected_session = Some(self.sessions[i].path.to_string_lossy().to_string());
                self.should_exit = true;
            }
        }
    }

    fn handle_key(&mut self, key: KeyCode) {
        match key {
            KeyCode::Up | KeyCode::Char('k') => self.previous_item(),
            KeyCode::Down | KeyCode::Char('j') => self.next_item(),
            KeyCode::Enter | KeyCode::Char(' ') => self.select_current(),
            KeyCode::Esc | KeyCode::Char('q') => self.should_exit = true,
            _ => {}
        }
    }
}

fn draw_dashboard(frame: &mut Frame, app: &mut DashboardApp) {
    let area = frame.area();

    // Main horizontal layout: Plasma (left) | Content (right)
    let main_layout = Layout::horizontal([
        Constraint::Percentage(45),
        Constraint::Percentage(55),
    ])
    .split(area);

    // Resize plasma to fit
    let plasma_width = main_layout[0].width as usize;
    let plasma_height = main_layout[0].height as usize;
    app.resize_plasma(plasma_width.max(10), plasma_height.max(10));

    // Render plasma
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

    let ascii_area = Rect {
        x: main_layout[0].x,
        y: main_layout[0].y + vertical_padding,
        width: main_layout[0].width,
        height: ascii_height.min(available_height),
    };

    let ascii_widget = Paragraph::new(ascii_lines);
    frame.render_widget(ascii_widget, ascii_area);

    // Right side: session picker
    let right_block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray))
        .title(format!(" {} ", app.repo_name));

    let inner_area = right_block.inner(main_layout[1]);
    frame.render_widget(right_block, main_layout[1]);

    // Content layout
    let content_layout = Layout::vertical([
        Constraint::Length(2), // Title spacing
        Constraint::Length(2), // Welcome
        Constraint::Length(1), // Subtitle
        Constraint::Length(2), // Spacing
        Constraint::Length(2), // Instructions
        Constraint::Min(8),    // Session list
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
        "Select a workspace to open",
        Style::default().fg(Color::DarkGray),
    )))
    .alignment(Alignment::Center);
    frame.render_widget(subtitle, content_layout[2]);

    // Instructions
    let instructions = Paragraph::new(Line::from(Span::styled(
        "Worktrees & Sessions",
        Style::default().fg(Color::Yellow),
    )))
    .alignment(Alignment::Center);
    frame.render_widget(instructions, content_layout[4]);

    // Session list
    let items: Vec<ListItem> = app
        .sessions
        .iter()
        .map(|session| {
            let status = if session.is_current {
                Span::styled(" [current]", Style::default().fg(Color::Cyan))
            } else if session.has_session {
                Span::styled(" [active]", Style::default().fg(Color::Green))
            } else {
                Span::styled(" [inactive]", Style::default().fg(Color::DarkGray))
            };
            ListItem::new(Line::from(vec![
                Span::styled(
                    format!(" {} ", session.branch),
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
                .title(" Sessions "),
        )
        .highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("> ");

    frame.render_stateful_widget(list, content_layout[5], &mut app.list_state);

    // Footer
    let footer = Paragraph::new(Line::from(vec![
        Span::styled("j/k", Style::default().fg(Color::Cyan)),
        Span::raw(" navigate  "),
        Span::styled("Enter", Style::default().fg(Color::Cyan)),
        Span::raw(" open  "),
        Span::styled("Esc", Style::default().fg(Color::Cyan)),
        Span::raw(" quit"),
    ]))
    .alignment(Alignment::Center);
    frame.render_widget(footer, content_layout[6]);
}

/// Result from dashboard
pub enum DashboardResult {
    /// User selected a session to open
    OpenSession(String),
    /// User quit without selecting
    Quit,
}

/// Run the dashboard TUI (for when config already exists)
pub fn run_dashboard() -> Result<DashboardResult> {
    let mut app = DashboardApp::new()?;

    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Main loop
    while !app.should_exit {
        app.update_animation();

        terminal.draw(|frame| draw_dashboard(frame, &mut app))?;

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

    if let Some(path) = app.selected_session {
        Ok(DashboardResult::OpenSession(path))
    } else {
        Ok(DashboardResult::Quit)
    }
}
