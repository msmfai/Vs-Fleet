//! Heavy round-trip property tests (test criterion: proptest round-trip on
//! EVERY `Session`/`AgentRun`/event/command variant).
//!
//! For each generated value `v`, we assert two invariants:
//!   1. `from_json(to_json(v)) == v`  (serialize→deserialize is identity)
//!   2. `to_json(from_json(to_json(v))) == to_json(v)`  (canonical-form stable)
//!
//! Strategies are built to exercise every enum variant and the presence/absence
//! of every optional field, including arbitrary unknown-field `extra` maps so
//! forward-compat carriage is covered by the property test, not just examples.

use fleet_protocol::commands::{Command, Target};
use fleet_protocol::events::Event;
use fleet_protocol::objects::{
    AgentKind, AgentRun, Editor, EditorKind, Extra, Location, LocationGlyph, LocationKind, Server,
    ServerKind, Session,
};
use fleet_protocol::state::{Confidence, State, Urgency};
use proptest::prelude::*;
use serde::de::DeserializeOwned;
use serde::Serialize;

/// Assert serialize→deserialize identity and canonical-form stability.
fn assert_round_trip<T>(v: &T)
where
    T: Serialize + DeserializeOwned + PartialEq + std::fmt::Debug,
{
    let json = serde_json::to_string(v).expect("serialize");
    let back: T = serde_json::from_str(&json).expect("deserialize");
    prop_assert_eq_panic(v, &back, &json);
    // Canonical form is stable across a second trip.
    let json2 = serde_json::to_string(&back).expect("re-serialize");
    assert_eq!(json, json2, "canonical JSON not stable");
}

fn prop_assert_eq_panic<T: PartialEq + std::fmt::Debug>(a: &T, b: &T, json: &str) {
    assert!(
        a == b,
        "round-trip mismatch\n  json: {json}\n  a: {a:?}\n  b: {b:?}"
    );
}

// ---- leaf strategies ----

fn state_strat() -> impl Strategy<Value = State> {
    prop::sample::select(State::ALL.to_vec())
}
fn urgency_strat() -> impl Strategy<Value = Urgency> {
    prop::sample::select(Urgency::ALL.to_vec())
}
fn confidence_strat() -> impl Strategy<Value = Confidence> {
    prop::sample::select(Confidence::ALL.to_vec())
}
fn agent_kind_strat() -> impl Strategy<Value = AgentKind> {
    prop_oneof![
        Just(AgentKind::ClaudeCode),
        Just(AgentKind::Codex),
        Just(AgentKind::Other),
    ]
}

/// Arbitrary unknown-field map (forward-compat carriage). Keys are restricted to
/// identifiers that cannot collide with known struct fields by construction
/// (prefixed `x_`), values are arbitrary JSON.
fn extra_strat() -> impl Strategy<Value = Extra> {
    prop::collection::btree_map("x_[a-z]{1,6}", json_value_strat(), 0..3)
}

/// A non-null arbitrary JSON value, for opaque optional fields (`diff_summary`,
/// `policy`). On the wire, `None` and `Some(Null)` are indistinguishable (both
/// mean "absent"), so generating a bare `Null` here would test an unreachable
/// distinction. We model "no value" as `None` and "a value" as a non-null JSON.
fn json_value_nonnull_strat() -> impl Strategy<Value = serde_json::Value> {
    json_value_strat().prop_filter("not bare null", |v| !v.is_null())
}

/// A small recursive arbitrary JSON value.
fn json_value_strat() -> impl Strategy<Value = serde_json::Value> {
    let leaf = prop_oneof![
        Just(serde_json::Value::Null),
        any::<bool>().prop_map(serde_json::Value::from),
        any::<i64>().prop_map(serde_json::Value::from),
        "[a-z]{0,8}".prop_map(serde_json::Value::from),
    ];
    leaf.prop_recursive(2, 6, 3, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 0..3).prop_map(serde_json::Value::from),
            prop::collection::btree_map("[a-z]{1,4}", inner, 0..3)
                .prop_map(|m| serde_json::Value::Object(m.into_iter().collect())),
        ]
    })
}

fn iso_strat() -> impl Strategy<Value = String> {
    // A handful of plausible ISO-8601 strings; the protocol treats them opaquely.
    prop::sample::select(vec![
        "2026-06-08T00:00:00Z".to_string(),
        "2026-01-01T12:34:56.789Z".to_string(),
        "1999-12-31T23:59:59+00:00".to_string(),
    ])
}

