use crate::config::Config;
use anyhow::{Context, Result};
use std::collections::HashSet;
use std::path::Path;
use std::process::Command;

/// Check if a tmux session exists
pub fn session_exists(name: &str) -> bool {
    Command::new("tmux")
        .args(["has-session", "-t", name])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Get list of active tmux session names
pub fn get_active_sessions() -> HashSet<String> {
    let output = Command::new("tmux")
        .args(["list-sessions", "-F", "#{session_name}"])
        .output();

    match output {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout)
            .lines()
            .map(|s| s.to_string())
            .collect(),
        _ => HashSet::new(),
    }
}

/// Check if we're running inside a tmux session
pub fn is_inside_tmux() -> bool {
    std::env::var("TMUX").is_ok()
}

/// Attach to an existing tmux session (replaces current process)
/// If already inside tmux, uses switch-client instead of attach
pub fn attach(session: &str) -> Result<()> {
    if is_inside_tmux() {
        // Inside tmux (including popups), use switch-client
        let result = Command::new("tmux")
            .args(["switch-client", "-t", session])
            .output()
            .context("Failed to switch tmux client")?;

        if !result.status.success() {
            let stderr = String::from_utf8_lossy(&result.stderr);
            anyhow::bail!("Failed to switch to session: {}", stderr.trim());
        }
        Ok(())
    } else {
        // Outside tmux, use attach (replaces current process)
        let err = exec::execvp("tmux", &["tmux", "attach", "-t", session]);
        anyhow::bail!("Failed to attach to tmux: {}", err);
    }
}

/// Kill a tmux session
pub fn kill_session(session: &str) -> Result<()> {
    let result = Command::new("tmux")
        .args(["kill-session", "-t", session])
        .output()
        .context("Failed to kill tmux session")?;

    if !result.status.success() {
        anyhow::bail!("Failed to kill session: {}", session);
    }

    Ok(())
}

/// Detect if on large display (external monitor)
pub fn is_large_display() -> bool {
    let output = Command::new("osascript")
        .arg("-e")
        .arg("tell application \"System Events\" to tell process \"Ghostty\" to get position of window 1")
        .output();

    if let Ok(o) = output {
        if o.status.success() {
            let pos = String::from_utf8_lossy(&o.stdout);
            if let Some(y_str) = pos.split(',').nth(1) {
                if let Ok(y) = y_str.trim().parse::<i32>() {
                    return y < 0;
                }
            }
        }
    }
    false
}

/// Create a new tmux session with custom window title
pub fn create_session_with_title(session: &str, dir: &Path, window_title: &str) -> Result<()> {
    let dir_str = dir.to_str().context("Invalid path")?;
    let ghostty_env = get_ghostty_env();

    // Load config to get panel tools
    let config = Config::load().unwrap_or_default();
    let ai_cmd = config.ai_tool.command();
    let git_cmd = config.git_tool.command();
    let explorer_cmd = config.explorer_tool.command();

    // Create new session with window name
    let mut args = vec![
        "new-session".to_string(),
        "-d".to_string(),
        "-s".to_string(),
        session.to_string(),
        "-n".to_string(),
        window_title.to_string(),
        "-c".to_string(),
        dir_str.to_string(),
    ];
    args.extend(ghostty_env.clone());

    Command::new("tmux")
        .args(&args)
        .output()
        .context("Failed to create tmux session")?;

    // Set up status bar with PR info
    setup_status_bar(session, dir)?;

    // Create layout based on display size
    if is_large_display() {
        create_large_layout(
            session,
            dir_str,
            &ghostty_env,
            ai_cmd,
            git_cmd,
            explorer_cmd,
        )?;
    } else {
        create_small_layout(
            session,
            dir_str,
            &ghostty_env,
            ai_cmd,
            git_cmd,
            explorer_cmd,
        )?;
    }

    Ok(())
}

/// Set up tmux status bar with branch and PR info
fn setup_status_bar(session: &str, dir: &Path) -> Result<()> {
    let dir_str = dir.to_str().context("Invalid path")?;

    // Create the status script if it doesn't exist
    ensure_status_script()?;

    // Set status-right to call our script with the directory
    // The script caches results for 60s to avoid lag
    let status_cmd = format!("#(ws --status-bar \"{}\")", dir_str);

    Command::new("tmux")
        .args(["set-option", "-t", session, "status-right", &status_cmd])
        .output()
        .context("Failed to set status-right")?;

    // Set status-right-length to allow longer content
    Command::new("tmux")
        .args(["set-option", "-t", session, "status-right-length", "100"])
        .output()
        .context("Failed to set status-right-length")?;

    Ok(())
}

