//! farol/1: what the Farol plugin tells the board, and what the board asks
//! of it. The plugin digests Paseo's own data into these shapes, so the board
//! never depends on Paseo's internal schema. The specification and shared
//! fixtures are in `protocol/` at the repository root.

use alloc::string::String;
use alloc::vec::Vec;

use serde::{Deserialize, Serialize};

/// Protocol version the board speaks.
pub const VERSION: u32 = 1;

/// How an agent is doing, in order of urgency (the deck's sort order).
#[derive(Deserialize, Serialize, Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Default)]
#[serde(rename_all = "lowercase")]
pub enum State {
    /// Waiting for the user: a question, a permission or a plan.
    Call,
    /// The last turn failed.
    Fail,
    /// The last turn finished and the user has not looked yet.
    Done,
    /// A turn is running.
    Work,
    /// Finished and looked at.
    #[default]
    Seen,
}

#[derive(Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum AskKind {
    Question,
    Permission,
    Plan,
}

/// Something the agent waits on. `options` are the choices to show, in the
/// order the plugin maps `choice` indices back to Paseo answers.
#[derive(Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Ask {
    pub kind: AskKind,
    pub request_id: String,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub options: Vec<String>,
}

#[derive(Deserialize, Clone, Debug, PartialEq)]
pub struct Todo {
    pub done: u16,
    pub total: u16,
    #[serde(default)]
    pub current: String,
}

#[derive(Deserialize, Clone, Debug, PartialEq, Default)]
pub struct Agent {
    pub id: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub project: String,
    /// Short model label, e.g. "opus 5.5".
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub state: State,
    /// Epoch ms the current turn started (work) or the last one ended.
    #[serde(default)]
    pub since: Option<i64>,
    /// The action in progress, e.g. "cargo test -p runtime".
    #[serde(default)]
    pub now: String,
    /// Up to three earlier actions, newest first.
    #[serde(default)]
    pub recent: Vec<String>,
    #[serde(default)]
    pub todo: Option<Todo>,
    /// Context window used, percent.
    #[serde(default)]
    pub ctx: Option<u8>,
    /// Cost of the last turn, already formatted ("$0.42").
    #[serde(default)]
    pub cost: Option<String>,
    #[serde(default)]
    pub ask: Option<Ask>,
    /// Preview of the last reply (done) or what happened (fail).
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub error: Option<String>,
    /// The plugin can stop this agent's turn.
    #[serde(default)]
    pub stoppable: bool,
}

#[derive(Deserialize, Clone, Debug, PartialEq)]
pub struct Window {
    /// "Claude · 本窗".
    pub label: String,
    /// Percent used.
    pub used: u8,
    /// Epoch ms the window resets.
    #[serde(default)]
    pub resets: Option<i64>,
}

/// The tightest quota, shown as the lamp's level.
#[derive(Deserialize, Clone, Debug, PartialEq)]
pub struct Fuel {
    pub label: String,
    /// Percent left.
    pub left: u8,
}

/// The plugin's answer to `farol.next`: everything the board shows.
#[derive(Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub v: u32,
    /// Increases whenever anything changed; the board's poll cursor.
    pub rev: u64,
    /// The plugin's clock, epoch ms, when it answered.
    pub now: i64,
    /// Display time zone, minutes east of UTC.
    #[serde(default)]
    pub tz: i32,
    /// Daemon host label ("Gt's Mac mini").
    #[serde(default)]
    pub host: String,
    /// Board actions are disabled in the plugin's settings.
    #[serde(default)]
    pub read_only: bool,
    /// Nothing changed before the poll timed out; only `rev`/`now` are meaningful.
    #[serde(default)]
    pub idle: bool,
    #[serde(default)]
    pub agents: Vec<Agent>,
    #[serde(default)]
    pub usage: Vec<Window>,
    #[serde(default)]
    pub fuel: Option<Fuel>,
}

/// What the board asks the plugin to do (`farol.act`).
#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ActKind {
    /// Pick option `choice` of the agent's pending ask.
    Answer,
    /// Stop the running turn.
    Stop,
    /// The user looked at a finished or failed agent on the board.
    Seen,
}

#[derive(Serialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Act {
    pub agent: String,
    pub kind: ActKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub choice: Option<u8>,
}

/// The plugin's answer to `farol.act`.
#[derive(Deserialize, Clone, Debug, PartialEq, Default)]
pub struct ActResult {
    pub ok: bool,
    /// "already_resolved", "read_only", "not_found", "failed".
    #[serde(default)]
    pub reason: String,
    #[serde(default)]
    pub message: String,
}

/// `farol.next` input: long-poll for anything newer than `cursor`.
#[derive(Serialize, Debug)]
pub struct Next<'a> {
    pub v: u32,
    pub cursor: u64,
    /// The agent whose card is on screen; the plugin sends its live detail.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub focus: Option<&'a str>,
    pub device: &'a str,
}

/// `farol.focus` input: wakes a waiting `farol.next` with the new focus.
#[derive(Serialize, Debug)]
pub struct Focus<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub focus: Option<&'a str>,
    pub device: &'a str,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_fixture_parses() {
        let s: Snapshot =
            serde_json::from_str(include_str!("../../../protocol/fixtures/snapshot.json")).unwrap();
        assert_eq!(s.v, VERSION);
        assert!(
            s.agents
                .iter()
                .any(|a| a.state == State::Call && a.ask.is_some())
        );
        assert!(
            s.agents
                .iter()
                .any(|a| a.state == State::Work && a.todo.is_some())
        );
    }

    #[test]
    fn unknown_fields_are_ignored_and_missing_ones_default() {
        let s: Snapshot = serde_json::from_str(
            r#"{"v":1,"rev":3,"now":5,"agents":[{"id":"a","x":1}],"later":true}"#,
        )
        .unwrap();
        assert_eq!(s.agents[0].state, State::Seen);
        assert!(s.usage.is_empty());
    }

    #[test]
    fn act_serializes_compactly() {
        let a = Act {
            agent: "a".into(),
            kind: ActKind::Answer,
            request_id: Some("r".into()),
            choice: Some(1),
        };
        assert_eq!(
            serde_json::to_string(&a).unwrap(),
            r#"{"agent":"a","kind":"answer","requestId":"r","choice":1}"#
        );
    }
}
