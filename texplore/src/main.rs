use std::collections::HashMap;
use std::env;
use std::fs;
use std::io::{self, stdout};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use chrono::{DateTime, Local};
use crossterm::{
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyModifiers,
        MouseButton, MouseEvent, MouseEventKind,
    },
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ignore::gitignore::{Gitignore, GitignoreBuilder};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Terminal,
};
use std::os::unix::fs::PermissionsExt;

const SPINNER_FRAMES: [&str; 4] = ["|", "/", "-", "\\"];
const SPINNER_RATE_MS: u128 = 50;
const VERSION: &str = env!("CARGO_PKG_VERSION");

fn main() -> io::Result<()> {
    if let Some(arg) = env::args().nth(1) {
        match arg.as_str() {
            "--version" | "-V" => {
                println!("texplore {}", VERSION);
                return Ok(());
            }
            "--help" | "-h" => {
                println!(
                    "texplore {} - Terminal file explorer with git integration",
                    VERSION
                );
                println!();
                println!("Usage: texplore [PATH]");
                println!();
                println!("Arguments:");
                println!("  [PATH]  Directory to explore (default: current directory)");
                println!();
                println!("Options:");
                println!("  -h, --help     Print help");
                println!("  -V, --version  Print version");
                return Ok(());
            }
            _ => {}
        }
    }

    let root = match env::args().nth(1) {
        Some(arg) => PathBuf::from(arg),
        None => env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
    };

    let root_abs = fs::canonicalize(&root).unwrap_or_else(|_| root.clone());
    let git_status = load_git_status(&root_abs);
    let gitignore = build_gitignore(&root_abs);
    let mut root_node = build_node(&root_abs, &gitignore, &git_status)?;
    if root_node.is_dir {
        root_node.expanded = true;
        load_children(&mut root_node, &gitignore, &git_status)?;
    }
    expand_changed_paths(&mut root_node, &root_abs, &gitignore, &git_status)?;

    enable_raw_mode()?;
    let mut stdout = stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = App::new(root_node, gitignore, git_status, root_abs);
    let mut last_refresh = Instant::now();
    let mut has_focus = true;

    loop {
        app.refresh_visible();
        terminal.draw(|frame| render(frame, &mut app))?;

        if event::poll(Duration::from_millis(SPINNER_RATE_MS as u64))? {
            match event::read()? {
                Event::Key(key) => {
                    if handle_key(&mut app, key)? {
                        break;
                    }
                }
                Event::FocusLost => {
                    has_focus = false;
                }
                Event::FocusGained => {
                    has_focus = true;
                    app.refreshing = true;
                    resync(&mut app)?;
                    app.refreshing = false;
                    last_refresh = Instant::now();
                }
                Event::Mouse(mouse) => {
                    if has_focus {
                        handle_mouse(&mut app, mouse)?;
                    }
                }
                _ => {}
            }
        }

        if last_refresh.elapsed() >= Duration::from_secs(30) {
            app.refreshing = true;
            resync(&mut app)?;
            app.refreshing = false;
            last_refresh = Instant::now();
        }
    }

    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;

    Ok(())
}

struct App {
    root: Node,
    gitignore: Option<Gitignore>,
    git_status: GitStatus,
    root_path: PathBuf,
    visible: Vec<VisibleEntry>,
    focus: usize,
    scroll: usize,
    status: String,
    pending_delete: Option<usize>,
    viewer: Option<Viewer>,
    last_click: Option<(Instant, usize)>,
    refreshing: bool,
}

impl App {
    fn new(
        root: Node,
        gitignore: Option<Gitignore>,
        git_status: GitStatus,
        root_path: PathBuf,
    ) -> Self {
        Self {
            root,
            gitignore,
            git_status,
            root_path,
            visible: Vec::new(),
            focus: 0,
            scroll: 0,
            status: String::from(
                "q: quit  j/k: move  h/l/Enter: collapse/expand  d: delete  o: open  N: nvim  C: copy",
            ),
            pending_delete: None,
            viewer: None,
            last_click: None,
            refreshing: false,
        }
    }

    fn refresh_visible(&mut self) {
        self.visible.clear();
        let mut indices = Vec::new();
        let mut bars = Vec::new();
        let metrics = format_git_metrics(&self.git_status);
        collect_visible(
            &self.root,
            &mut indices,
            &mut bars,
            true,
            true,
            &metrics,
            &mut self.visible,
        );

        if self.visible.is_empty() {
            self.focus = 0;
            self.scroll = 0;
            return;
        }

        if self.focus >= self.visible.len() {
            self.focus = self.visible.len() - 1;
        }

        if self.pending_delete.is_some() {
            if let Some(idx) = self.pending_delete {
                if idx >= self.visible.len() {
                    self.pending_delete = None;
                }
            }
        }
    }
}

#[derive(Clone)]
struct VisibleEntry {
    indices: Vec<usize>,
    prefix: String,
    path: PathBuf,
    is_dir: bool,
    icon: &'static str,
    icon_key: String,
    status: String,
    modified: String,
    subtree_changes: usize,
    metrics: String,
    name: String,
    ignored: bool,
}

struct Node {
    path: PathBuf,
    is_dir: bool,
    expanded: bool,
    children: Option<Vec<Node>>,
    icon: &'static str,
    icon_key: String,
    status: String,
    modified: String,
    subtree_changes: usize,
    name: String,
    ignored: bool,
}

struct Viewer {
    title: String,
    lines: Vec<StyledLine>,
    scroll: usize,
    pending_g: bool,
}

#[derive(Clone, Default, PartialEq)]
struct TextStyle {
    fg: Option<Color>,
    bg: Option<Color>,
    bold: bool,
    dim: bool,
    italic: bool,
    underline: bool,
}

#[derive(Clone)]
struct StyledSpan {
    text: String,
    style: TextStyle,
}

#[derive(Clone)]
struct StyledLine {
    spans: Vec<StyledSpan>,
}

fn handle_key(app: &mut App, key: KeyEvent) -> io::Result<bool> {
    if app.viewer.is_some() {
        return handle_viewer_key(app, key);
    }

    match key.code {
        KeyCode::Char('q') => return Ok(true),
        KeyCode::Char('j') | KeyCode::Down => move_focus(app, 1),
        KeyCode::Char('k') | KeyCode::Up => move_focus(app, -1),
        KeyCode::Char('g') if key.modifiers.contains(KeyModifiers::SHIFT) => {
            app.focus = app.visible.len().saturating_sub(1);
        }
        KeyCode::Char('g') => app.focus = 0,
        KeyCode::Char('h') | KeyCode::Left => collapse_node(app),
        KeyCode::Char('l') | KeyCode::Right => expand_node(app),
        KeyCode::Char('d') => prompt_delete(app),
        KeyCode::Char('o') => open_with_bat(app)?,
        KeyCode::Char('y') => confirm_delete(app)?,
        KeyCode::Char('C') => copy_path_to_clipboard(app)?,
        KeyCode::Char('N') => open_in_nvim_popup(app)?,
        KeyCode::Enter => toggle_or_open(app)?,
        KeyCode::Esc => cancel_delete(app),
        _ => {}
    }

    Ok(false)
}