fn location_strat() -> impl Strategy<Value = Location> {
    (
        prop_oneof![
            Just(LocationKind::Local),
            Just(LocationKind::Docker),
            Just(LocationKind::Remote)
        ],
        ".*",
        prop_oneof![
            Just(LocationGlyph::Laptop),
            Just(LocationGlyph::Docker),
            Just(LocationGlyph::Remote)
        ],
        prop::option::of("[a-z/]{0,12}"),
        extra_strat(),
    )
        .prop_map(|(kind, label, glyph, attach_hint, extra)| Location {
            kind,
            label,
            glyph,
            attach_hint,
            extra,
        })
}

fn editor_strat() -> impl Strategy<Value = Editor> {
    (
        prop::option::of(prop_oneof![
            Just(EditorKind::Vscode),
            Just(EditorKind::Cursor),
            Just(EditorKind::Windsurf)
        ]),
        prop::option::of("[a-z -]{0,12}"),
        extra_strat(),
    )
        .prop_map(|(kind, focus_hint, extra)| Editor {
            kind,
            focus_hint,
            extra,
        })
}

fn server_strat() -> impl Strategy<Value = Server> {
    (
        prop_oneof![
            Just(ServerKind::CodeServer),
            Just(ServerKind::OpenvscodeServer),
            Just(ServerKind::DesktopRemote),
            Just(ServerKind::Local)
        ],
        prop::option::of("[0-9.]{1,6}"),
        extra_strat(),
    )
        .prop_map(|(kind, version, extra)| Server {
            kind,
            version,
            extra,
        })
}

fn run_strat() -> impl Strategy<Value = AgentRun> {
    (
        (
            "run-[a-z0-9]{1,8}",
            agent_kind_strat(),
            "[a-z0-9-]{1,10}",
            "[a-z/]{0,12}",
        ),
        state_strat(),
        prop::option::of(urgency_strat()),
        prop::option::of(".*"),
        prop::option::of(iso_strat()),
        confidence_strat(),
        prop::option::of(json_value_nonnull_strat()),
        iso_strat(),
        extra_strat(),
    )
        .prop_map(
            |(
                (run_id, agent_kind, native_id, cwd),
                state,
                urgency,
                last_message,
                waiting_since,
                confidence,
                diff_summary,
                updated_at,
                extra,
            )| AgentRun {
                schema_version: fleet_protocol::SCHEMA_VERSION,
                run_id,
                agent_kind,
                native_id,
                cwd,
                state,
                urgency,
                last_message,
                waiting_since,
                confidence,
                diff_summary,
                updated_at,
                extra,
            },
        )
}

fn session_strat() -> impl Strategy<Value = Session> {
    (
        ("sess-[a-z0-9]{1,8}", ".*"),
        location_strat(),
        prop::option::of(editor_strat()),
        server_strat(),
        prop::collection::vec(run_strat(), 0..4),
        state_strat(),
        prop::option::of(urgency_strat()),
        (any::<bool>(), any::<bool>(), any::<bool>()),
        prop::collection::vec("[a-z]{1,6}", 0..3),
        prop::option::of(json_value_nonnull_strat()),
        iso_strat(),
        extra_strat(),
    )
        .prop_map(
            |(
                (session_id, title),
                location,
                editor,
                server,
                runs,
                rollup_state,
                rollup_urgency,
                (muted, soloed, unread),
                tags,
                policy,
                updated_at,
                extra,
            )| Session {
                schema_version: fleet_protocol::SCHEMA_VERSION,
                session_id,
                title,
                location,
                editor,
                server,
                runs,
                rollup_state,
                rollup_urgency,
                muted,
                soloed,
                unread,
                tags,
                policy,
                updated_at,
                extra,
            },
        )
}

fn target_strat() -> impl Strategy<Value = Target> {
    prop_oneof![
        "[a-z0-9-]{1,10}".prop_map(Target::session),
        "[a-z0-9-]{1,10}".prop_map(Target::run),
    ]
}

fn event_strat() -> impl Strategy<Value = Event> {
    prop_oneof![
        prop::collection::vec(session_strat(), 0..3).prop_map(Event::snapshot),
        session_strat().prop_map(Event::session_added),
        session_strat().prop_map(Event::session_updated),
        "[a-z0-9-]{1,10}".prop_map(Event::session_removed),
        (("[a-z0-9-]{1,10}"), run_strat()).prop_map(|(s, r)| Event::run_added(s, r)),
        (("[a-z0-9-]{1,10}"), run_strat()).prop_map(|(s, r)| Event::run_updated(s, r)),
        (("[a-z0-9-]{1,10}"), "[a-z0-9-]{1,10}").prop_map(|(s, r)| Event::run_removed(s, r)),
    ]
}

