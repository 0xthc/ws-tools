use super::get_session_name;
use crate::git;
use crate::tmux;
use anyhow::{Context, Result};
use crossterm::{
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind, MouseEventKind,
    },
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Color as RatColor, Modifier, Style},
    text::{Line, Span},
    widgets::{
        Block, Borders, Cell as RatCell, Clear, Paragraph, Row, Table as RatTable, TableState,
    },
    Frame, Terminal,
};
use std::io::stdout;
use std::path::PathBuf;
use std::time::Instant;

// ============================================================================
// Plasma Animation (Reaction-Diffusion)
// ============================================================================

/// Workspace metrics used to create unique plasma patterns
#[derive(Clone)]
struct WorkspaceMetrics {
    repo_name: String,
    num_commits: usize,
    active_sessions: usize,
}

impl Default for WorkspaceMetrics {
    fn default() -> Self {
        Self {
            repo_name: "default".to_string(),
            num_commits: 100,
            active_sessions: 1,
        }
    }
}

impl WorkspaceMetrics {
    fn name_hash(&self) -> u64 {
        self.repo_name
            .bytes()
            .fold(0u64, |acc, b| acc.wrapping_mul(31).wrapping_add(b as u64))
    }

    fn from_git_root(git_root: &std::path::Path) -> Self {
        let repo_name = git_root
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "unknown".to_string());

        // Count commits (quick estimate)
        let num_commits = std::process::Command::new("git")
            .current_dir(git_root)
            .args(["rev-list", "--count", "HEAD"])
            .output()
            .ok()
            .and_then(|o| String::from_utf8_lossy(&o.stdout).trim().parse().ok())
            .unwrap_or(100);

        // Count active sessions for this repo
        let active_sessions = crate::tmux::get_active_sessions()
            .iter()
            .filter(|s| s.starts_with(&format!("{}-", repo_name)))
            .count()
            .max(1);

        Self {
            repo_name,
            num_commits,
            active_sessions,
        }
    }
}

struct ReactionDiffusion {
    width: usize,
    height: usize,
    u: Vec<Vec<f64>>,
    v: Vec<Vec<f64>>,
    du: f64,
    dv: f64,
    f: f64,
    k: f64,
    time: f64,
    pulse_speed: f64,
    num_seeds: usize,
    seed_positions: Vec<(usize, usize)>,
    metrics: WorkspaceMetrics,
}

impl ReactionDiffusion {
    fn new(width: usize, height: usize) -> Self {
        Self::with_metrics(width, height, WorkspaceMetrics::default())
    }

    fn with_metrics(width: usize, height: usize, metrics: WorkspaceMetrics) -> Self {
        let hash = metrics.name_hash();

        // Derive parameters from metrics
        let f = 0.030 + ((hash % 20) as f64) * 0.001;
        let k = 0.057 + (((hash >> 8) % 15) as f64) * 0.001;
        let du = 0.14 + ((hash >> 16) % 5) as f64 * 0.01;
        let dv = 0.06 + ((hash >> 24) % 4) as f64 * 0.01;

        // Pulse speed based on commit activity
        let pulse_speed = 0.06 + (metrics.num_commits.min(1000) as f64 / 1000.0) * 0.04;

        // Number of seed points based on active sessions
        let num_seeds = metrics.active_sessions.clamp(1, 5);

        let mut seed_positions = Vec::new();
        let cx = width / 2;
        let cy = height / 2;

        if num_seeds == 1 {
            seed_positions.push((cx, cy));
        } else {
            let angle_offset = ((hash >> 32) % 360) as f64 * std::f64::consts::PI / 180.0;
            let radius = (width.min(height) / 4) as f64;

            for i in 0..num_seeds {
                let angle =
                    angle_offset + (i as f64 * 2.0 * std::f64::consts::PI / num_seeds as f64);
                let sx = (cx as f64 + angle.cos() * radius * 0.5) as usize;
                let sy = (cy as f64 + angle.sin() * radius * 0.25) as usize;
                seed_positions.push((
                    sx.clamp(1, width.saturating_sub(2)),
                    sy.clamp(1, height.saturating_sub(2)),
                ));
            }
        }

        let mut u = vec![vec![1.0; width]; height];
        let mut v = vec![vec![0.0; width]; height];

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

    fn resize(&mut self, width: usize, height: usize) {
        if self.width != width || self.height != height {
            *self = Self::with_metrics(width, height, self.metrics.clone());
        }
    }

    fn laplacian(grid: &[Vec<f64>], x: usize, y: usize, width: usize, height: usize) -> f64 {
        let x_prev = if x == 0 { width - 1 } else { x - 1 };
        let x_next = if x == width - 1 { 0 } else { x + 1 };
        let y_prev = if y == 0 { height - 1 } else { y - 1 };
        let y_next = if y == height - 1 { 0 } else { y + 1 };
        grid[y_prev][x] + grid[y_next][x] + grid[y][x_prev] + grid[y][x_next] - 4.0 * grid[y][x]
    }

    fn step(&mut self) {
        let mut new_u = self.u.clone();
        let mut new_v = self.v.clone();

        self.time += self.pulse_speed;
        let pulse = (self.time.sin() * 0.5 + 0.5) * 0.01;
        let f = self.f + pulse;
        let k = self.k - pulse * 0.5;

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

                new_u[y][x] = u + self.du * lap_u - uvv + f * (1.0 - u);
                new_v[y][x] = v + self.dv * lap_v + uvv - (f + k) * v;

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

                new_u[y][x] = new_u[y][x].clamp(0.0, 1.0);
                new_v[y][x] = new_v[y][x].clamp(0.0, 1.0);
            }
        }

        self.u = new_u;
        self.v = new_v;
    }

    fn render(&self) -> Vec<String> {
        let chars = [' ', '·', '-', '=', '+', '*', '#', '@'];
        self.v
            .iter()
            .map(|row| {
                row.iter()
                    .map(|&val| {
                        let idx = (val * (chars.len() - 1) as f64).round() as usize;
                        chars[idx.min(chars.len() - 1)]
                    })
                    .collect()
            })
            .collect()
    }
}

// ============================================================================
// Status Types and App
// ============================================================================

/// Action to perform after exiting the TUI (only actions that require TUI exit)
#[derive(Clone)]
pub enum StatusAction {
    None,
    Open(PathBuf),
    Ai,            // Needs to run its own TUI
    ReviewPr(u32), // Review a PR by number
}

/// Entry representing a worktree with its session info
#[derive(Clone)]
struct WorktreeEntry {
    session: String,
    branch: String,
    path: PathBuf,
    is_main: bool,
    has_session: bool,
}

/// Input mode for the status app
#[derive(PartialEq)]
enum InputMode {
    Normal,
    NewBranch,
    ConfirmDelete,
    SyncMenu,
    PrMenu,
    Help,
}

/// Result from a background task
enum TaskResult {
    DeleteWorktree {
        branch: String,
        success: bool,
        error: Option<String>,
    },
    Gc {
        deleted: usize,
    },
    SyncCreate {
        created: usize,
    },
    SyncDelete {
        deleted: usize,
    },
}

/// Status application state
struct StatusApp {
    entries: Vec<WorktreeEntry>,
    table_state: TableState,
    orphaned_sessions: Vec<String>,
    orphaned_worktrees: Vec<String>,
    repo_name: String,
    git_root: PathBuf,
    should_exit: bool,
    action: StatusAction,
    input_mode: InputMode,
    input_buffer: String,
    message: Option<(String, bool)>, // (message, is_error)
    task_receiver: Option<std::sync::mpsc::Receiver<TaskResult>>,
    is_busy: bool,
}