fn handle_mouse(app: &mut App, mouse: MouseEvent) -> io::Result<()> {
    if let Some(viewer) = app.viewer.as_mut() {
        let height = crossterm::terminal::size()?.1 as usize;
        let view_height = height.saturating_sub(2);
        let max_scroll = viewer.lines.len().saturating_sub(view_height);
        match mouse.kind {
            MouseEventKind::ScrollDown => {
                viewer.scroll = (viewer.scroll + 1).min(max_scroll);
                viewer.pending_g = false;
            }
            MouseEventKind::ScrollUp => {
                viewer.scroll = viewer.scroll.saturating_sub(1);
                viewer.pending_g = false;
            }
            _ => {}
        }
        return Ok(());
    }

    match mouse.kind {
        MouseEventKind::ScrollDown => {
            move_focus(app, 1);
            app.last_click = None;
        }
        MouseEventKind::ScrollUp => {
            move_focus(app, -1);
            app.last_click = None;
        }
        MouseEventKind::Down(MouseButton::Left) => {
            let height = crossterm::terminal::size()?.1 as usize;
            let view_height = height.saturating_sub(2);
            let row = mouse.row as usize;
            if row == 0 || row > view_height {
                return Ok(());
            }
            let idx = app.scroll + row - 1;
            if idx >= app.visible.len() {
                return Ok(());
            }

            app.focus = idx;
            if app.visible[idx].is_dir {
                toggle_expand(app);
                app.last_click = None;
                return Ok(());
            }

            let now = Instant::now();
            let mut is_double = false;
            if let Some((last_time, last_idx)) = app.last_click {
                if last_idx == idx && now.duration_since(last_time) <= Duration::from_millis(400) {
                    is_double = true;
                }
            }
            app.last_click = Some((now, idx));
            if is_double {
                app.last_click = None;
                open_with_bat(app)?;
            }
        }
        _ => {}
    }

    Ok(())
}

fn resync(app: &mut App) -> io::Result<()> {
    let focused_path = app.visible.get(app.focus).map(|entry| entry.path.clone());
    let root_abs = app.root_path.clone();
    let git_status = load_git_status(&root_abs);
    let gitignore = build_gitignore(&root_abs);
    let mut root_node = build_node(&root_abs, &gitignore, &git_status)?;
    if root_node.is_dir {
        root_node.expanded = true;
        load_children(&mut root_node, &gitignore, &git_status)?;
    }
    expand_changed_paths(&mut root_node, &root_abs, &gitignore, &git_status)?;

    app.root = root_node;
    app.gitignore = gitignore;
    app.git_status = git_status;
    app.refresh_visible();

    if let Some(path) = focused_path {
        if let Some(idx) = app.visible.iter().position(|entry| entry.path == path) {
            app.focus = idx;
        }
    }

    Ok(())
}

fn handle_viewer_key(app: &mut App, key: KeyEvent) -> io::Result<bool> {
    let height = crossterm::terminal::size()?.1 as usize;
    let view_height = height.saturating_sub(2);
    let max_scroll = app
        .viewer
        .as_ref()
        .map(|viewer| viewer.lines.len().saturating_sub(view_height))
        .unwrap_or(0);

    if let Some(viewer) = app.viewer.as_mut() {
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => {
                app.viewer = None;
                app.status = String::from(
                    "q: quit  j/k: move  h/l/Enter: collapse/expand  d: delete  y: confirm  o: open",
                );
            }
            KeyCode::Char('j') | KeyCode::Down => {
                viewer.scroll = (viewer.scroll + 1).min(max_scroll);
                viewer.pending_g = false;
            }
            KeyCode::Char('k') | KeyCode::Up => {
                viewer.scroll = viewer.scroll.saturating_sub(1);
                viewer.pending_g = false;
            }
            KeyCode::Char('g') if key.modifiers.contains(KeyModifiers::SHIFT) => {
                viewer.scroll = max_scroll;
                viewer.pending_g = false;
            }
            KeyCode::Char('g') => {
                if viewer.pending_g {
                    viewer.scroll = 0;
                    viewer.pending_g = false;
                } else {
                    viewer.pending_g = true;
                }
            }
            _ => {
                viewer.pending_g = false;
            }
        }
    }

    Ok(false)
}

fn move_focus(app: &mut App, delta: isize) {
    if app.visible.is_empty() {
        return;
    }
    let len = app.visible.len() as isize;
    let mut next = app.focus as isize + delta;
    if next < 0 {
        next = 0;
    }
    if next >= len {
        next = len - 1;
    }
    app.focus = next as usize;
}

fn copy_path_to_clipboard(app: &mut App) -> io::Result<()> {
    let entry = match app.visible.get(app.focus) {
        Some(entry) => entry.clone(),
        None => return Ok(()),
    };

    // Get relative path from root
    let rel_path = entry
        .path
        .strip_prefix(&app.root_path)
        .unwrap_or(&entry.path);

    let path_str = rel_path.display().to_string();

    // Use pbcopy on macOS, xclip/xsel on Linux
    #[cfg(target_os = "macos")]
    let result = std::process::Command::new("pbcopy")
        .stdin(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write;
            if let Some(stdin) = child.stdin.as_mut() {
                stdin.write_all(path_str.as_bytes())?;
            }
            child.wait()
        });

    #[cfg(not(target_os = "macos"))]
    let result = std::process::Command::new("xclip")
        .args(["-selection", "clipboard"])
        .stdin(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write;
            if let Some(stdin) = child.stdin.as_mut() {
                stdin.write_all(path_str.as_bytes())?;
            }
            child.wait()
        });

    match result {
        Ok(status) if status.success() => {
            app.status = format!("copied: {}", path_str);
        }
        _ => {
            app.status = String::from("failed to copy to clipboard");
        }
    }

    Ok(())
}

fn open_in_nvim_popup(app: &mut App) -> io::Result<()> {
    let entry = match app.visible.get(app.focus) {
        Some(entry) => entry.clone(),
        None => return Ok(()),
    };

    if entry.path.is_dir() {
        app.status = String::from("cannot open a directory in nvim");
        return Ok(());
    }

    // Check if we're inside tmux
    if std::env::var("TMUX").is_err() {
        app.status = String::from("nvim popup requires tmux");
        return Ok(());
    }

    let path_str = entry.path.display().to_string();

    // Open nvim in a tmux popup with double-Esc to quit
    let result = std::process::Command::new("tmux")
        .args([
            "display-popup",
            "-E",
            "-w",
            "80%",
            "-h",
            "80%",
            &format!("nvim -c 'nnoremap <Esc><Esc> :q!<CR>' '{}'", path_str),
        ])
        .status();

    match result {
        Ok(status) if status.success() => {
            app.status = format!(
                "opened: {}",
                entry.path.file_name().unwrap_or_default().to_string_lossy()
            );
        }
        _ => {
            app.status = String::from("failed to open nvim popup");
        }
    }

    Ok(())
}

