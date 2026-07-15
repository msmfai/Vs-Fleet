//! Registry for Fleet's opaque VS Code webview surfaces.
//!
//! The shell chooses a session and supplies its URL. This registry remembers
//! only which webview belongs to that session and which URL was successfully
//! loaded, so switching does not reload VS Code. It intentionally knows nothing
//! about agents, tasks, files, Git, or VS Code's internal UI.

use std::collections::HashMap;
use std::sync::Mutex;

#[derive(Debug, Clone)]
struct SurfaceEntry {
    label: String,
    loaded_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SurfaceLayout {
    pub active_label: Option<String>,
    pub inactive_labels: Vec<String>,
}

/// App-owned navigation metadata for singleton and persistent editor webviews.
#[derive(Default)]
pub struct SurfaceRegistry {
    singleton_loaded_url: Mutex<Option<String>>,
    persistent: Mutex<HashMap<String, SurfaceEntry>>,
}

impl SurfaceRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Serialize singleton target checks and navigation just as the former
    /// multiplexer-owned lock did. The URL is recorded only after the caller's
    /// webview navigation succeeds.
    pub fn navigate_singleton<E>(
        &self,
        target: &str,
        force: bool,
        navigate: impl FnOnce() -> Result<(), E>,
    ) -> Result<bool, E> {
        let Ok(mut loaded) = self.singleton_loaded_url.lock() else {
            return Ok(false);
        };
        if !force && loaded.as_deref() == Some(target) {
            return Ok(false);
        }
        navigate()?;
        *loaded = Some(target.to_string());
        Ok(true)
    }

    pub fn clear_singleton(&self) {
        if let Ok(mut loaded) = self.singleton_loaded_url.lock() {
            *loaded = None;
        }
    }

    pub fn persistent_label(&self, session_id: &str) -> Option<String> {
        self.persistent
            .lock()
            .ok()
            .and_then(|surfaces| surfaces.get(session_id).map(|entry| entry.label.clone()))
    }

    pub fn register_persistent(&self, session_id: &str, label: String) {
        if let Ok(mut surfaces) = self.persistent.lock() {
            surfaces
                .entry(session_id.to_string())
                .or_insert(SurfaceEntry {
                    label,
                    loaded_url: None,
                });
        }
    }

    pub fn persistent_needs_navigation(&self, session_id: &str, target: &str, force: bool) -> bool {
        self.persistent
            .lock()
            .ok()
            .and_then(|surfaces| {
                surfaces
                    .get(session_id)
                    .map(|entry| force || entry.loaded_url.as_deref() != Some(target))
            })
            .unwrap_or(force)
    }

    pub fn record_persistent_navigation(&self, session_id: &str, target: &str) {
        if let Ok(mut surfaces) = self.persistent.lock() {
            if let Some(entry) = surfaces.get_mut(session_id) {
                entry.loaded_url = Some(target.to_string());
            }
        }
    }

    pub fn remove_persistent(&self, session_id: &str) -> Option<String> {
        self.persistent
            .lock()
            .ok()
            .and_then(|mut surfaces| surfaces.remove(session_id).map(|entry| entry.label))
    }

    pub fn layout(&self, selected_session_id: Option<&str>) -> SurfaceLayout {
        let mut active_label = None;
        let mut inactive_labels = Vec::new();
        if let Ok(surfaces) = self.persistent.lock() {
            for (session_id, entry) in surfaces.iter() {
                if selected_session_id == Some(session_id.as_str()) {
                    active_label = Some(entry.label.clone());
                } else {
                    inactive_labels.push(entry.label.clone());
                }
            }
        }
        inactive_labels.sort();
        SurfaceLayout {
            active_label,
            inactive_labels,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn singleton_navigation_tracks_only_successfully_recorded_target() {
        let registry = SurfaceRegistry::new();
        assert_eq!(
            registry.navigate_singleton("http://one/", false, || Ok::<_, ()>(())),
            Ok(true)
        );
        assert_eq!(
            registry.navigate_singleton("http://one/", false, || Ok::<_, ()>(())),
            Ok(false)
        );
        assert_eq!(
            registry.navigate_singleton("http://one/", true, || Ok::<_, ()>(())),
            Ok(true)
        );
        assert_eq!(
            registry.navigate_singleton("http://two/", false, || Err("failed")),
            Err("failed")
        );
        // A failed navigation does not become the remembered target.
        assert_eq!(
            registry.navigate_singleton("http://two/", false, || Ok::<_, ()>(())),
            Ok(true)
        );
        registry.clear_singleton();
        assert_eq!(
            registry.navigate_singleton("http://two/", false, || Ok::<_, ()>(())),
            Ok(true)
        );
    }

    #[test]
    fn persistent_surfaces_keep_identity_navigation_and_layout_separate() {
        let registry = SurfaceRegistry::new();
        registry.register_persistent("one", "editor:one".into());
        registry.register_persistent("two", "editor:two".into());

        assert_eq!(
            registry.persistent_label("one").as_deref(),
            Some("editor:one")
        );
        assert!(registry.persistent_needs_navigation("one", "http://one/", false));
        registry.record_persistent_navigation("one", "http://one/");
        assert!(!registry.persistent_needs_navigation("one", "http://one/", false));

        assert_eq!(
            registry.layout(Some("one")),
            SurfaceLayout {
                active_label: Some("editor:one".into()),
                inactive_labels: vec!["editor:two".into()],
            }
        );
        assert_eq!(
            registry.remove_persistent("one").as_deref(),
            Some("editor:one")
        );
        assert_eq!(registry.persistent_label("one"), None);
    }
}