impl StatusApp {
    fn new() -> Result<Self> {
        let git_root = git::get_root(None).context("Not in a git repository")?;
        let repo_name = git_root
            .file_name()
            .context("Invalid git root")?
            .to_string_lossy()
            .to_string();

        // Fetch from origin to ensure we're in sync (run in background, don't block)
        let _ = std::process::Command::new("git")
            .current_dir(&git_root)
            .args(["fetch", "--all", "--prune", "-q"])
            .spawn();

        let mut app = Self {
            entries: Vec::new(),
            table_state: TableState::default(),
            orphaned_sessions: Vec::new(),
            orphaned_worktrees: Vec::new(),
            repo_name,
            git_root,
            should_exit: false,
            action: StatusAction::None,
            input_mode: InputMode::Normal,
            input_buffer: String::new(),
            message: None,
            task_receiver: None,
            is_busy: false,
        };
        app.refresh();
        Ok(app)
    }

    /// Refresh worktree and session data
    fn refresh(&mut self) {
        let worktrees = match git::list_worktrees(&self.git_root) {
            Ok(wt) => wt,
            Err(_) => return,
        };
        let active_sessions = tmux::get_active_sessions();

        let mut entries: Vec<WorktreeEntry> = Vec::new();
        let mut worktree_sessions: std::collections::HashSet<String> =
            std::collections::HashSet::new();

        for wt in &worktrees {
            if let Ok(session_name) = get_session_name(&wt.path) {
                let has_session = active_sessions.contains(&session_name);
                let is_main = wt.path == self.git_root;

                if has_session {
                    worktree_sessions.insert(session_name.clone());
                }

                entries.push(WorktreeEntry {
                    session: session_name,
                    branch: wt.branch.clone(),
                    path: wt.path.clone(),
                    is_main,
                    has_session,
                });
            }
        }

        // Find orphaned sessions: sessions that belong to this repo but don't have worktrees
        // Sessions are named: {worktree_dir_name}-{sanitized_branch}
        let repo_prefix = format!("{}-", self.repo_name);
        let workspaces_dir = super::get_workspaces_dir()
            .ok()
            .map(|p| p.join(&self.repo_name));

        // Get all directories in the workspaces folder (existing worktree dirs)
        let workspace_dirs: std::collections::HashSet<String> = workspaces_dir
            .and_then(|dir| dir.read_dir().ok())
            .map(|entries| {
                entries
                    .filter_map(|e| e.ok())
                    .filter(|e| e.path().is_dir())
                    .filter_map(|e| e.file_name().to_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();

        // Get all remote branches to help identify orphaned sessions
        let remote_branches: std::collections::HashSet<String> = std::process::Command::new("git")
            .current_dir(&self.git_root)
            .args(["branch", "-r", "--format=%(refname:short)"])
            .output()
            .ok()
            .map(|o| {
                String::from_utf8_lossy(&o.stdout)
                    .lines()
                    .map(|s| s.trim_start_matches("origin/"))
                    .map(|s| crate::git::sanitize_branch(s))
                    .collect()
            })
            .unwrap_or_default();

        self.orphaned_sessions = active_sessions
            .iter()
            .filter(|session| {
                // Skip if this session is already tracked by a worktree
                if worktree_sessions.contains(*session) {
                    return false;
                }

                // Check if session starts with repo name (main worktree pattern)
                if session.starts_with(&repo_prefix) {
                    return true;
                }

                // Check if session starts with any workspace directory name
                // Session format: {dir_name}-{branch}
                for dir_name in &workspace_dirs {
                    if session.starts_with(&format!("{}-", dir_name)) {
                        return true;
                    }
                }

                // Check if session ends with a known branch name from this repo
                // This catches sessions for deleted worktrees
                for branch in &remote_branches {
                    if session.ends_with(&format!("-{}", branch)) {
                        return true;
                    }
                }

                false
            })
            .cloned()
            .collect();

        self.orphaned_worktrees = worktrees
            .iter()
            .filter(|wt| {
                if wt.path == self.git_root {
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

        // Preserve selection if possible
        let old_selection = self.table_state.selected();
        self.entries = entries;

        if !self.entries.is_empty() {
            let new_selection = old_selection
                .map(|i| i.min(self.entries.len() - 1))
                .or(Some(0));
            self.table_state.select(new_selection);
        } else {
            self.table_state.select(None);
        }
    }

    fn has_orphans(&self) -> bool {
        !self.orphaned_sessions.is_empty() || !self.orphaned_worktrees.is_empty()
    }

    fn selected_entry(&self) -> Option<&WorktreeEntry> {
        self.table_state
            .selected()
            .and_then(|i| self.entries.get(i))
    }

    /// Check for completed background tasks
    fn poll_tasks(&mut self) {
        if let Some(ref receiver) = self.task_receiver {
            match receiver.try_recv() {
                Ok(result) => {
                    self.is_busy = false;
                    self.task_receiver = None;
                    match result {
                        TaskResult::DeleteWorktree {
                            branch,
                            success,
                            error,
                        } => {
                            if success {
                                self.message = Some((format!("Deleted '{}'", branch), false));
                            } else {
                                self.message =
                                    Some((format!("Error: {}", error.unwrap_or_default()), true));
                            }
                        }
                        TaskResult::Gc { deleted } => {
                            if deleted > 0 {
                                self.message = Some((
                                    format!("Cleaned {} merged worktree(s)", deleted),
                                    false,
                                ));
                            } else {
                                self.message =
                                    Some(("No merged worktrees to clean".to_string(), false));
                            }
                        }
                        TaskResult::SyncCreate { created } => {
                            self.message = Some((format!("Created {} session(s)", created), false));
                        }
                        TaskResult::SyncDelete { deleted } => {
                            self.message =
                                Some((format!("Deleted {} worktree(s)", deleted), false));
                        }
                    }
                    self.refresh();
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    // Still working
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.is_busy = false;
                    self.task_receiver = None;
                    self.message = Some(("Task failed unexpectedly".to_string(), true));
                }
            }
        }
    }

    // ========================================================================
    // Inline Action Execution
    // ========================================================================

    fn exec_new_worktree(&mut self, branch: &str) {
        use super::get_workspaces_dir;
        use std::process::Command;

        let base = git::get_default_branch(Some(&self.git_root));
        let branch_safe = git::sanitize_branch(branch);

        let wt_path = match get_workspaces_dir() {
            Ok(dir) => dir.join(&self.repo_name).join(&branch_safe),
            Err(e) => {
                self.message = Some((format!("Error: {}", e), true));
                return;
            }
        };

        if wt_path.exists() {
            self.message = Some((format!("Worktree '{}' already exists", branch), true));
            return;
        }

        // Create worktree
        let output = Command::new("git")
            .current_dir(&self.git_root)
            .args([
                "worktree",
                "add",
                "-b",
                branch,
                wt_path.to_str().unwrap(),
                &base,
            ])
            .output();

        match output {
            Ok(out) if out.status.success() => {
                self.message = Some((format!("Created worktree '{}'", branch), false));
                self.refresh();
            }
            Ok(out) => {
                let err = String::from_utf8_lossy(&out.stderr);
                self.message = Some((format!("Error: {}", err.trim()), true));
            }
            Err(e) => {
                self.message = Some((format!("Error: {}", e), true));
            }
        }
    }

    fn exec_delete_worktree(&mut self, path: PathBuf, force: bool) {
        use std::process::{Command, Stdio};
        use std::sync::mpsc;
        use std::thread;

        if self.is_busy {
            self.message = Some((
                "Please wait for current operation to complete".to_string(),
                true,
            ));
            return;
        }

        let branch = self
            .entries
            .iter()
            .find(|e| e.path == path)
            .map(|e| e.branch.clone())
            .unwrap_or_default();

        // Kill tmux session if exists (but not if we're inside it)
        if let Ok(session) = get_session_name(&path) {
            if tmux::session_exists(&session) {
                // Check if we're currently in this session
                let current_session = Command::new("tmux")
                    .args(["display-message", "-p", "#{session_name}"])
                    .stdout(Stdio::piped())
                    .stderr(Stdio::null())
                    .output()
                    .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                    .unwrap_or_default();

                if current_session == session {
                    self.message =
                        Some(("Cannot delete current session from TUI".to_string(), true));
                    return;
                }
                let _ = tmux::kill_session(&session);
            }
        }

        // Spawn background thread for worktree removal
        let (tx, rx) = mpsc::channel();
        let git_root = self.git_root.clone();
        let branch_clone = branch.clone();
        let path_clone = path.clone();

        self.is_busy = true;
        self.task_receiver = Some(rx);
        self.message = Some((format!("Deleting '{}'...", branch), false));

        thread::spawn(move || {
            let mut args = vec!["worktree", "remove"];
            if force {
                args.push("--force");
            }
            let path_str = path_clone.to_string_lossy().to_string();
            args.push(&path_str);

            let output = Command::new("git")
                .current_dir(&git_root)
                .args(&args)
                .output();

            let result = match output {
                Ok(out) if out.status.success() => {
                    // Also delete branch
                    let _ = Command::new("git")
                        .current_dir(&git_root)
                        .args(["branch", "-d", &branch_clone])
                        .output();
                    TaskResult::DeleteWorktree {
                        branch: branch_clone,
                        success: true,
                        error: None,
                    }
                }
                Ok(out) => {
                    let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
                    TaskResult::DeleteWorktree {
                        branch: branch_clone,
                        success: false,
                        error: Some(err),
                    }
                }
                Err(e) => TaskResult::DeleteWorktree {
                    branch: branch_clone,
                    success: false,
                    error: Some(e.to_string()),
                },
            };
            let _ = tx.send(result);
        });
    }

    fn exec_reload_session(&mut self, path: PathBuf) {
        let session = match get_session_name(&path) {
            Ok(s) => s,
            Err(e) => {
                self.message = Some((format!("Error: {}", e), true));
                return;
            }
        };

        // Kill existing session
        if tmux::session_exists(&session) {
            if let Err(e) = tmux::kill_session(&session) {
                self.message = Some((format!("Error killing session: {}", e), true));
                return;
            }
        }

        // Create new session
        let window_title = match super::get_window_title(&path) {
            Ok(t) => t,
            Err(_) => session.clone(),
        };

        match tmux::create_session_with_title(&session, &path, &window_title) {
            Ok(_) => {
                self.message = Some((format!("Reloaded session '{}'", session), false));
                self.refresh();
            }
            Err(e) => {
                self.message = Some((format!("Error: {}", e), true));
            }
        }
    }

    fn exec_sync_create(&mut self) {
        let mut created = 0;

        for entry in &self.entries {
            if !entry.has_session && !entry.is_main {
                let window_title =
                    super::get_window_title(&entry.path).unwrap_or_else(|_| entry.session.clone());
                if tmux::create_session_with_title(&entry.session, &entry.path, &window_title)
                    .is_ok()
                {
                    created += 1;
                }
            }
        }

        self.message = Some((format!("Created {} session(s)", created), false));
        self.refresh();
    }

    fn exec_sync_delete(&mut self) {
        use std::process::Command;

        let mut deleted = 0;

        // Delete orphaned worktrees (those without sessions, excluding main)
        let to_delete: Vec<_> = self
            .entries
            .iter()
            .filter(|e| !e.has_session && !e.is_main)
            .map(|e| (e.path.clone(), e.branch.clone()))
            .collect();

        for (path, branch) in to_delete {
            let output = Command::new("git")
                .current_dir(&self.git_root)
                .args(["worktree", "remove", path.to_str().unwrap()])
                .output();

            if output.map(|o| o.status.success()).unwrap_or(false) {
                let _ = Command::new("git")
                    .current_dir(&self.git_root)
                    .args(["branch", "-d", &branch])
                    .output();
                deleted += 1;
            }
        }

        // Kill orphaned sessions
        for session in &self.orphaned_sessions {
            let _ = tmux::kill_session(session);
        }

        self.message = Some((
            format!("Deleted {} worktree(s), cleaned orphaned sessions", deleted),
            false,
        ));
        self.refresh();
    }

    fn exec_gc(&mut self) {
        use std::process::Command;

        let default_branch = git::get_default_branch(Some(&self.git_root));

        // Find merged branches
        let output = Command::new("git")
            .current_dir(&self.git_root)
            .args(["branch", "--merged", &format!("origin/{}", default_branch)])
            .output();

        let merged_branches: std::collections::HashSet<String> = match output {
            Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout)
                .lines()
                .map(|l| {
                    l.trim()
                        .trim_start_matches("* ")
                        .trim_start_matches("+ ")
                        .to_string()
                })
                .filter(|b| !b.is_empty() && b != &default_branch && !b.starts_with("remotes/"))
                .collect(),
            _ => {
                self.message = Some(("Error getting merged branches".to_string(), true));
                return;
            }
        };

        let mut deleted = 0;

        for entry in self.entries.clone() {
            if !entry.is_main && merged_branches.contains(&entry.branch) {
                // Kill session
                if let Ok(session) = get_session_name(&entry.path) {
                    if tmux::session_exists(&session) {
                        let _ = tmux::kill_session(&session);
                    }
                }

                // Remove worktree
                let _ = Command::new("git")
                    .current_dir(&self.git_root)
                    .args([
                        "worktree",
                        "remove",
                        "--force",
                        entry.path.to_str().unwrap(),
                    ])
                    .output();

                // Delete branch
                let _ = Command::new("git")
                    .current_dir(&self.git_root)
                    .args(["branch", "-D", &entry.branch])
                    .output();

                deleted += 1;
            }
        }

        // Prune worktrees
        let _ = Command::new("git")
            .current_dir(&self.git_root)
            .args(["worktree", "prune"])
            .output();

        if deleted > 0 {
            self.message = Some((format!("Cleaned {} merged worktree(s)", deleted), false));
        } else {
            self.message = Some(("No merged worktrees to clean".to_string(), false));
        }
        self.refresh();
    }

    fn exec_doctor(&mut self) {
        use crate::config::Config;

        let cfg = match Config::load() {
            Ok(c) => c,
            Err(e) => {
                self.message = Some((format!("Error loading config: {}", e), true));
                return;
            }
        };

        let mut missing = Vec::new();

        if !cfg.is_ai_tool_installed() {
            missing.push(cfg.ai_tool.name());
        }
        if !cfg.is_git_tool_installed() {
            missing.push(cfg.git_tool.name());
        }
        if !cfg.is_explorer_tool_installed() {
            missing.push(cfg.explorer_tool.name());
        }

        if missing.is_empty() {
            self.message = Some(("All tools installed".to_string(), false));
        } else {
            self.message = Some((format!("Missing: {}", missing.join(", ")), true));
        }
    }

    fn next_item(&mut self) {
        if self.entries.is_empty() {
            return;
        }
        let i = match self.table_state.selected() {
            Some(i) => {
                if i >= self.entries.len() - 1 {
                    0
                } else {
                    i + 1
                }
            }
            None => 0,
        };
        self.table_state.select(Some(i));
    }

    fn previous_item(&mut self) {
        if self.entries.is_empty() {
            return;
        }
        let i = match self.table_state.selected() {
            Some(i) => {
                if i == 0 {
                    self.entries.len() - 1
                } else {
                    i - 1
                }
            }
            None => 0,
        };
        self.table_state.select(Some(i));
    }

    fn open_selected(&mut self) {
        if let Some(entry) = self.selected_entry() {
            self.action = StatusAction::Open(entry.path.clone());
            self.should_exit = true;
        }
    }

    fn start_new_worktree(&mut self) {
        self.input_mode = InputMode::NewBranch;
        self.input_buffer.clear();
        self.message = None;
    }

    fn confirm_new_worktree(&mut self) {
        if !self.input_buffer.is_empty() {
            let branch = self.input_buffer.clone();
            self.input_mode = InputMode::Normal;
            self.input_buffer.clear();
            self.exec_new_worktree(&branch);
        } else {
            self.input_mode = InputMode::Normal;
            self.input_buffer.clear();
        }
    }

    fn start_delete(&mut self) {
        if let Some(entry) = self.selected_entry() {
            if entry.is_main {
                self.message = Some(("Cannot delete main worktree".to_string(), true));
                return;
            }
            self.input_mode = InputMode::ConfirmDelete;
            self.message = None;
        }
    }

    fn confirm_delete(&mut self, force: bool) {
        if let Some(entry) = self.selected_entry() {
            let path = entry.path.clone();
            self.input_mode = InputMode::Normal;
            self.exec_delete_worktree(path, force);
        } else {
            self.input_mode = InputMode::Normal;
        }
    }

    fn reload_selected(&mut self) {
        if let Some(entry) = self.selected_entry() {
            let path = entry.path.clone();
            self.exec_reload_session(path);
        }
    }

    fn show_sync_menu(&mut self) {
        self.input_mode = InputMode::SyncMenu;
        self.message = None;
    }

    fn show_pr_menu(&mut self) {
        self.input_mode = InputMode::PrMenu;
        self.message = None;
    }

    fn show_help(&mut self) {
        self.input_mode = InputMode::Help;
    }

    fn handle_key(&mut self, key: KeyCode) {
        match self.input_mode {
            InputMode::Normal => self.handle_normal_key(key),
            InputMode::NewBranch => self.handle_input_key(key),
            InputMode::ConfirmDelete => self.handle_delete_key(key),
            InputMode::SyncMenu => self.handle_sync_key(key),
            InputMode::PrMenu => self.handle_pr_key(key),
            InputMode::Help => self.handle_help_key(key),
        }
    }

    fn handle_normal_key(&mut self, key: KeyCode) {
        match key {
            // Navigation
            KeyCode::Up | KeyCode::Char('k') => self.previous_item(),
            KeyCode::Down | KeyCode::Char('j') => self.next_item(),
            KeyCode::Home | KeyCode::Char('g') => {
                if !self.entries.is_empty() {
                    self.table_state.select(Some(0));
                }
            }
            KeyCode::End | KeyCode::Char('G') => {
                if !self.entries.is_empty() {
                    self.table_state.select(Some(self.entries.len() - 1));
                }
            }

            // Worktree actions
            KeyCode::Enter | KeyCode::Char('o') => self.open_selected(),
            KeyCode::Char('n') => self.start_new_worktree(),
            KeyCode::Char('d') => self.start_delete(),
            KeyCode::Char('r') => self.reload_selected(),

            // Sync & cleanup
            KeyCode::Char('s') => self.show_sync_menu(),
            KeyCode::Char('c') => self.exec_gc(),

            // AI & config (needs to exit TUI for its own TUI)
            KeyCode::Char('a') => {
                self.action = StatusAction::Ai;
                self.should_exit = true;
            }

            // PR commands
            KeyCode::Char('p') => self.show_pr_menu(),

            // Doctor
            KeyCode::Char('D') => self.exec_doctor(),

            // Refresh
            KeyCode::Char('R') => {
                self.refresh();
                self.message = Some(("Refreshed".to_string(), false));
            }

            // Help & quit
            KeyCode::Char('?') => self.show_help(),
            KeyCode::Esc | KeyCode::Char('q') => self.should_exit = true,
            _ => {}
        }
    }

    fn handle_input_key(&mut self, key: KeyCode) {
        match key {
            KeyCode::Enter => self.confirm_new_worktree(),
            KeyCode::Esc => {
                self.input_mode = InputMode::Normal;
                self.input_buffer.clear();
            }
            KeyCode::Backspace => {
                self.input_buffer.pop();
            }
            KeyCode::Char(c) => {
                self.input_buffer.push(c);
            }
            _ => {}
        }
    }

    fn handle_delete_key(&mut self, key: KeyCode) {
        match key {
            KeyCode::Char('y') => self.confirm_delete(false),
            KeyCode::Char('f') => self.confirm_delete(true),
            KeyCode::Char('n') | KeyCode::Esc => {
                self.input_mode = InputMode::Normal;
            }
            _ => {}
        }
    }

    fn handle_sync_key(&mut self, key: KeyCode) {
        match key {
            KeyCode::Char('c') => {
                self.input_mode = InputMode::Normal;
                self.exec_sync_create();
            }
            KeyCode::Char('d') => {
                self.input_mode = InputMode::Normal;
                self.exec_sync_delete();
            }
            KeyCode::Esc | KeyCode::Char('q') => {
                self.input_mode = InputMode::Normal;
            }
            _ => {}
        }
    }

    fn handle_pr_key(&mut self, key: KeyCode) {
        match key {
            KeyCode::Char('c') | KeyCode::Enter => {
                self.input_mode = InputMode::Normal;
                // PR create needs to open browser - show message
                self.message = Some(("Use 'ws pr' from terminal to create PR".to_string(), false));
            }
            KeyCode::Char('l') => {
                self.input_mode = InputMode::Normal;
                // PR list needs external display - show message
                self.message = Some((
                    "Use 'ws pr list' from terminal to list PRs".to_string(),
                    false,
                ));
            }
            KeyCode::Esc | KeyCode::Char('q') => {
                self.input_mode = InputMode::Normal;
            }
            _ => {}
        }
    }

    fn handle_help_key(&mut self, key: KeyCode) {
        match key {
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('?') | KeyCode::Enter => {
                self.input_mode = InputMode::Normal;
            }
            _ => {}
        }
    }
}

fn draw_status(frame: &mut Frame, app: &mut StatusApp) {
    let area = frame.area();

    let chunks = if app.has_orphans() {
        Layout::vertical([
            Constraint::Length(2), // Title
            Constraint::Min(5),    // Main table
            Constraint::Length(6), // Orphan tables
            Constraint::Length(2), // Footer
        ])
        .split(area)
    } else {
        Layout::vertical([
            Constraint::Length(2), // Title
            Constraint::Min(5),    // Main table
            Constraint::Length(2), // Footer
        ])
        .split(area)
    };

    // Title
    let title = Line::from(vec![
        Span::styled(
            format!(" {} ", app.repo_name),
            Style::default()
                .fg(RatColor::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("Workspace Status"),
        Span::styled("  ?", Style::default().fg(RatColor::DarkGray)),
        Span::styled(" help", Style::default().fg(RatColor::DarkGray)),
    ]);
    frame.render_widget(Paragraph::new(title), chunks[0]);

    // Main table
    let header = Row::new(vec![
        RatCell::from("").style(Style::default()),
        RatCell::from("Session").style(Style::default().fg(RatColor::Cyan)),
        RatCell::from("Branch").style(Style::default().fg(RatColor::Cyan)),
        RatCell::from("Path").style(Style::default().fg(RatColor::Cyan)),
    ])
    .height(1);

    let rows: Vec<Row> = app
        .entries
        .iter()
        .map(|entry| {
            let status = if entry.has_session { "●" } else { "○" };
            let status_style = if entry.has_session {
                Style::default().fg(RatColor::Green)
            } else {
                Style::default().fg(RatColor::Yellow)
            };
            let main_marker = if entry.is_main { " (main)" } else { "" };
            let dim_style = Style::default().fg(RatColor::DarkGray);

            Row::new(vec![
                RatCell::from(status).style(status_style),
                RatCell::from(entry.session.as_str()).style(if entry.has_session {
                    Style::default()
                } else {
                    dim_style
                }),
                RatCell::from(format!("{}{}", entry.branch, main_marker)),
                RatCell::from(entry.path.display().to_string()).style(dim_style),
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
    )
    .row_highlight_style(
        Style::default()
            .bg(RatColor::DarkGray)
            .add_modifier(Modifier::BOLD),
    )
    .highlight_symbol("> ");

    frame.render_stateful_widget(table, chunks[1], &mut app.table_state);

    if app.has_orphans() {
        // Split bottom area for two orphan tables
        let orphan_chunks =
            Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)])
                .split(chunks[2]);

        // Orphaned sessions table
        let session_rows: Vec<Row> = if app.orphaned_sessions.is_empty() {
            vec![Row::new(vec![
                RatCell::from("None").style(Style::default().fg(RatColor::DarkGray))
            ])]
        } else {
            app.orphaned_sessions
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
        let worktree_rows: Vec<Row> = if app.orphaned_worktrees.is_empty() {
            vec![Row::new(vec![
                RatCell::from("None").style(Style::default().fg(RatColor::DarkGray))
            ])]
        } else {
            app.orphaned_worktrees
                .iter()
                .map(|b| Row::new(vec![RatCell::from(b.as_str())]))
                .collect()
        };
        let worktrees_table = RatTable::new(worktree_rows, [Constraint::Percentage(100)]).block(
            Block::default()
                .borders(Borders::ALL)
                .title(Span::styled(
                    " Orphaned Worktrees ",
                    Style::default().fg(RatColor::Yellow),
                ))
                .border_style(Style::default().fg(RatColor::DarkGray)),
        );
        frame.render_widget(worktrees_table, orphan_chunks[1]);

        // Footer
        draw_footer(frame, app, chunks[3]);
    } else {
        // Footer
        draw_footer(frame, app, chunks[2]);
    }

    // Draw overlays
    match app.input_mode {
        InputMode::NewBranch => draw_new_branch_popup(frame, app),
        InputMode::ConfirmDelete => draw_delete_popup(frame, app),
        InputMode::SyncMenu => draw_sync_popup(frame),
        InputMode::PrMenu => draw_pr_popup(frame),
        InputMode::Help => draw_help_popup(frame),
        InputMode::Normal => {}
    }
}

/// Build worktree action footer spans (shared between status and dashboard)
fn worktree_footer_spans(has_orphans: bool) -> Vec<Span<'static>> {
    if has_orphans {
        vec![
            Span::styled("o", Style::default().fg(RatColor::Cyan)),
            Span::raw("pen "),
            Span::styled("n", Style::default().fg(RatColor::Cyan)),
            Span::raw("ew "),
            Span::styled("d", Style::default().fg(RatColor::Cyan)),
            Span::raw("el "),
            Span::styled("r", Style::default().fg(RatColor::Cyan)),
            Span::raw("eload "),
            Span::styled("s", Style::default().fg(RatColor::Cyan)),
            Span::raw("ync "),
            Span::styled("a", Style::default().fg(RatColor::Cyan)),
            Span::raw("i "),
            Span::styled("p", Style::default().fg(RatColor::Cyan)),
            Span::raw("r "),
            Span::styled("c", Style::default().fg(RatColor::Cyan)),
            Span::raw("lean "),
            Span::styled("q", Style::default().fg(RatColor::Cyan)),
            Span::raw("uit"),
        ]
    } else {
        vec![
            Span::styled(" ✓ ", Style::default().fg(RatColor::Green)),
            Span::raw("All in sync  "),
            Span::styled("│", Style::default().fg(RatColor::DarkGray)),
            Span::raw(" "),
            Span::styled("o", Style::default().fg(RatColor::Cyan)),
            Span::raw("pen "),
            Span::styled("n", Style::default().fg(RatColor::Cyan)),
            Span::raw("ew "),
            Span::styled("d", Style::default().fg(RatColor::Cyan)),
            Span::raw("el "),
            Span::styled("r", Style::default().fg(RatColor::Cyan)),
            Span::raw("eload "),
            Span::styled("a", Style::default().fg(RatColor::Cyan)),
            Span::raw("i "),
            Span::styled("p", Style::default().fg(RatColor::Cyan)),
            Span::raw("r "),
            Span::styled("c", Style::default().fg(RatColor::Cyan)),
            Span::raw("lean "),
            Span::styled("q", Style::default().fg(RatColor::Cyan)),
            Span::raw("uit"),
        ]
    }
}

/// Build PR action footer spans
fn pr_footer_spans(worktree_exists: bool) -> Vec<Span<'static>> {
    let status = if worktree_exists {
        vec![
            Span::styled(" ✓ ", Style::default().fg(RatColor::Green)),
            Span::raw("Worktree exists  "),
            Span::styled("│", Style::default().fg(RatColor::DarkGray)),
            Span::raw(" "),
        ]
    } else {
        vec![
            Span::styled(" ○ ", Style::default().fg(RatColor::Yellow)),
            Span::raw("No worktree  "),
            Span::styled("│", Style::default().fg(RatColor::DarkGray)),
            Span::raw(" "),
        ]
    };

    let mut spans = status;
    spans.extend(vec![
        Span::styled("Enter", Style::default().fg(RatColor::Cyan)),
        Span::raw(" review "),
        Span::styled("q", Style::default().fg(RatColor::Cyan)),
        Span::raw("uit"),
    ]);
    spans
}

fn draw_footer(frame: &mut Frame, app: &StatusApp, area: Rect) {
    let footer_content = if let Some((msg, is_error)) = &app.message {
        vec![
            Span::styled(
                if *is_error { " ✗ " } else { " ✓ " },
                Style::default().fg(if *is_error {
                    RatColor::Red
                } else {
                    RatColor::Green
                }),
            ),
            Span::raw(msg.as_str()),
        ]
    } else {
        worktree_footer_spans(app.has_orphans())
    };

    let footer = Paragraph::new(Line::from(footer_content));
    frame.render_widget(footer, area);
}

fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let popup_layout = Layout::vertical([
        Constraint::Percentage((100 - percent_y) / 2),
        Constraint::Percentage(percent_y),
        Constraint::Percentage((100 - percent_y) / 2),
    ])
    .split(r);

    Layout::horizontal([
        Constraint::Percentage((100 - percent_x) / 2),
        Constraint::Percentage(percent_x),
        Constraint::Percentage((100 - percent_x) / 2),
    ])
    .split(popup_layout[1])[1]
}

fn draw_new_branch_popup(frame: &mut Frame, app: &StatusApp) {
    let area = centered_rect(50, 20, frame.area());
    frame.render_widget(Clear, area);

    let block = Block::default()
        .title(" New Worktree ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(RatColor::Cyan))
        .style(Style::default().bg(RatColor::Black));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let chunks = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(3),
        Constraint::Length(1),
    ])
    .split(inner);

    let label = Paragraph::new("Branch name:").style(Style::default().fg(RatColor::White));
    frame.render_widget(label, chunks[1]);

    let input = Paragraph::new(format!("{}_", app.input_buffer))
        .style(Style::default().fg(RatColor::Yellow))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(RatColor::DarkGray)),
        );
    frame.render_widget(input, chunks[2]);

    let hint = Paragraph::new("Enter to confirm, Esc to cancel")
        .style(Style::default().fg(RatColor::DarkGray))
        .alignment(Alignment::Center);
    frame.render_widget(hint, chunks[3]);
}

fn draw_delete_popup(frame: &mut Frame, app: &StatusApp) {
    let area = centered_rect(50, 25, frame.area());
    frame.render_widget(Clear, area);

    let branch = app
        .selected_entry()
        .map(|e| e.branch.as_str())
        .unwrap_or("?");

    let block = Block::default()
        .title(" Delete Worktree ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(RatColor::Red))
        .style(Style::default().bg(RatColor::Black));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let chunks = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(2),
        Constraint::Length(2),
        Constraint::Length(1),
    ])
    .split(inner);

    let msg = Paragraph::new(format!("Delete worktree '{}'?", branch))
        .style(Style::default().fg(RatColor::White))
        .alignment(Alignment::Center);
    frame.render_widget(msg, chunks[1]);

    let hint = Paragraph::new(Line::from(vec![
        Span::styled("y", Style::default().fg(RatColor::Green)),
        Span::raw(" yes  "),
        Span::styled("f", Style::default().fg(RatColor::Yellow)),
        Span::raw(" force  "),
        Span::styled("n", Style::default().fg(RatColor::Red)),
        Span::raw(" cancel"),
    ]))
    .alignment(Alignment::Center);
    frame.render_widget(hint, chunks[2]);
}

fn draw_sync_popup(frame: &mut Frame) {
    let area = centered_rect(40, 25, frame.area());
    frame.render_widget(Clear, area);

    let block = Block::default()
        .title(" Sync ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(RatColor::Cyan))
        .style(Style::default().bg(RatColor::Black));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let chunks = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(2),
        Constraint::Length(2),
        Constraint::Length(1),
    ])
    .split(inner);