fn open_with_bat(app: &mut App) -> io::Result<()> {
    let entry = match app.visible.get(app.focus) {
        Some(entry) => entry.clone(),
        None => return Ok(()),
    };

    if entry.path.is_dir() {
        app.status = String::from("cannot open a directory");
        return Ok(());
    }

    let width = crossterm::terminal::size()?.0;
    let output = std::process::Command::new("bat")
        .arg("--paging=never")
        .arg("--color=always")
        .arg("--decorations=always")
        .arg("--style=full")
        .arg(format!("--terminal-width={}", width))
        .arg(&entry.path)
        .output();

    match output {
        Ok(output) if output.status.success() => {
            let content = String::from_utf8_lossy(&output.stdout);
            let lines = parse_ansi_lines(&content);
            app.viewer = Some(Viewer {
                title: entry.path.display().to_string(),
                lines,
                scroll: 0,
                pending_g: false,
            });
            app.status = String::from("VIEW: q close  j/k scroll  gg/G top/bottom");
        }
        Ok(output) => {
            let error = String::from_utf8_lossy(&output.stderr);
            app.status = format!("bat failed: {}", error.trim());
        }
        Err(err) => {
            app.status = format!("bat failed: {}", err);
        }
    }

    Ok(())
}

fn collapse_node(app: &mut App) {
    if let Some(entry) = app.visible.get(app.focus) {
        if let Some(node) = node_at_mut(&mut app.root, &entry.indices) {
            if node.is_dir && node.expanded {
                node.expanded = false;
                app.status = format!("collapsed {}", node.name);
            }
        }
    }
}

fn expand_node(app: &mut App) {
    if let Some(entry) = app.visible.get(app.focus) {
        if let Some(node) = node_at_mut(&mut app.root, &entry.indices) {
            if node.is_dir {
                if !node.expanded {
                    node.expanded = true;
                }
                if node.children.is_none() {
                    if let Err(err) = load_children(node, &app.gitignore, &app.git_status) {
                        app.status = format!("error: {}", err);
                    }
                }
            }
        }
    }
}

fn toggle_expand(app: &mut App) {
    if let Some(entry) = app.visible.get(app.focus) {
        if let Some(node) = node_at_mut(&mut app.root, &entry.indices) {
            if node.is_dir {
                if node.expanded {
                    node.expanded = false;
                    app.status = format!("collapsed {}", node.name);
                } else {
                    node.expanded = true;
                    if node.children.is_none() {
                        if let Err(err) = load_children(node, &app.gitignore, &app.git_status) {
                            app.status = format!("error: {}", err);
                        }
                    }
                }
            }
        }
    }
}

fn toggle_or_open(app: &mut App) -> io::Result<()> {
    let entry = match app.visible.get(app.focus) {
        Some(entry) => entry.clone(),
        None => return Ok(()),
    };

    if entry.path.is_dir() {
        toggle_expand(app);
        return Ok(());
    }

    open_with_bat(app)
}

fn prompt_delete(app: &mut App) {
    if let Some(entry) = app.visible.get(app.focus) {
        if entry.indices.is_empty() {
            app.status = String::from("cannot delete root");
            return;
        }
        app.pending_delete = Some(app.focus);
        app.status = format!("Delete {}? y to confirm, Esc to cancel", entry.name);
    }
}

fn cancel_delete(app: &mut App) {
    if app.pending_delete.is_some() {
        app.pending_delete = None;
        app.status = String::from("delete canceled");
    }
}

fn confirm_delete(app: &mut App) -> io::Result<()> {
    let idx = match app.pending_delete {
        Some(idx) => idx,
        None => return Ok(()),
    };

    let entry = match app.visible.get(idx) {
        Some(entry) => entry.clone(),
        None => return Ok(()),
    };

    if entry.indices.is_empty() {
        app.status = String::from("cannot delete root");
        app.pending_delete = None;
        return Ok(());
    }

    if let Err(err) = trash::delete(&entry.path) {
        app.status = format!("delete failed: {}", err);
        app.pending_delete = None;
        return Ok(());
    }

    if let Some(parent) = parent_at_mut(&mut app.root, &entry.indices) {
        if let Some(children) = parent.children.as_mut() {
            let remove_idx = *entry.indices.last().unwrap();
            if remove_idx < children.len() {
                children.remove(remove_idx);
            }
        }
    }

    app.pending_delete = None;
    app.status = format!("deleted {}", entry.name);
    Ok(())
}

fn render(frame: &mut ratatui::Frame, app: &mut App) {
    let area = frame.area();

    let chunks = Layout::vertical([
        Constraint::Length(1), // Title bar
        Constraint::Min(1),    // Tree view
        Constraint::Length(1), // Status bar
    ])
    .split(area);

    if app.viewer.is_some() {
        render_viewer(frame, app, chunks[0], chunks[1], chunks[2]);
    } else {
        render_tree(frame, app, chunks[0], chunks[1], chunks[2]);
    }
}

fn render_tree(
    frame: &mut ratatui::Frame,
    app: &mut App,
    title_area: Rect,
    tree_area: Rect,
    status_area: Rect,
) {
    let view_height = tree_area.height as usize;

    // Adjust scroll
    if app.focus < app.scroll {
        app.scroll = app.focus;
    }
    if app.focus >= app.scroll + view_height && view_height > 0 {
        app.scroll = app.focus - view_height + 1;
    }

    // Title bar
    let title = if let Some(name) = app.root_path.file_name().and_then(|n| n.to_str()) {
        format!("texplore - {}", name)
    } else {
        format!("texplore - {}", app.root_path.display())
    };
    let title_widget = Paragraph::new(title).style(Style::default().add_modifier(Modifier::DIM));
    frame.render_widget(title_widget, title_area);

    // Tree lines
    let use_color = should_color();
    let lines: Vec<Line> = app
        .visible
        .iter()
        .skip(app.scroll)
        .take(view_height)
        .enumerate()
        .map(|(row, entry)| {
            let focused = app.focus == app.scroll + row;
            build_tree_line(entry, focused, use_color, tree_area.width as usize)
        })
        .collect();

    let tree_widget = Paragraph::new(lines);
    frame.render_widget(tree_widget, tree_area);

    // Status bar
    let status_text = if app.refreshing {
        format!("{} {}", spinner_frame(), app.status)
    } else {
        app.status.clone()
    };
    let status_widget = Paragraph::new(status_text);
    frame.render_widget(status_widget, status_area);
}

fn render_viewer(
    frame: &mut ratatui::Frame,
    app: &mut App,
    title_area: Rect,
    content_area: Rect,
    status_area: Rect,
) {
    let viewer = match app.viewer.as_ref() {
        Some(v) => v,
        None => return,
    };

    // Title bar
    let title = if let Some(name) = app.root_path.file_name().and_then(|n| n.to_str()) {
        format!("texplore - {}", name)
    } else {
        format!("texplore - {}", app.root_path.display())
    };
    let title_widget = Paragraph::new(title).style(Style::default().add_modifier(Modifier::DIM));
    frame.render_widget(title_widget, title_area);

    // Content
    let view_height = content_area.height as usize;
    let max_scroll = viewer.lines.len().saturating_sub(view_height);
    let scroll = viewer.scroll.min(max_scroll);

    let lines: Vec<Line> = viewer
        .lines
        .iter()
        .skip(scroll)
        .take(view_height)
        .map(|styled_line| build_styled_line(styled_line))
        .collect();

    let content_widget = Paragraph::new(lines);
    frame.render_widget(content_widget, content_area);

    // Status bar
    let status = format!("{} | q close  j/k scroll  gg/G top/bottom", viewer.title);
    let status_widget = Paragraph::new(status);
    frame.render_widget(status_widget, status_area);
}

