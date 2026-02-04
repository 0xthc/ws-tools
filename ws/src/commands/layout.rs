use anyhow::Result;

/// Toggle tmux layout based on display size or flags
pub fn layout(force_expand: bool, force_shrink: bool) -> Result<()> {
    crate::tmux::toggle_layout(force_expand, force_shrink)
}
