//! Agent attention as an overlay on Fleet's VS Code sessions.
//!
//! The Hub transport updates this shared rendered projection. The shell only
//! reads it to render badges, waiting state, and notifications; it does not gain
//! ownership of the agent process or anything inside the VS Code surface.

use std::sync::{Arc, Mutex};

use crate::render::RenderedInbox;

/// Latest attention projection shared between the Hub link and Fleet shell.
pub type SharedAttention = Arc<Mutex<RenderedInbox>>;

pub fn shared_attention() -> SharedAttention {
    Arc::new(Mutex::new(RenderedInbox::default()))
}

/// Initial attention snapshot for the rail. Live changes arrive as `inbox`
/// events emitted by the Hub client.
#[cfg_attr(coverage_nightly, coverage(off))]
#[tauri::command]
pub fn get_inbox(state: tauri::State<'_, SharedAttention>) -> RenderedInbox {
    state.lock().map(|inbox| inbox.clone()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_attention_starts_with_the_default_inbox() {
        let attention = shared_attention();
        assert_eq!(*attention.lock().unwrap(), RenderedInbox::default());
    }
}