/// Ensure the status script exists in ~/.ws/
fn ensure_status_script() -> Result<()> {
    // The script is now built into ws itself via --status-bar flag
    // No external script needed
    Ok(())
}

fn create_large_layout(
    session: &str,
    dir: &str,
    env: &[String],
    ai_cmd: &str,
    git_cmd: &str,
    explorer_cmd: &str,
) -> Result<()> {
    // Large display: golden ratio 3 columns (23% | 54% | 23%)
    run_tmux_split(session, "0.0", dir, env, &["-h", "-p", "77"])?;
    run_tmux_split(session, "0.1", dir, env, &["-h", "-p", "30"])?;
    run_tmux_split(session, "0.0", dir, env, &["-v"])?;
    run_tmux_split(session, "0.3", dir, env, &["-v"])?;

    // Send commands to panes (use clear for TUI apps)
    send_keys_with_clear(session, "0.0", git_cmd)?;
    send_keys_with_clear(session, "0.1", explorer_cmd)?;
    send_keys_with_clear(session, "0.2", ai_cmd)?;
    send_keys(session, "0.3", "ls -la")?;
    send_keys(
        session,
        "0.4",
        "tsqlx postgres://postgres:123@localhost:5432/basalt",
    )?;

    select_pane(session, "0.2")?;
    Ok(())
}

fn create_small_layout(
    session: &str,
    dir: &str,
    env: &[String],
    ai_cmd: &str,
    git_cmd: &str,
    explorer_cmd: &str,
) -> Result<()> {
    // Small display: 2 columns golden ratio (38% | 62%)
    run_tmux_split(session, "0.0", dir, env, &["-h", "-p", "62"])?;
    run_tmux_split(session, "0.0", dir, env, &["-v"])?;

    // Send commands to panes (use clear for TUI apps)
    send_keys_with_clear(session, "0.0", git_cmd)?;
    send_keys_with_clear(session, "0.1", explorer_cmd)?;
    send_keys_with_clear(session, "0.2", ai_cmd)?;

    select_pane(session, "0.2")?;
    Ok(())
}

fn run_tmux_split(
    session: &str,
    target: &str,
    dir: &str,
    env: &[String],
    extra_args: &[&str],
) -> Result<()> {
    let target_str = format!("{}:{}", session, target);
    let mut args = vec!["split-window"];
    args.extend(extra_args);
    args.extend(&["-t", &target_str, "-c", dir]);

    let env_refs: Vec<&str> = env.iter().map(|s| s.as_str()).collect();
    args.extend(env_refs);

    Command::new("tmux")
        .args(&args)
        .output()
        .context("Failed to split window")?;
    Ok(())
}

fn send_keys(session: &str, target: &str, cmd: &str) -> Result<()> {
    Command::new("tmux")
        .args([
            "send-keys",
            "-t",
            &format!("{}:{}", session, target),
            cmd,
            "C-m",
        ])
        .output()
        .context("Failed to send keys")?;
    Ok(())
}

/// Send keys with terminal clearing first (for TUI apps)
fn send_keys_with_clear(session: &str, target: &str, cmd: &str) -> Result<()> {
    let target_str = format!("{}:{}", session, target);

    // Clear terminal and command line first
    Command::new("tmux")
        .args(["send-keys", "-t", &target_str, "C-u", "clear", "C-m"])
        .output()
        .context("Failed to clear terminal")?;

    // Small delay for clear to complete
    std::thread::sleep(std::time::Duration::from_millis(50));

    // Send the actual command
    Command::new("tmux")
        .args(["send-keys", "-t", &target_str, cmd, "C-m"])
        .output()
        .context("Failed to send keys")?;

    Ok(())
}

fn select_pane(session: &str, target: &str) -> Result<()> {
    Command::new("tmux")
        .args(["select-pane", "-t", &format!("{}:{}", session, target)])
        .output()
        .context("Failed to select pane")?;
    Ok(())
}