    let options = Paragraph::new(vec![
        Line::from(vec![
            Span::styled("c", Style::default().fg(RatColor::Cyan)),
            Span::raw(" Create sessions for worktrees"),
        ]),
        Line::from(vec![
            Span::styled("d", Style::default().fg(RatColor::Yellow)),
            Span::raw(" Delete orphaned worktrees"),
        ]),
    ]);
    frame.render_widget(options, chunks[1]);

    let hint = Paragraph::new("Esc to cancel")
        .style(Style::default().fg(RatColor::DarkGray))
        .alignment(Alignment::Center);
    frame.render_widget(hint, chunks[3]);
}

fn draw_pr_popup(frame: &mut Frame) {
    let area = centered_rect(40, 25, frame.area());
    frame.render_widget(Clear, area);

    let block = Block::default()
        .title(" Pull Request ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(RatColor::Cyan))
        .style(Style::default().bg(RatColor::Black));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let chunks = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(2),
        Constraint::Length(2),
        Constraint::Length(1),
    ])
    .split(inner);

    let options = Paragraph::new(vec![
        Line::from(vec![
            Span::styled("c", Style::default().fg(RatColor::Green)),
            Span::raw(" Create new PR (opens browser)"),
        ]),
        Line::from(vec![
            Span::styled("l", Style::default().fg(RatColor::Cyan)),
            Span::raw(" List PRs for worktrees"),
        ]),
    ]);
    frame.render_widget(options, chunks[1]);

    let hint = Paragraph::new("Esc to cancel")
        .style(Style::default().fg(RatColor::DarkGray))
        .alignment(Alignment::Center);
    frame.render_widget(hint, chunks[3]);
}