fn build_tree_line(entry: &VisibleEntry, focused: bool, use_color: bool, width: usize) -> Line<'_> {
    let mut spans = Vec::new();

    let base_style = if focused {
        Style::default().add_modifier(Modifier::REVERSED)
    } else {
        Style::default()
    };

    // Prefix (tree lines)
    if !entry.prefix.is_empty() {
        let prefix_style = if use_color && !focused {
            base_style.fg(Color::DarkGray).add_modifier(Modifier::DIM)
        } else {
            base_style
        };
        spans.push(Span::styled(format!("{} ", entry.prefix), prefix_style));
    }

    // Icon
    let icon_color = if use_color && !focused {
        color_for_key(&entry.icon_key)
    } else {
        Color::Reset
    };
    spans.push(Span::styled(
        format!("{} ", entry.icon),
        base_style.fg(icon_color),
    ));

    // Name
    let name_style = if entry.ignored && !focused {
        base_style.add_modifier(Modifier::DIM)
    } else if use_color && !focused {
        base_style.fg(color_for_key(&entry.icon_key))
    } else {
        base_style
    };
    spans.push(Span::styled(entry.name.clone(), name_style));

    // Calculate current length for right-aligned content
    let current_len: usize = spans.iter().map(|s| s.content.chars().count()).sum();

    // Build suffix parts
    let mut suffix_parts: Vec<(String, Color)> = Vec::new();

    if !entry.status.trim().is_empty() {
        let color = if use_color {
            color_for_status(&entry.status)
        } else {
            Color::Reset
        };
        suffix_parts.push((entry.status.clone(), color));
    }

    if !entry.modified.is_empty() {
        suffix_parts.push((entry.modified.clone(), Color::DarkGray));
    }

    if entry.is_dir && entry.subtree_changes > 0 {
        suffix_parts.push((format!("Δ{}", entry.subtree_changes), Color::Yellow));
    }

    if !entry.metrics.is_empty() {
        suffix_parts.push((entry.metrics.clone(), Color::DarkGray));
    }

    // Calculate suffix length
    let suffix_len: usize = suffix_parts
        .iter()
        .map(|(s, _)| s.chars().count())
        .sum::<usize>()
        + suffix_parts.len().saturating_sub(1) * 2; // separators

    // Add padding and suffix if there's room
    if !suffix_parts.is_empty() && current_len + 1 + suffix_len < width {
        let pad = width.saturating_sub(current_len).saturating_sub(suffix_len);
        if pad > 0 {
            spans.push(Span::styled(" ".repeat(pad), base_style));
        }

        for (i, (text, color)) in suffix_parts.into_iter().enumerate() {
            if i > 0 {
                spans.push(Span::styled("  ", base_style));
            }
            let style = if focused {
                base_style
            } else {
                base_style.fg(color)
            };
            spans.push(Span::styled(text, style));
        }
    }

    Line::from(spans)
}

fn build_styled_line(styled_line: &StyledLine) -> Line<'_> {
    let spans: Vec<Span> = styled_line
        .spans
        .iter()
        .map(|span| {
            let mut style = Style::default();
            if let Some(fg) = span.style.fg {
                style = style.fg(fg);
            }
            if let Some(bg) = span.style.bg {
                style = style.bg(bg);
            }
            if span.style.bold {
                style = style.add_modifier(Modifier::BOLD);
            }
            if span.style.dim {
                style = style.add_modifier(Modifier::DIM);
            }
            if span.style.italic {
                style = style.add_modifier(Modifier::ITALIC);
            }
            if span.style.underline {
                style = style.add_modifier(Modifier::UNDERLINED);
            }
            Span::styled(span.text.clone(), style)
        })
        .collect();
    Line::from(spans)
}

fn spinner_frame() -> &'static str {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let index = (millis / SPINNER_RATE_MS) as usize % SPINNER_FRAMES.len();
    SPINNER_FRAMES[index]
}

fn collect_visible(
    node: &Node,
    indices: &mut Vec<usize>,
    bars: &mut Vec<bool>,
    is_root: bool,
    is_last: bool,
    root_metrics: &str,
    out: &mut Vec<VisibleEntry>,
) {
    let prefix = if is_root {
        String::new()
    } else {
        make_prefix(bars, is_last)
    };

    let metrics = if indices.is_empty() {
        root_metrics.to_string()
    } else {
        String::new()
    };

    out.push(VisibleEntry {
        indices: indices.clone(),
        prefix,
        path: node.path.clone(),
        is_dir: node.is_dir,
        icon: node.icon,
        icon_key: node.icon_key.clone(),
        status: node.status.clone(),
        modified: node.modified.clone(),
        subtree_changes: node.subtree_changes,
        metrics,
        name: node.name.clone(),
        ignored: node.ignored,
    });

    if node.is_dir && node.expanded {
        if let Some(children) = &node.children {
            bars.push(!is_last);
            let last_index = children.len().saturating_sub(1);
            for (idx, child) in children.iter().enumerate() {
                let child_last = idx == last_index;
                indices.push(idx);
                collect_visible(child, indices, bars, false, child_last, root_metrics, out);
                indices.pop();
            }
            bars.pop();
        }
    }
}

fn make_prefix(bars: &[bool], is_last: bool) -> String {
    let mut prefix = String::new();
    for &bar in bars {
        if bar {
            prefix.push('│');
        } else {
            prefix.push(' ');
        }
    }
    prefix.push(if is_last { '└' } else { '├' });
    prefix
}

fn build_node(
    path: &Path,
    gitignore: &Option<Gitignore>,
    git_status: &GitStatus,
) -> io::Result<Node> {
    let meta = fs::symlink_metadata(path)?;
    let file_type = meta.file_type();
    let (icon, icon_key) = icon_for(path, &meta, file_type);
    let is_dir = meta.is_dir();
    let name = display_name(path, is_dir);
    let ignored = is_ignored(gitignore, path, is_dir);
    let status = git_status
        .map
        .get(path)
        .cloned()
        .unwrap_or_else(|| "  ".to_string());
    let modified = format_modified(&meta);
    let subtree_changes = if is_dir || status.trim().is_empty() {
        0
    } else {
        1
    };

    Ok(Node {
        path: path.to_path_buf(),
        is_dir,
        expanded: false,
        children: None,
        icon,
        icon_key,
        status,
        modified,
        subtree_changes,
        name,
        ignored,
    })
}

fn load_children(
    node: &mut Node,
    gitignore: &Option<Gitignore>,
    git_status: &GitStatus,
) -> io::Result<()> {
    if !node.is_dir {
        return Ok(());
    }

    let mut children = Vec::new();
    for entry in fs::read_dir(&node.path)? {
        let entry = match entry {
            Ok(e) => e,
            Err(err) => {
                eprintln!("warn: {}", err);
                continue;
            }
        };
        let child_path = entry.path();
        match build_node(&child_path, gitignore, git_status) {
            Ok(child) => children.push(child),
            Err(err) => eprintln!("warn: {}", err),
        }
    }

    children.sort_by(|a, b| sort_key(&a.path).cmp(&sort_key(&b.path)));
    node.children = Some(children);
    if let Some(children) = node.children.as_ref() {
        node.subtree_changes = children.iter().map(|child| child.subtree_changes).sum();
    }
    Ok(())
}

