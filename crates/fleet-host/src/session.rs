//! Fleet's thin-shell description of one VS Code-backed work environment.
//!
//! A session is deliberately small: Fleet needs enough information to list,
//! select, focus, rename, and (when locally owned) close an opaque VS Code
//! surface. Editing, terminals, source control, tasks, and project semantics
//! remain inside VS Code.

use serde::{Deserialize, Serialize};

/// One VS Code work environment that Fleet can switch to.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Session {
    /// Stable identity shared by the runtime, bridge, reporter, and rail.
    pub id: String,
    /// Display label shown in the rail.
    pub label: String,
    /// The opaque VS Code web URL Fleet embeds.
    pub url: String,
    /// Whether Fleet owns the backing process/container and can stop it.
    pub owned: bool,
    /// Whether the user explicitly replaced the reported/default label.
    #[serde(default)]
    pub renamed: bool,
}
