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

/// Attach to an existing tmux session (replaces current process)
pub fn attach(session: &str) -> Result<()> {
    let err = exec::execvp("tmux", &["tmux", "attach", "-t", session]);
    anyhow::bail!("Failed to attach to tmux: {}", err);
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