fn node_at_mut<'a>(node: &'a mut Node, indices: &[usize]) -> Option<&'a mut Node> {
    let mut current = node;
    for &idx in indices {
        let children = current.children.as_mut()?;
        current = children.get_mut(idx)?;
    }
    Some(current)
}

fn parent_at_mut<'a>(node: &'a mut Node, indices: &[usize]) -> Option<&'a mut Node> {
    if indices.is_empty() {
        return None;
    }
    node_at_mut(node, &indices[..indices.len() - 1])
}

fn icon_for(path: &Path, meta: &fs::Metadata, file_type: fs::FileType) -> (&'static str, String) {
    if file_type.is_symlink() {
        let key = "symlink".to_string();
        return (icon_for_key(&key), key);
    }
    if meta.is_dir() {
        let key = "directory".to_string();
        return (icon_for_key(&key), key);
    }
    if is_executable(meta) {
        let key = "executable".to_string();
        return (icon_for_key(&key), key);
    }

    let icon_key = icon_key_for(path);
    (icon_for_key(&icon_key), icon_key)
}

fn is_executable(meta: &fs::Metadata) -> bool {
    if !meta.is_file() {
        return false;
    }
    meta.permissions().mode() & 0o111 != 0
}

fn display_name(path: &Path, is_dir: bool) -> String {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .map(|name| name.to_string())
        .unwrap_or_else(|| path.display().to_string());

    if is_dir && !name.ends_with('/') {
        format!("{}/", name)
    } else {
        name
    }
}

fn sort_key(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
}

fn build_gitignore(root: &Path) -> Option<Gitignore> {
    let git_root = git_toplevel(root)?;
    let mut builder = GitignoreBuilder::new(&git_root);

    // Add root .gitignore - this handles patterns like /target/
    let root_ignore = git_root.join(".gitignore");
    if root_ignore.is_file() {
        let _ = builder.add(&root_ignore);
    }

    match builder.build() {
        Ok(ignore) => Some(ignore),
        Err(err) => {
            eprintln!("warn: {}", err);
            None
        }
    }
}

fn icon_key_for(path: &Path) -> String {
    if let Some(stem) = path.file_name().and_then(|name| name.to_str()) {
        let stem_key = stem.to_string();
        if let Some(key) = icon_key_for_stem(&stem_key) {
            return key;
        }
    }

    if let Some(name) = path.file_name().and_then(|name| name.to_str()) {
        let name_key = name.to_string();
        if let Some(key) = icon_key_for_suffix(&name_key) {
            return key;
        }
    }

    if let Some(ext) = path.extension().and_then(|ext| ext.to_str()) {
        let ext_key = ext.to_ascii_lowercase();
        if let Some(key) = icon_key_for_suffix(&ext_key) {
            return key;
        }
    }

    "default".to_string()
}

fn icon_key_for_stem(stem: &str) -> Option<String> {
    match stem {
        "Dockerfile" => Some("docker".to_string()),
        "Podfile" => Some("ruby".to_string()),
        "Procfile" => Some("heroku".to_string()),
        _ => None,
    }
}

fn icon_key_for_suffix(suffix: &str) -> Option<String> {
    let suffix = suffix.to_ascii_lowercase();
    match suffix.as_str() {
        "astro" => Some("astro".to_string()),
        "aac" | "flac" | "m4a" | "mka" | "mp3" | "ogg" | "opus" | "wav" | "wma" | "wv" => {
            Some("audio".to_string())
        }
        "bak" => Some("backup".to_string()),
        "bicep" => Some("bicep".to_string()),
        "lockb" => Some("bun".to_string()),
        "c" | "h" => Some("c".to_string()),
        "cairo" => Some("cairo".to_string()),
        "handlebars" | "metadata" | "rkt" | "scm" => Some("code".to_string()),
        "coffee" => Some("coffeescript".to_string()),
        "c++" | "h++" | "cc" | "cpp" | "cxx" | "hh" | "hpp" | "hxx" | "inl" | "ixx" => {
            Some("cpp".to_string())
        }
        "cr" | "ecr" => Some("crystal".to_string()),
        "cs" => Some("csharp".to_string()),
        "csproj" => Some("csproj".to_string()),
        "css" | "pcss" | "postcss" => Some("css".to_string()),
        "cue" => Some("cue".to_string()),
        "dart" => Some("dart".to_string()),
        "diff" => Some("diff".to_string()),
        "doc" | "docx" | "mdx" | "odp" | "ods" | "odt" | "pdf" | "ppt" | "pptx" | "rtf" | "txt"
        | "xls" | "xlsx" => Some("document".to_string()),
        "eex" | "ex" | "exs" | "heex" => Some("elixir".to_string()),
        "elm" => Some("elm".to_string()),
        "emakefile" | "app.src" | "erl" | "escript" | "hrl" | "rebar.config" | "xrl" | "yrl" => {
            Some("erlang".to_string())
        }
        "eslint.config.cjs" | "eslint.config.cts" | "eslint.config.js" | "eslint.config.mjs"
        | "eslint.config.mts" | "eslint.config.ts" | "eslintrc" | "eslintrc.js"
        | "eslintrc.json" => Some("eslint".to_string()),
        "otf" | "ttf" | "woff" | "woff2" => Some("font".to_string()),
        "fs" => Some("fsharp".to_string()),
        "fsproj" => Some("fsproj".to_string()),
        "gitlab-ci.yml" => Some("gitlab".to_string()),
        "gleam" => Some("gleam".to_string()),
        "go" | "mod" | "work" => Some("go".to_string()),
        "gql" | "graphql" | "graphqls" => Some("graphql".to_string()),
        "hs" => Some("haskell".to_string()),
        "hcl" => Some("hcl".to_string()),
        "htm" | "html" => Some("html".to_string()),
        "avif" | "bmp" | "gif" | "heic" | "heif" | "ico" | "j2k" | "jfif" | "jp2" | "jpeg"
        | "jpg" | "jxl" | "png" | "psd" | "qoi" | "svg" | "tiff" | "webp" => {
            Some("image".to_string())
        }
        "java" => Some("java".to_string()),
        "cjs" | "js" | "mjs" => Some("javascript".to_string()),
        "json" | "jsonc" => Some("json".to_string()),
        "jl" => Some("julia".to_string()),
        "kdl" => Some("kdl".to_string()),
        "kt" => Some("kotlin".to_string()),
        "lock" => Some("lock".to_string()),
        "log" => Some("log".to_string()),
        "lua" => Some("lua".to_string()),
        "luau" => Some("luau".to_string()),
        "markdown" | "md" => Some("markdown".to_string()),
        "metal" => Some("metal".to_string()),
        "nim" => Some("nim".to_string()),
        "nix" => Some("nix".to_string()),
        "ml" | "mli" => Some("ocaml".to_string()),
        "odin" => Some("odin".to_string()),
        "php" => Some("php".to_string()),
        "prettier.config.cjs"
        | "prettier.config.js"
        | "prettier.config.mjs"
        | "prettierignore"
        | "prettierrc"
        | "prettierrc.cjs"
        | "prettierrc.js"
        | "prettierrc.json"
        | "prettierrc.json5"
        | "prettierrc.mjs"
        | "prettierrc.toml"
        | "prettierrc.yaml"
        | "prettierrc.yml" => Some("prettier".to_string()),
        "prisma" => Some("prisma".to_string()),
        "pp" => Some("puppet".to_string()),
        "py" => Some("python".to_string()),
        "r" => Some("r".to_string()),
        "cjsx" | "ctsx" | "jsx" | "mjsx" | "mtsx" | "tsx" => Some("react".to_string()),
        "roc" => Some("roc".to_string()),
        "rb" => Some("ruby".to_string()),
        "rs" => Some("rust".to_string()),
        "sass" | "scss" => Some("sass".to_string()),
        "scala" | "sc" => Some("scala".to_string()),
        "conf" | "ini" | "yaml" | "yml" => Some("settings".to_string()),
        "sol" => Some("solidity".to_string()),
        "accdb" | "csv" | "dat" | "db" | "dbf" | "dll" | "fmp" | "fp7" | "frm" | "gdb" | "ib"
        | "ldf" | "mdb" | "mdf" | "myd" | "myi" | "pdb" | "rdata" | "sav" | "sdf" | "sql"
        | "sqlite" | "tsv" => Some("storage".to_string()),
        "stylelint.config.cjs"
        | "stylelint.config.js"
        | "stylelint.config.mjs"
        | "stylelintignore"
        | "stylelintrc"
        | "stylelintrc.cjs"
        | "stylelintrc.js"
        | "stylelintrc.json"
        | "stylelintrc.mjs"
        | "stylelintrc.yaml"
        | "stylelintrc.yml" => Some("stylelint".to_string()),
        "surql" => Some("surrealql".to_string()),
        "svelte" => Some("svelte".to_string()),
        "swift" => Some("swift".to_string()),
        "tcl" => Some("tcl".to_string()),
        "hbs" | "plist" | "xml" => Some("template".to_string()),
        "bash" | "bash_aliases" | "bash_login" | "bash_logout" | "bash_profile" | "bashrc"
        | "fish" | "nu" | "profile" | "ps1" | "sh" | "zlogin" | "zlogout" | "zprofile" | "zsh"
        | "zsh_aliases" | "zsh_histfile" | "zsh_history" | "zshenv" | "zshrc" => {
            Some("terminal".to_string())
        }
        "tf" | "tfvars" => Some("terraform".to_string()),
        "toml" => Some("toml".to_string()),
        "cts" | "mts" | "ts" => Some("typescript".to_string()),
        "v" | "vsh" | "vv" => Some("v".to_string()),
        "commit_editmsg" | "edit_description" | "merge_msg" | "notes_editmsg" | "tag_editmsg"
        | "gitattributes" | "gitignore" | "gitkeep" | "gitmodules" => Some("vcs".to_string()),
        "vbproj" => Some("vbproj".to_string()),
        "avi" | "m4v" | "mkv" | "mov" | "mp4" | "webm" | "wmv" => Some("video".to_string()),
        "sln" => Some("vs_sln".to_string()),
        "suo" => Some("vs_suo".to_string()),
        "vue" => Some("vue".to_string()),
        "vy" | "vyi" => Some("vyper".to_string()),
        "wgsl" => Some("wgsl".to_string()),
        "zig" => Some("zig".to_string()),
        _ => None,
    }
}