fn command_strat() -> impl Strategy<Value = Command> {
    prop_oneof![
        target_strat().prop_map(Command::focus),
        "[a-z0-9-]{1,10}".prop_map(Command::mute),
        "[a-z0-9-]{1,10}".prop_map(Command::unmute),
        "[a-z0-9-]{1,10}".prop_map(Command::solo),
        target_strat().prop_map(Command::dismiss),
        (
            ("[a-z0-9-]{1,10}"),
            prop::collection::vec("[a-z]{1,5}", 0..4)
        )
            .prop_map(|(s, t)| Command::set_tags(s, t)),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    #[test]
    fn state_round_trips(s in state_strat()) { assert_round_trip(&s); }

    #[test]
    fn urgency_round_trips(u in urgency_strat()) { assert_round_trip(&u); }

    #[test]
    fn confidence_round_trips(c in confidence_strat()) { assert_round_trip(&c); }

    #[test]
    fn location_round_trips(l in location_strat()) { assert_round_trip(&l); }

    #[test]
    fn editor_round_trips(e in editor_strat()) { assert_round_trip(&e); }

    #[test]
    fn server_round_trips(s in server_strat()) { assert_round_trip(&s); }

    #[test]
    fn run_round_trips(r in run_strat()) { assert_round_trip(&r); }

    #[test]
    fn session_round_trips(s in session_strat()) { assert_round_trip(&s); }

    #[test]
    fn event_round_trips(e in event_strat()) { assert_round_trip(&e); }

    #[test]
    fn command_round_trips(c in command_strat()) { assert_round_trip(&c); }
}

// ---- Exhaustive (non-property) variant coverage: every enum token, explicitly,
//      so a renamed variant can't slip past with low proptest sampling. ----

#[test]
fn every_state_variant_round_trips() {
    for s in State::ALL {
        let j = serde_json::to_string(&s).unwrap();
        let back: State = serde_json::from_str(&j).unwrap();
        assert_eq!(s, back, "state {s:?}");
    }
}

#[test]
fn every_urgency_variant_round_trips() {
    for u in Urgency::ALL {
        let j = serde_json::to_string(&u).unwrap();
        let back: Urgency = serde_json::from_str(&j).unwrap();
        assert_eq!(u, back, "urgency {u:?}");
    }
}

#[test]
fn every_confidence_variant_round_trips() {
    for c in Confidence::ALL {
        let j = serde_json::to_string(&c).unwrap();
        let back: Confidence = serde_json::from_str(&j).unwrap();
        assert_eq!(c, back, "confidence {c:?}");
    }
}

#[test]
fn every_event_variant_serializes_with_its_tag() {
    let run = AgentRun::new(
        "r",
        AgentKind::Codex,
        "n",
        "/",
        State::Working,
        Confidence::High,
        "t",
    );
    let loc = Location {
        kind: LocationKind::Local,
        label: "l".into(),
        glyph: LocationGlyph::Laptop,
        attach_hint: None,
        extra: Extra::new(),
    };
    let srv = Server {
        kind: ServerKind::Local,
        version: None,
        extra: Extra::new(),
    };
    let sess = Session::new("s", "t", loc, srv, State::Idle, "t");
    let events = vec![
        Event::snapshot(vec![sess.clone()]),
        Event::session_added(sess.clone()),
        Event::session_updated(sess.clone()),
        Event::session_removed("s"),
        Event::run_added("s", run.clone()),
        Event::run_updated("s", run.clone()),
        Event::run_removed("s", "r"),
    ];
    for e in events {
        let j = serde_json::to_value(&e).unwrap();
        assert_eq!(j["type"], e.type_name());
        let back: Event = serde_json::from_value(j).unwrap();
        assert_eq!(e, back);
    }
}

#[test]
fn every_command_variant_serializes_with_its_tag() {
    let cmds = vec![
        Command::focus(Target::run("r")),
        Command::mute("s"),
        Command::unmute("s"),
        Command::solo("s"),
        Command::dismiss(Target::session("s")),
        Command::set_tags("s", vec!["a".into()]),
    ];
    for c in cmds {
        let j = serde_json::to_value(&c).unwrap();
        assert_eq!(j["command"], c.command_name());
        let back: Command = serde_json::from_value(j).unwrap();
        assert_eq!(c, back);
    }
}