fn draw_help_popup(frame: &mut Frame) {
    let area = centered_rect(60, 70, frame.area());
    frame.render_widget(Clear, area);

    let block = Block::default()
        .title(" Help ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(RatColor::Cyan))
        .style(Style::default().bg(RatColor::Black));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let help_text = vec![
        Line::from(Span::styled(
            "Navigation",
            Style::default()
                .fg(RatColor::Yellow)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(vec![
            Span::styled("  j/k ", Style::default().fg(RatColor::Cyan)),
            Span::raw("Move down/up"),
        ]),
        Line::from(vec![
            Span::styled("  g/G ", Style::default().fg(RatColor::Cyan)),
            Span::raw("Go to first/last"),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            "Worktree Actions",
            Style::default()
                .fg(RatColor::Yellow)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(vec![
            Span::styled("  o/Enter ", Style::default().fg(RatColor::Cyan)),
            Span::raw("Open selected worktree"),
        ]),
        Line::from(vec![
            Span::styled("  n ", Style::default().fg(RatColor::Cyan)),
            Span::raw("Create new worktree"),
        ]),
        Line::from(vec![
            Span::styled("  d ", Style::default().fg(RatColor::Cyan)),
            Span::raw("Delete selected worktree"),
        ]),
        Line::from(vec![
            Span::styled("  r ", Style::default().fg(RatColor::Cyan)),
            Span::raw("Reload session (kill & recreate)"),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            "Sync & Cleanup",
            Style::default()
                .fg(RatColor::Yellow)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(vec![
            Span::styled("  s ", Style::default().fg(RatColor::Cyan)),
            Span::raw("Sync menu (create/delete orphans)"),
        ]),
        Line::from(vec![
            Span::styled("  c ", Style::default().fg(RatColor::Cyan)),
            Span::raw("Garbage collect merged branches"),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            "Other",
            Style::default()
                .fg(RatColor::Yellow)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(vec![
            Span::styled("  a ", Style::default().fg(RatColor::Cyan)),
            Span::raw("Switch AI tool"),
        ]),
        Line::from(vec![
            Span::styled("  p ", Style::default().fg(RatColor::Cyan)),
            Span::raw("PR menu (create/list)"),
        ]),
        Line::from(vec![
            Span::styled("  D ", Style::default().fg(RatColor::Cyan)),
            Span::raw("Run doctor (check dependencies)"),
        ]),
        Line::from(vec![
            Span::styled("  R ", Style::default().fg(RatColor::Cyan)),
            Span::raw("Refresh worktree list"),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled("  q/Esc ", Style::default().fg(RatColor::Cyan)),
            Span::raw("Quit"),
        ]),
    ];

    let help = Paragraph::new(help_text);
    frame.render_widget(help, inner);
}

/// Show interactive status dashboard with worktrees and sessions
pub fn status() -> Result<StatusAction> {
    let mut app = StatusApp::new()?;

    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Main loop with proper cleanup on error
    let result = run_status_loop(&mut terminal, &mut app);

    // Always restore terminal
    let _ = disable_raw_mode();
    let _ = execute!(terminal.backend_mut(), LeaveAlternateScreen);

    result?;
    Ok(app.action)
}

fn run_status_loop(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    app: &mut StatusApp,
) -> Result<()> {
    while !app.should_exit {
        // Check for completed background tasks
        app.poll_tasks();

        terminal.draw(|frame| draw_status(frame, app))?;

        if event::poll(std::time::Duration::from_millis(100))? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    app.handle_key(key.code);
                }
            }
        }
    }
    Ok(())
}

// ============================================================================
// Dashboard (Plasma + Status)
// ============================================================================

/// PR check status
#[derive(Clone, PartialEq)]
enum CheckStatus {
    Pending,
    Success,
    Failure,
    Unknown,
}

/// PR entry for display
#[derive(Clone)]
struct PrEntry {
    number: u32,
    title: String,
    branch: String,
    checks: CheckStatus,
}

/// Dashboard app state with plasma animation
struct DashboardApp {
    status: StatusApp,
    plasma: ReactionDiffusion,
    last_frame: Instant,
    prs: Vec<PrEntry>,
    pr_table_state: TableState,
    focus: DashboardFocus,
    gh_available: bool,
    // Areas for mouse click detection
    pr_area: Option<Rect>,
    worktree_area: Option<Rect>,
}

#[derive(PartialEq, Clone, Copy)]
enum DashboardFocus {
    Worktrees,
    PullRequests,
}

impl DashboardApp {
    fn new() -> Result<Self> {
        let status = StatusApp::new()?;
        let metrics = WorkspaceMetrics::from_git_root(&status.git_root);

        // Check if gh is available
        let gh_available = which::which("gh").is_ok();

        // Fetch PRs if gh is available
        let prs = if gh_available {
            Self::fetch_prs(&status.git_root)
        } else {
            Vec::new()
        };

        let mut pr_table_state = TableState::default();
        if !prs.is_empty() {
            pr_table_state.select(Some(0));
        }

        Ok(Self {
            status,
            plasma: ReactionDiffusion::with_metrics(80, 40, metrics),
            last_frame: Instant::now(),
            prs,
            pr_table_state,
            focus: DashboardFocus::Worktrees,
            gh_available,
            pr_area: None,
            worktree_area: None,
        })
    }

    fn handle_mouse_click(&mut self, x: u16, y: u16) {
        // Check if click is in PR area
        if let Some(pr_area) = self.pr_area {
            if x >= pr_area.x
                && x < pr_area.x + pr_area.width
                && y >= pr_area.y
                && y < pr_area.y + pr_area.height
            {
                self.focus = DashboardFocus::PullRequests;
                // Calculate which row was clicked (accounting for border)
                let row = y.saturating_sub(pr_area.y + 1) as usize;
                if row < self.prs.len() {
                    self.pr_table_state.select(Some(row));
                }
                return;
            }
        }

        // Check if click is in worktree area
        if let Some(wt_area) = self.worktree_area {
            if x >= wt_area.x
                && x < wt_area.x + wt_area.width
                && y >= wt_area.y
                && y < wt_area.y + wt_area.height
            {
                self.focus = DashboardFocus::Worktrees;
                // Calculate which row was clicked (accounting for header + border)
                let row = y.saturating_sub(wt_area.y + 2) as usize;
                if row < self.status.entries.len() {
                    self.status.table_state.select(Some(row));
                }
            }
        }
    }

    fn fetch_prs(git_root: &std::path::Path) -> Vec<PrEntry> {
        let output = std::process::Command::new("gh")
            .current_dir(git_root)
            .args([
                "pr",
                "list",
                "--json",
                "number,title,headRefName,statusCheckRollup",
                "--limit",
                "10",
            ])
            .output();

        match output {
            Ok(o) if o.status.success() => {
                if let Ok(prs) = serde_json::from_slice::<Vec<serde_json::Value>>(&o.stdout) {
                    prs.iter()
                        .filter_map(|pr| {
                            let checks = Self::parse_check_status(&pr["statusCheckRollup"]);
                            Some(PrEntry {
                                number: pr["number"].as_u64()? as u32,
                                title: pr["title"].as_str()?.to_string(),
                                branch: pr["headRefName"].as_str()?.to_string(),
                                checks,
                            })
                        })
                        .collect()
                } else {
                    Vec::new()
                }
            }
            _ => Vec::new(),
        }
    }

    fn parse_check_status(rollup: &serde_json::Value) -> CheckStatus {
        let checks = match rollup.as_array() {
            Some(arr) => arr,
            None => return CheckStatus::Unknown,
        };

        if checks.is_empty() {
            return CheckStatus::Unknown;
        }

        let mut has_pending = false;
        let mut has_failure = false;

        for check in checks {
            // Check both "state" (for check runs) and "conclusion" (for status contexts)
            let state = check["state"].as_str().unwrap_or("");
            let conclusion = check["conclusion"].as_str().unwrap_or("");
            let status = check["status"].as_str().unwrap_or("");

            // Failure states
            if state == "FAILURE"
                || state == "ERROR"
                || conclusion == "FAILURE"
                || conclusion == "failure"
                || conclusion == "ERROR"
                || conclusion == "error"
            {
                has_failure = true;
            }
            // Pending states
            else if state == "PENDING"
                || state == "EXPECTED"
                || status == "IN_PROGRESS"
                || status == "QUEUED"
                || conclusion.is_empty()
            {
                has_pending = true;
            }
        }

        if has_failure {
            CheckStatus::Failure
        } else if has_pending {
            CheckStatus::Pending
        } else {
            CheckStatus::Success
        }
    }

    fn update_plasma(&mut self) {
        if self.last_frame.elapsed() >= std::time::Duration::from_millis(50) {
            for _ in 0..4 {
                self.plasma.step();
            }
            self.last_frame = Instant::now();
        }
    }

    fn toggle_focus(&mut self) {
        if self.gh_available && !self.prs.is_empty() {
            self.focus = match self.focus {
                DashboardFocus::Worktrees => DashboardFocus::PullRequests,
                DashboardFocus::PullRequests => DashboardFocus::Worktrees,
            };
        }
    }

    fn next_pr(&mut self) {
        if self.prs.is_empty() {
            return;
        }
        let i = self.pr_table_state.selected().unwrap_or(0);
        let next = if i >= self.prs.len() - 1 { 0 } else { i + 1 };
        self.pr_table_state.select(Some(next));
    }

    fn prev_pr(&mut self) {
        if self.prs.is_empty() {
            return;
        }
        let i = self.pr_table_state.selected().unwrap_or(0);
        let prev = if i == 0 { self.prs.len() - 1 } else { i - 1 };
        self.pr_table_state.select(Some(prev));
    }

    fn selected_pr(&self) -> Option<&PrEntry> {
        self.pr_table_state.selected().and_then(|i| self.prs.get(i))
    }
}

fn draw_dashboard(frame: &mut Frame, app: &mut DashboardApp) {
    let area = frame.area();

    // Split: 40% plasma, 60% status
    let layout =
        Layout::horizontal([Constraint::Percentage(40), Constraint::Percentage(60)]).split(area);

    // Left: Plasma animation
    let plasma_width = layout[0].width as usize;
    let plasma_height = layout[0].height as usize;
    app.plasma
        .resize(plasma_width.max(10), plasma_height.max(10));

    let plasma_lines = app.plasma.render();
    let plasma_text: Vec<Line> = plasma_lines
        .iter()
        .map(|line| {
            Line::from(Span::styled(
                line.clone(),
                Style::default().fg(RatColor::Green),
            ))
        })
        .collect();

    let plasma_height_u16 = plasma_text.len() as u16;
    let available_height = layout[0].height;
    let vertical_padding = available_height.saturating_sub(plasma_height_u16) / 2;

    let plasma_area = Rect {
        x: layout[0].x,
        y: layout[0].y + vertical_padding,
        width: layout[0].width,
        height: plasma_height_u16.min(available_height),
    };

    frame.render_widget(Paragraph::new(plasma_text), plasma_area);

    // Right: Status content with PR list
    draw_dashboard_content(frame, app, layout[1]);
}

fn draw_dashboard_content(frame: &mut Frame, app: &mut DashboardApp, area: Rect) {
    // Wrap in a bordered block with repo name
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(RatColor::DarkGray))
        .title(format!(" {} ", app.status.repo_name));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    // Layout: PRs (if available) | Worktrees | Footer
    let has_prs = app.gh_available && !app.prs.is_empty();
    let chunks = if has_prs {
        Layout::vertical([
            Constraint::Length(app.prs.len().min(5) as u16 + 3), // PR table (max 5 rows + header + borders)
            Constraint::Min(5),                                  // Worktrees table
            Constraint::Length(2),                               // Footer
        ])
        .split(inner)
    } else {
        Layout::vertical([
            Constraint::Min(5),    // Worktrees table
            Constraint::Length(2), // Footer
        ])
        .split(inner)
    };

    let (pr_area_rect, worktree_area_rect, footer_area) = if has_prs {
        (Some(chunks[0]), chunks[1], chunks[2])
    } else {
        (None, chunks[0], chunks[1])
    };

    // Store areas for mouse click detection
    app.pr_area = pr_area_rect;
    app.worktree_area = Some(worktree_area_rect);

    // Draw PR table if available
    if let Some(pr_area) = pr_area_rect {
        let pr_rows: Vec<Row> = app
            .prs
            .iter()
            .take(5)
            .map(|pr| {
                let (check_icon, check_style) = match pr.checks {
                    CheckStatus::Success => ("✓", Style::default().fg(RatColor::Green)),
                    CheckStatus::Failure => ("✗", Style::default().fg(RatColor::Red)),
                    CheckStatus::Pending => ("○", Style::default().fg(RatColor::Yellow)),
                    CheckStatus::Unknown => (" ", Style::default().fg(RatColor::DarkGray)),
                };
                Row::new(vec![
                    RatCell::from(check_icon).style(check_style),
                    RatCell::from(format!("#{}", pr.number))
                        .style(Style::default().fg(RatColor::Cyan)),
                    RatCell::from(pr.title.chars().take(35).collect::<String>()),
                    RatCell::from(pr.branch.as_str())
                        .style(Style::default().fg(RatColor::DarkGray)),
                ])
            })
            .collect();

        let pr_block_style = if app.focus == DashboardFocus::PullRequests {
            Style::default().fg(RatColor::Cyan)
        } else {
            Style::default().fg(RatColor::DarkGray)
        };

        let pr_table = RatTable::new(
            pr_rows,
            [
                Constraint::Length(2),
                Constraint::Length(6),
                Constraint::Percentage(55),
                Constraint::Percentage(45),
            ],
        )
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Pull Requests ")
                .border_style(pr_block_style),
        )
        .row_highlight_style(
            Style::default()
                .bg(RatColor::DarkGray)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("> ");

        frame.render_stateful_widget(pr_table, pr_area, &mut app.pr_table_state);
    }

    // Draw worktrees table
    let header = Row::new(vec![
        RatCell::from("").style(Style::default()),
        RatCell::from("Session").style(Style::default().fg(RatColor::Cyan)),
        RatCell::from("Branch").style(Style::default().fg(RatColor::Cyan)),
    ])
    .height(1);

    let rows: Vec<Row> = app
        .status
        .entries
        .iter()
        .map(|entry| {
            let status = if entry.has_session { "●" } else { "○" };
            let status_style = if entry.has_session {
                Style::default().fg(RatColor::Green)
            } else {
                Style::default().fg(RatColor::Yellow)
            };
            let main_marker = if entry.is_main { " (main)" } else { "" };
            let dim_style = Style::default().fg(RatColor::DarkGray);

            Row::new(vec![
                RatCell::from(status).style(status_style),
                RatCell::from(entry.session.as_str()).style(if entry.has_session {
                    Style::default()
                } else {
                    dim_style
                }),
                RatCell::from(format!("{}{}", entry.branch, main_marker)),
            ])
        })
        .collect();

    let worktree_block_style = if app.focus == DashboardFocus::Worktrees {
        Style::default().fg(RatColor::Cyan)
    } else {
        Style::default().fg(RatColor::DarkGray)
    };

    let table = RatTable::new(
        rows,
        [
            Constraint::Length(2),
            Constraint::Percentage(50),
            Constraint::Percentage(50),
        ],
    )
    .header(header)
    .block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Worktrees & Sessions ")
            .border_style(worktree_block_style),
    )
    .row_highlight_style(
        Style::default()
            .bg(RatColor::DarkGray)
            .add_modifier(Modifier::BOLD),
    )
    .highlight_symbol("> ");

    frame.render_stateful_widget(table, worktree_area_rect, &mut app.status.table_state);

    // Dynamic footer based on focus (reuses shared footer spans)
    let footer_text = match app.focus {
        DashboardFocus::PullRequests => {
            // Check if selected PR's branch has a worktree
            let worktree_exists = app
                .selected_pr()
                .map(|pr| app.status.entries.iter().any(|e| e.branch == pr.branch))
                .unwrap_or(false);
            pr_footer_spans(worktree_exists)
        }
        DashboardFocus::Worktrees => worktree_footer_spans(app.status.has_orphans()),
    };

    let footer = Paragraph::new(Line::from(footer_text));
    frame.render_widget(footer, footer_area);
}

/// Show dashboard with plasma animation on left and status on right
pub fn dashboard() -> Result<StatusAction> {
    let mut app = DashboardApp::new()?;

    enable_raw_mode()?;
    let mut stdout = stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = run_dashboard_loop(&mut terminal, &mut app);

    let _ = disable_raw_mode();
    let _ = execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    );

    result?;
    Ok(app.status.action)
}

fn run_dashboard_loop(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    app: &mut DashboardApp,
) -> Result<()> {
    while !app.status.should_exit {
        // Check for completed background tasks
        app.status.poll_tasks();

        app.update_plasma();
        terminal.draw(|frame| draw_dashboard(frame, app))?;

        if event::poll(std::time::Duration::from_millis(50))? {
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    // Handle dashboard-specific keys first
                    match key.code {
                        KeyCode::Tab => {
                            app.toggle_focus();
                            continue;
                        }
                        KeyCode::Char('j') | KeyCode::Down => {
                            if app.focus == DashboardFocus::PullRequests {
                                app.next_pr();
                                continue;
                            }
                        }
                        KeyCode::Char('k') | KeyCode::Up => {
                            if app.focus == DashboardFocus::PullRequests {
                                app.prev_pr();
                                continue;
                            }
                        }
                        KeyCode::Enter => {
                            if app.focus == DashboardFocus::PullRequests {
                                if let Some(pr) = app.selected_pr() {
                                    // Set action to review this PR
                                    app.status.action = StatusAction::ReviewPr(pr.number);
                                    app.status.should_exit = true;
                                }
                                continue;
                            }
                        }
                        _ => {}
                    }
                    // Fall through to status key handling
                    app.status.handle_key(key.code);
                }
                Event::Mouse(mouse) => {
                    if mouse.kind == MouseEventKind::Down(crossterm::event::MouseButton::Left) {
                        app.handle_mouse_click(mouse.column, mouse.row);
                    }
                }
                _ => {}
            }
        }
    }
    Ok(())
}