fn icon_for_key(key: &str) -> &'static str {
    match key {
        "directory" => "\u{f07b}",    // nf-fa-folder
        "symlink" => "\u{f0c1}",      // nf-fa-link
        "executable" => "\u{f489}",   // nf-oct-terminal
        "astro" => "\u{e6b3}",        // nf-seti-astro
        "audio" => "\u{f001}",        // nf-fa-music
        "backup" => "\u{f0c7}",       // nf-fa-save
        "bicep" => "\u{ebd8}",        // nf-md-arm_flex
        "bun" => "\u{e76f}",          // nf-dev-javascript_badge
        "c" => "\u{e61e}",            // nf-custom-c
        "cairo" => "\u{f15c}",        // nf-fa-file_text
        "code" => "\u{f121}",         // nf-fa-code
        "coffeescript" => "\u{e751}", // nf-dev-coffeescript
        "cpp" => "\u{e61d}",          // nf-custom-cpp
        "crystal" => "\u{e7a3}",      // nf-dev-ruby_rough
        "csharp" => "\u{f81a}",       // nf-mdi-language_csharp
        "csproj" => "\u{f81a}",       // nf-mdi-language_csharp
        "css" => "\u{e749}",          // nf-dev-css3
        "cue" => "\u{f15c}",          // nf-fa-file_text
        "dart" => "\u{e798}",         // nf-dev-dart
        "diff" => "\u{f440}",         // nf-oct-diff
        "docker" => "\u{f308}",       // nf-linux-docker
        "document" => "\u{f15c}",     // nf-fa-file_text
        "elixir" => "\u{e62d}",       // nf-custom-elixir
        "elm" => "\u{e62c}",          // nf-custom-elm
        "erlang" => "\u{e7b1}",       // nf-dev-erlang
        "eslint" => "\u{e799}",       // nf-dev-eslint
        "font" => "\u{f031}",         // nf-fa-font
        "fsharp" => "\u{e7a7}",       // nf-dev-fsharp
        "fsproj" => "\u{e7a7}",       // nf-dev-fsharp
        "gitlab" => "\u{f296}",       // nf-fa-gitlab
        "gleam" => "\u{f0e7}",        // nf-fa-bolt
        "go" => "\u{e626}",           // nf-custom-go
        "graphql" => "\u{e662}",      // nf-custom-graphql
        "haskell" => "\u{e777}",      // nf-dev-haskell
        "hcl" => "\u{f1bb}",          // nf-fa-file_code_o
        "heroku" => "\u{e77b}",       // nf-dev-heroku
        "html" => "\u{e736}",         // nf-dev-html5
        "image" => "\u{f03e}",        // nf-fa-image
        "java" => "\u{e738}",         // nf-dev-java
        "javascript" => "\u{e74e}",   // nf-dev-javascript
        "json" => "\u{e60b}",         // nf-seti-json
        "julia" => "\u{e624}",        // nf-custom-julia
        "kdl" => "\u{f15c}",          // nf-fa-file_text
        "kotlin" => "\u{e634}",       // nf-custom-kotlin
        "lock" => "\u{f023}",         // nf-fa-lock
        "log" => "\u{f0f6}",          // nf-fa-file_text_o
        "lua" => "\u{e620}",          // nf-custom-lua
        "luau" => "\u{e620}",         // nf-custom-lua
        "markdown" => "\u{e73e}",     // nf-dev-markdown
        "metal" => "\u{f121}",        // nf-fa-code
        "nim" => "\u{f6a4}",          // nf-mdi-crown
        "nix" => "\u{f313}",          // nf-linux-nixos
        "ocaml" => "\u{e67a}",        // nf-seti-ocaml
        "odin" => "\u{f121}",         // nf-fa-code
        "phoenix" => "\u{e62d}",      // nf-custom-elixir
        "php" => "\u{e73d}",          // nf-dev-php
        "prettier" => "\u{e6b4}",     // nf-seti-prettier
        "prisma" => "\u{e684}",       // nf-seti-prisma
        "puppet" => "\u{e631}",       // nf-custom-puppet
        "python" => "\u{e73c}",       // nf-dev-python
        "r" => "\u{f25d}",            // nf-fa-registered
        "react" => "\u{e7ba}",        // nf-dev-react
        "roc" => "\u{f121}",          // nf-fa-code
        "ruby" => "\u{e791}",         // nf-dev-ruby
        "rust" => "\u{e7a8}",         // nf-dev-rust
        "sass" => "\u{e74b}",         // nf-dev-sass
        "scala" => "\u{e737}",        // nf-dev-scala
        "settings" => "\u{f013}",     // nf-fa-gear
        "solidity" => "\u{f0ac}",     // nf-fa-globe
        "storage" => "\u{f1c0}",      // nf-fa-database
        "stylelint" => "\u{e799}",    // nf-dev-eslint
        "surrealql" => "\u{f1c0}",    // nf-fa-database
        "svelte" => "\u{e697}",       // nf-seti-svelte
        "swift" => "\u{e755}",        // nf-dev-swift
        "tcl" => "\u{f121}",          // nf-fa-code
        "template" => "\u{f1c9}",     // nf-fa-file_code_o
        "terminal" => "\u{f120}",     // nf-fa-terminal
        "terraform" => "\u{e69a}",    // nf-seti-terraform
        "toml" => "\u{e6b2}",         // nf-seti-toml
        "typescript" => "\u{e628}",   // nf-seti-typescript
        "v" => "\u{f121}",            // nf-fa-code
        "vbproj" => "\u{f121}",       // nf-fa-code
        "vcs" => "\u{f1d3}",          // nf-fa-git_square
        "video" => "\u{f03d}",        // nf-fa-video_camera
        "vs_sln" => "\u{e70c}",       // nf-dev-visualstudio
        "vs_suo" => "\u{e70c}",       // nf-dev-visualstudio
        "vue" => "\u{e6a0}",          // nf-seti-vue
        "vyper" => "\u{f0ac}",        // nf-fa-globe
        "wgsl" => "\u{f121}",         // nf-fa-code
        "zig" => "\u{f0e7}",          // nf-fa-bolt
        _ => "\u{f15c}",              // nf-fa-file_text
    }
}