fn get_ghostty_env() -> Vec<String> {
    let mut env = Vec::new();

    let vars = [
        ("TERM_PROGRAM", "ghostty"),
        ("COLORTERM", "truecolor"),
        ("__CFBundleIdentifier", "com.mitchellh.ghostty"),
    ];

    for (key, default) in vars {
        let value = std::env::var(key).unwrap_or_else(|_| default.to_string());
        env.push("-e".to_string());
        env.push(format!("{}={}", key, value));
    }

    // Pass through these if they exist
    for key in [
        "TERM_PROGRAM_VERSION",
        "GHOSTTY_RESOURCES_DIR",
        "GHOSTTY_BIN_DIR",
        "GHOSTTY_SHELL_FEATURES",
    ] {
        if let Ok(value) = std::env::var(key) {
            env.push("-e".to_string());
            env.push(format!("{}={}", key, value));
        }
    }

    env
}

/// Get number of panes in a session
pub fn get_pane_count(session: &str) -> usize {
    let output = Command::new("tmux")
        .args(["list-panes", "-t", session, "-F", "#{pane_id}"])
        .output();

    match output {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).lines().count(),
        _ => 0,
    }
}

/// Get current session name from TMUX env
pub fn get_current_session() -> Option<String> {
    // TMUX env format: /tmp/tmux-501/default,12345,0
    // We need to query tmux for the actual session name
    if !is_inside_tmux() {
        return None;
    }

    let output = Command::new("tmux")
        .args(["display-message", "-p", "#{session_name}"])
        .output()
        .ok()?;

    if output.status.success() {
        let name = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !name.is_empty() {
            return Some(name);
        }
    }
    None
}

/// Get working directory from pane 0 of a session
pub fn get_session_dir(session: &str) -> Option<String> {
    let output = Command::new("tmux")
        .args([
            "display-message",
            "-t",
            &format!("{}:0.0", session),
            "-p",
            "#{pane_current_path}",
        ])
        .output()
        .ok()?;

    if output.status.success() {
        let dir = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !dir.is_empty() {
            return Some(dir);
        }
    }
    None
}

/// Expand layout from 3 to 5 panes
pub fn expand_layout(session: &str, dir: &str) -> Result<()> {
    let config = Config::load().unwrap_or_default();
    let ghostty_env = get_ghostty_env();

    // Split pane 2 horizontally (creates pane 3)
    run_tmux_split(session, "0.2", dir, &ghostty_env, &["-h", "-p", "30"])?;

    // Split the new pane 3 vertically (creates pane 4)
    run_tmux_split(session, "0.3", dir, &ghostty_env, &["-v"])?;

    // Resize to golden ratio: 23% | 54% | 23%
    // Pane 0 and 1 are on the left, pane 2 is center, panes 3 and 4 are right

    // Resize left column (pane 0) from 38% to 23%
    Command::new("tmux")
        .args([
            "resize-pane",
            "-t",
            &format!("{}:0.0", session),
            "-x",
            "23%",
        ])
        .output()
        .context("Failed to resize pane 0")?;

    // Resize center column (pane 2) to 54%
    Command::new("tmux")
        .args([
            "resize-pane",
            "-t",
            &format!("{}:0.2", session),
            "-x",
            "54%",
        ])
        .output()
        .context("Failed to resize pane 2")?;

    // Send default commands to new panes
    send_keys(session, "0.3", "ls -la")?;
    send_keys(
        session,
        "0.4",
        "tsqlx postgres://postgres:123@localhost:5432/basalt",
    )?;

    // Re-select the AI pane (pane 2)
    select_pane(session, "0.2")?;

    eprintln!(
        "Expanded to 5-pane layout (using {} for AI)",
        config.ai_tool.name()
    );
    Ok(())
}

