//! Map the GUI-agnostic [`InboxView`] into a JSON-serializable DTO the webview
//! frontend renders. Kept tiny and pure so the *logic* stays in the tested
//! `fleet-host-core` reducer — this is only presentation glue.

use serde::Serialize;

use fleet_host_core::{AgentIcon, InboxView, SessionTab, TabState};
use fleet_protocol::{Confidence, LocationGlyph, Urgency};

/// The whole inbox as the frontend consumes it.
#[derive(Debug, Clone, Default, Serialize)]
pub struct RenderedInbox {
    pub tabs: Vec<RenderedTab>,
    /// How many tabs are demanding attention (waiting) — drives the title badge.
    pub waiting_count: usize,
    /// Whether the Hub link is currently up (false ⇒ the window shows a banner).
    pub connected: bool,
}

/// One rendered session row.
#[derive(Debug, Clone, Serialize)]
pub struct RenderedTab {
    pub session_id: String,
    /// Location glyph (`laptop`/`docker`/`remote`) — serializes to its wire token.
    pub location: LocationGlyph,
    /// Agent flavor label (`claude`/`codex`/`agent`/``).
    pub agent: String,
    pub title: String,
    /// Lowercase state label (`working`/`waiting`/…).
    pub state: String,
    /// The state glyph the inbox draws (▶ ⏸ · ✓ ✕ ☠).
    pub state_glyph: String,
    /// `true` only for `waiting` — the one attention-demanding state.
    pub attention: bool,
    pub urgency: Option<Urgency>,
    /// Worst confidence among waiting runs (reported truthfully, never upgraded).
    pub confidence: Option<Confidence>,
    pub waiting_since: Option<String>,
    pub muted: bool,
    pub soloed: bool,
    pub unread: bool,
    pub run_count: usize,
    /// The rolled-up run's last message — the inbox preview line (what Claude
    /// said on idle/done, "Approve …?" on waiting). `None` when absent.
    pub last_message: Option<String>,
}

fn state_label(s: TabState) -> &'static str {
    match s {
        TabState::Working => "working",
        TabState::Waiting => "waiting",
        TabState::Idle => "idle",
        TabState::Done => "done",
        TabState::Error => "error",
        TabState::Dead => "dead",
    }
}

fn render_tab(t: &SessionTab) -> RenderedTab {
    RenderedTab {
        session_id: t.session_id.clone(),
        location: t.glyph.clone(),
        agent: agent_label(t.agent_icon).to_string(),
        title: t.title.clone(),
        state: state_label(t.state).to_string(),
        state_glyph: t.state.glyph().to_string(),
        attention: t.state.is_attention(),
        urgency: t.urgency,
        confidence: t.confidence,
        waiting_since: t.waiting_since.clone(),
        muted: t.muted,
        soloed: t.soloed,
        unread: t.unread,
        run_count: t.run_count,
        last_message: t.last_message.clone(),
    }
}

fn agent_label(a: AgentIcon) -> &'static str {
    a.label()
}

/// Render a whole inbox view, marking it connected/disconnected.
pub fn render(view: &InboxView, connected: bool) -> RenderedInbox {
    let tabs: Vec<RenderedTab> = view.tabs.iter().map(render_tab).collect();
    let waiting_count = tabs.iter().filter(|t| t.attention).count();
    RenderedInbox {
        tabs,
        waiting_count,
        connected,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fleet_host_core::InboxModel;
    use fleet_protocol::{
        AgentKind, AgentRun, Confidence, Event, Extra, Location, LocationGlyph, LocationKind,
        Server, ServerKind, Session, State,
    };

    fn session_with_run(id: &str, title: &str, state: State) -> Session {
        let mut s = Session::new(
            id,
            title,
            Location {
                kind: LocationKind::Local,
                label: "laptop".into(),
                glyph: LocationGlyph::Laptop,
                attach_hint: None,
                extra: Extra::new(),
            },
            Server {
                kind: ServerKind::Local,
                version: None,
                extra: Extra::new(),
            },
            State::Idle,
            "2026-06-08T00:00:00Z",
        );
        s.runs = vec![AgentRun::new(
            format!("{id}:run-1"),
            AgentKind::ClaudeCode,
            id,
            "/repo",
            state,
            Confidence::Inferred,
            "2026-06-08T00:00:00Z",
        )];
        // A real reporter keeps the session rollup consistent with its runs.
        s.rollup_state = state;
        s
    }

    #[test]
    fn renders_a_waiting_session_with_attention_and_glyph() {
        let mut model = InboxModel::new();
        model.apply(Event::snapshot(vec![session_with_run(
            "s1",
            "main",
            State::Waiting,
        )]));
        let r = render(&model.view(), true);
        assert_eq!(r.tabs.len(), 1);
        let t = &r.tabs[0];
        assert_eq!(t.title, "main");
        assert_eq!(t.state, "waiting");
        assert_eq!(t.state_glyph, "⏸");
        assert!(t.attention);
        assert_eq!(t.agent, "claude");
        assert_eq!(r.waiting_count, 1);
        assert!(r.connected);
    }

    #[test]
    fn surfaces_the_rolled_up_run_last_message_as_preview() {
        let mut model = InboxModel::new();
        let mut s = session_with_run("s1", "main", State::Idle);
        s.runs[0].last_message = Some("All tests pass.".into());
        model.apply(Event::snapshot(vec![s]));
        let r = render(&model.view(), true);
        assert_eq!(r.tabs[0].last_message.as_deref(), Some("All tests pass."));
    }

    #[test]
    fn working_session_is_not_attention() {
        let mut model = InboxModel::new();
        model.apply(Event::snapshot(vec![session_with_run(
            "s1",
            "main",
            State::Working,
        )]));
        let r = render(&model.view(), true);
        assert_eq!(r.tabs[0].state, "working");
        assert!(!r.tabs[0].attention);
        assert_eq!(r.waiting_count, 0);
    }

    #[test]
    fn serializes_to_json_the_frontend_can_read() {
        let mut model = InboxModel::new();
        model.apply(Event::snapshot(vec![session_with_run(
            "s1",
            "main",
            State::Working,
        )]));
        let json = serde_json::to_string(&render(&model.view(), true)).unwrap();
        assert!(json.contains("\"title\":\"main\""));
        assert!(json.contains("\"state\":\"working\""));
        assert!(json.contains("\"connected\":true"));
    }
}