fn color_for_key(key: &str) -> Color {
    match key {
        "directory" => Color::Blue,
        "symlink" => Color::Cyan,
        "executable" => Color::Green,
        "audio" | "video" => Color::Magenta,
        "image" => Color::Yellow,
        "document" | "markdown" => Color::Yellow,
        "json" | "yaml" | "toml" | "settings" => Color::Yellow,
        "rust" | "go" | "python" | "javascript" | "typescript" | "cpp" | "c" | "java"
        | "kotlin" | "csharp" | "ruby" | "php" | "swift" => Color::Green,
        "gitlab" | "vcs" => Color::Red,
        "lock" => Color::DarkGray,
        "log" => Color::DarkGray,
        _ => Color::White,
    }
}

fn color_for_status(status: &str) -> Color {
    let chars: Vec<char> = status.chars().collect();
    let x = chars.first().copied().unwrap_or(' ');
    let y = chars.get(1).copied().unwrap_or(' ');

    if x == '?' || y == '?' {
        return Color::Yellow;
    }
    if x == 'D' || y == 'D' {
        return Color::Red;
    }
    if x == 'A' || y == 'A' {
        return Color::Green;
    }
    if x == 'M' || y == 'M' {
        return Color::Blue;
    }
    Color::DarkGray
}

fn format_modified(meta: &fs::Metadata) -> String {
    match meta.modified() {
        Ok(time) => {
            let dt: DateTime<Local> = time.into();
            dt.format("%Y-%m-%d %H:%M").to_string()
        }
        Err(_) => String::new(),
    }
}

fn format_git_metrics(git: &GitStatus) -> String {
    if git.ahead == 0
        && git.behind == 0
        && git.counts.staged == 0
        && git.counts.unstaged == 0
        && git.counts.untracked == 0
    {
        return String::new();
    }

    format!(
        "↑{} ↓{} S{} U{} ?{}",
        git.ahead, git.behind, git.counts.staged, git.counts.unstaged, git.counts.untracked
    )
}

fn should_color() -> bool {
    env::var_os("NO_COLOR").is_none()
}

fn is_ignored(gitignore: &Option<Gitignore>, path: &Path, is_dir: bool) -> bool {
    // Check root gitignore first
    if let Some(ignore) = gitignore {
        if ignore.matched_path_or_any_parents(path, is_dir).is_ignore() {
            return true;
        }
    }

    // Check for nested .gitignore files in parent directories
    let file_name = match path.file_name().and_then(|n| n.to_str()) {
        Some(name) => name,
        None => return false,
    };

    // Walk up the directory tree looking for .gitignore files
    let mut current = path.parent();
    while let Some(dir) = current {
        let gitignore_path = dir.join(".gitignore");
        if gitignore_path.is_file() {
            if let Ok(contents) = fs::read_to_string(&gitignore_path) {
                for line in contents.lines() {
                    let line = line.trim();
                    // Skip comments and empty lines
                    if line.is_empty() || line.starts_with('#') {
                        continue;
                    }
                    // Handle /pattern (anchored to this directory)
                    let pattern = line.trim_start_matches('/').trim_end_matches('/');
                    if pattern == file_name {
                        return true;
                    }
                }
            }
        }
        current = dir.parent();
    }

    false
}

struct GitCounts {
    staged: usize,
    unstaged: usize,
    untracked: usize,
}

struct GitStatus {
    map: HashMap<PathBuf, String>,
    counts: GitCounts,
    ahead: usize,
    behind: usize,
}

fn load_git_status(root: &Path) -> GitStatus {
    let git_root = match git_toplevel(root) {
        Some(root) => root,
        None => {
            return GitStatus {
                map: HashMap::new(),
                counts: GitCounts {
                    staged: 0,
                    unstaged: 0,
                    untracked: 0,
                },
                ahead: 0,
                behind: 0,
            }
        }
    };

    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(&git_root)
        .arg("status")
        .arg("--porcelain")
        .arg("-z")
        .output();

    let output = match output {
        Ok(output) if output.status.success() => output,
        _ => {
            return GitStatus {
                map: HashMap::new(),
                counts: GitCounts {
                    staged: 0,
                    unstaged: 0,
                    untracked: 0,
                },
                ahead: 0,
                behind: 0,
            }
        }
    };

    let mut map = HashMap::new();
    let mut counts = GitCounts {
        staged: 0,
        unstaged: 0,
        untracked: 0,
    };
    let bytes = output.stdout;
    let mut idx = 0;
    while idx + 3 <= bytes.len() {
        let x = bytes[idx] as char;
        let y = bytes[idx + 1] as char;
        idx += 2;
        if idx < bytes.len() && bytes[idx] == b' ' {
            idx += 1;
        }

        let path1 = read_c_string(&bytes, &mut idx);
        if path1.as_os_str().is_empty() {
            continue;
        }

        let mut path = path1.clone();
        if x == 'R' || x == 'C' || y == 'R' || y == 'C' {
            let path2 = read_c_string(&bytes, &mut idx);
            if !path2.as_os_str().is_empty() {
                path = path2;
            }
        }

        let abs = git_root.join(&path);
        if x == '?' && y == '?' {
            counts.untracked += 1;
        } else {
            if x != ' ' {
                counts.staged += 1;
            }
            if y != ' ' {
                counts.unstaged += 1;
            }
        }

        if abs.starts_with(root) {
            map.insert(abs, format_status(x, y));
        }
    }

    let (ahead, behind) = git_ahead_behind(&git_root);

    GitStatus {
        map,
        counts,
        ahead,
        behind,
    }
}