/// Shrink layout from 5 to 3 panes
pub fn shrink_layout(session: &str) -> Result<()> {
    // Kill panes 4 and 3 (in reverse order to maintain indices)
    Command::new("tmux")
        .args(["kill-pane", "-t", &format!("{}:0.4", session)])
        .output()
        .context("Failed to kill pane 4")?;

    Command::new("tmux")
        .args(["kill-pane", "-t", &format!("{}:0.3", session)])
        .output()
        .context("Failed to kill pane 3")?;

    // Resize to small layout ratio: 38% | 62%

    // Resize left column (pane 0) to 38%
    Command::new("tmux")
        .args([
            "resize-pane",
            "-t",
            &format!("{}:0.0", session),
            "-x",
            "38%",
        ])
        .output()
        .context("Failed to resize pane 0")?;

    // Resize right column (pane 2) to 62%
    Command::new("tmux")
        .args([
            "resize-pane",
            "-t",
            &format!("{}:0.2", session),
            "-x",
            "62%",
        ])
        .output()
        .context("Failed to resize pane 2")?;

    // Re-select the AI pane (pane 2)
    select_pane(session, "0.2")?;

    eprintln!("Shrunk to 3-pane layout");
    Ok(())
}

/// Acquire a lock for layout operations (returns lock file path if acquired)
fn acquire_layout_lock(session: &str) -> Option<std::path::PathBuf> {
    let lock_dir = dirs::cache_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("/tmp"))
        .join("ws-layout");
    let _ = std::fs::create_dir_all(&lock_dir);

    let lock_path = lock_dir.join(format!("{}.lock", session));

    // Try to create lock file exclusively
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&lock_path)
    {
        Ok(_) => Some(lock_path),
        Err(_) => {
            // Lock exists - check if it's stale (> 5 seconds old)
            if let Ok(metadata) = std::fs::metadata(&lock_path) {
                if let Ok(modified) = metadata.modified() {
                    if std::time::SystemTime::now()
                        .duration_since(modified)
                        .unwrap_or_default()
                        > std::time::Duration::from_secs(5)
                    {
                        // Stale lock, remove and retry
                        let _ = std::fs::remove_file(&lock_path);
                        if std::fs::OpenOptions::new()
                            .write(true)
                            .create_new(true)
                            .open(&lock_path)
                            .is_ok()
                        {
                            return Some(lock_path);
                        }
                    }
                }
            }
            None
        }
    }
}

/// Release the layout lock
fn release_layout_lock(lock_path: &std::path::Path) {
    let _ = std::fs::remove_file(lock_path);
}

/// RAII guard to release lock on drop
struct LockGuard(std::path::PathBuf);

impl Drop for LockGuard {
    fn drop(&mut self) {
        release_layout_lock(&self.0);
    }
}

/// Toggle layout based on display size
pub fn toggle_layout(force_expand: bool, force_shrink: bool) -> Result<()> {
    let session = get_current_session().context("Not inside a tmux session")?;

    // Acquire lock to prevent concurrent modifications
    let lock_path = match acquire_layout_lock(&session) {
        Some(path) => path,
        None => {
            // Another layout operation is in progress, skip silently
            return Ok(());
        }
    };

    // Ensure lock is released on exit (even on panic/early return)
    let _lock_guard = LockGuard(lock_path);

    // Check pane count after acquiring lock
    let pane_count = get_pane_count(&session);

    // Only operate on valid layouts (exactly 3 or 5 panes)
    if pane_count != 3 && pane_count != 5 {
        // Not a ws-managed layout, skip silently
        return Ok(());
    }

    // Determine action based on flags or current state
    let should_expand = if force_expand {
        true
    } else if force_shrink {
        false
    } else {
        // Auto-detect based on display size
        is_large_display()
    };

    if should_expand && pane_count == 3 {
        let dir = get_session_dir(&session).unwrap_or_else(|| ".".to_string());
        expand_layout(&session, &dir)
    } else if !should_expand && pane_count == 5 {
        shrink_layout(&session)
    } else {
        // Already in correct layout
        Ok(())
    }
}

// Use exec crate for proper process replacement
mod exec {
    use std::ffi::CString;

    pub fn execvp(program: &str, args: &[&str]) -> std::io::Error {
        let c_program = CString::new(program).unwrap();
        let c_args: Vec<CString> = args.iter().map(|s| CString::new(*s).unwrap()).collect();
        let c_arg_ptrs: Vec<*const libc::c_char> = c_args
            .iter()
            .map(|s| s.as_ptr())
            .chain(std::iter::once(std::ptr::null()))
            .collect();

        unsafe {
            libc::execvp(c_program.as_ptr(), c_arg_ptrs.as_ptr());
        }

        std::io::Error::last_os_error()
    }
}