fn git_ahead_behind(git_root: &Path) -> (usize, usize) {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(git_root)
        .arg("status")
        .arg("-sb")
        .output();

    let output = match output {
        Ok(output) if output.status.success() => output,
        _ => return (0, 0),
    };

    let text = String::from_utf8_lossy(&output.stdout);
    let first = text.lines().next().unwrap_or("");
    let mut ahead = 0;
    let mut behind = 0;
    if let Some(start) = first.find('[') {
        if let Some(end) = first[start + 1..].find(']') {
            let inner = &first[start + 1..start + 1 + end];
            for part in inner.split(',') {
                let part = part.trim();
                if let Some(value) = part.strip_prefix("ahead ") {
                    ahead = value.parse::<usize>().unwrap_or(0);
                } else if let Some(value) = part.strip_prefix("behind ") {
                    behind = value.parse::<usize>().unwrap_or(0);
                }
            }
        }
    }
    (ahead, behind)
}

fn git_toplevel(root: &Path) -> Option<PathBuf> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .arg("rev-parse")
        .arg("--show-toplevel")
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let text = String::from_utf8_lossy(&output.stdout);
    let path = PathBuf::from(text.trim());
    Some(fs::canonicalize(&path).unwrap_or(path))
}

fn read_c_string(bytes: &[u8], idx: &mut usize) -> PathBuf {
    let start = *idx;
    while *idx < bytes.len() && bytes[*idx] != 0 {
        *idx += 1;
    }
    let slice = &bytes[start..*idx];
    if *idx < bytes.len() && bytes[*idx] == 0 {
        *idx += 1;
    }
    PathBuf::from(String::from_utf8_lossy(slice).to_string())
}

fn format_status(x: char, y: char) -> String {
    if x == '?' && y == '?' {
        "??".to_string()
    } else {
        format!("{}{}", x, y)
    }
}

fn expand_changed_paths(
    root: &mut Node,
    root_path: &Path,
    gitignore: &Option<Gitignore>,
    git_status: &GitStatus,
) -> io::Result<()> {
    for path in git_status.map.keys() {
        if let Ok(rel) = path.strip_prefix(root_path) {
            expand_path(root, rel, gitignore, git_status)?;
        }
    }

    Ok(())
}

fn expand_path(
    node: &mut Node,
    rel_path: &Path,
    gitignore: &Option<Gitignore>,
    git_status: &GitStatus,
) -> io::Result<()> {
    let mut components = rel_path.components();
    let first = match components.next() {
        Some(component) => component,
        None => return Ok(()),
    };

    if node.is_dir {
        node.expanded = true;
        if node.children.is_none() {
            load_children(node, gitignore, git_status)?;
        }
        if let Some(children) = node.children.as_mut() {
            if let Some(child) = children
                .iter_mut()
                .find(|child| child.path.file_name() == Some(first.as_os_str()))
            {
                let rest = components.as_path();
                expand_path(child, rest, gitignore, git_status)?;
            }
        }
    }

    Ok(())
}

fn parse_ansi_lines(text: &str) -> Vec<StyledLine> {
    text.lines()
        .map(|line| StyledLine {
            spans: parse_ansi_spans(line),
        })
        .collect()
}

fn parse_ansi_spans(line: &str) -> Vec<StyledSpan> {
    let mut spans = Vec::new();
    let mut style = TextStyle::default();
    let mut buf = String::new();
    let mut chars = line.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '\u{001b}' {
            if let Some('[') = chars.peek().copied() {
                let _ = chars.next();
                let mut codes = String::new();
                for c in chars.by_ref() {
                    if c == 'm' {
                        break;
                    }
                    codes.push(c);
                }
                if !buf.is_empty() {
                    spans.push(StyledSpan {
                        text: buf.clone(),
                        style: style.clone(),
                    });
                    buf.clear();
                }
                apply_sgr(&codes, &mut style);
                continue;
            }
        }
        buf.push(ch);
    }

    if !buf.is_empty() {
        spans.push(StyledSpan { text: buf, style });
    }

    spans
}

fn apply_sgr(codes: &str, style: &mut TextStyle) {
    if codes.is_empty() {
        *style = TextStyle::default();
        return;
    }

    let parts = codes.split(';').filter(|p| !p.is_empty());
    let mut codes_vec = Vec::new();
    for part in parts {
        if let Ok(value) = part.parse::<u16>() {
            codes_vec.push(value);
        }
    }

    let mut i = 0;
    while i < codes_vec.len() {
        match codes_vec[i] {
            0 => *style = TextStyle::default(),
            1 => style.bold = true,
            2 => style.dim = true,
            3 => style.italic = true,
            4 => style.underline = true,
            22 => {
                style.bold = false;
                style.dim = false;
            }
            23 => style.italic = false,
            24 => style.underline = false,
            39 => style.fg = None,
            49 => style.bg = None,
            30..=37 | 90..=97 => style.fg = Some(ansi_color(codes_vec[i])),
            40..=47 | 100..=107 => style.bg = Some(ansi_color(codes_vec[i] - 10)),
            38 => {
                if let Some((color, consumed)) = parse_extended_color(&codes_vec[i + 1..]) {
                    style.fg = Some(color);
                    i += consumed;
                }
            }
            48 => {
                if let Some((color, consumed)) = parse_extended_color(&codes_vec[i + 1..]) {
                    style.bg = Some(color);
                    i += consumed;
                }
            }
            _ => {}
        }
        i += 1;
    }
}

fn parse_extended_color(codes: &[u16]) -> Option<(Color, usize)> {
    match codes.first().copied() {
        Some(5) => codes.get(1).map(|n| (Color::Indexed(*n as u8), 2)),
        Some(2) => {
            if codes.len() >= 4 {
                Some((
                    Color::Rgb(codes[1] as u8, codes[2] as u8, codes[3] as u8),
                    4,
                ))
            } else {
                None
            }
        }
        _ => None,
    }
}

fn ansi_color(code: u16) -> Color {
    match code {
        30 => Color::Black,
        31 => Color::Red,
        32 => Color::Green,
        33 => Color::Yellow,
        34 => Color::Blue,
        35 => Color::Magenta,
        36 => Color::Cyan,
        37 => Color::Gray,
        90 => Color::DarkGray,
        91 => Color::LightRed,
        92 => Color::LightGreen,
        93 => Color::LightYellow,
        94 => Color::LightBlue,
        95 => Color::LightMagenta,
        96 => Color::LightCyan,
        97 => Color::White,
        _ => Color::White,
    }
}
