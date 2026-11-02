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
    /// The running turn's id; a stop names it so only that turn stops.
    #[serde(default)]
    pub turn: Option<String>,
}

#[derive(Deserialize, Clone, Debug, PartialEq)]
pub struct Window {
    /// "Claude · 本窗".
    pub label: String,
    /// The provider the window belongs to ("Claude"); absent from older
    /// plugins, where `label` carries it before " · ".
    #[serde(default)]
    pub group: Option<String>,
    /// The window's own name ("Session"); absent from older plugins.
    #[serde(default)]
    pub name: Option<String>,
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
    /// Daemon host label ("studio-mac").
    #[serde(default)]
    pub host: String,
    /// Board actions are disabled in the plugin's settings.
    #[serde(default)]
    pub read_only: bool,
    /// Nothing changed before the poll timed out; only `rev`/`now` are meaningful.
    #[serde(default)]
    pub idle: bool,
    /// Board settings made in Paseo; absent from plugins that have none yet.
    #[serde(default)]
    pub settings: Option<Settings>,
    #[serde(default)]
    pub agents: Vec<Agent>,
    #[serde(default)]
    pub usage: Vec<Window>,
    #[serde(default)]
    pub fuel: Option<Fuel>,
    /// The Paseo app's language for the board: "zh" or "en"; absent from
    /// plugins before it, which leaves the board's own.
    #[serde(default)]
    pub lang: Option<String>,
    /// Where a session started on the board goes; absent when no default
    /// workspace is set in the plugin or boards are read-only.
    #[serde(default)]
    pub compose: Option<Compose>,
}

/// New sessions from the board (`farol.start`) start in this workspace.
#[derive(Deserialize, Clone, Debug, PartialEq, Default)]
pub struct Compose {
    /// The workspace's name, for the board to show.
    pub workspace: String,
    /// The agent profile's name; empty from plugins before it.
    #[serde(default)]
    pub profile: String,
}

/// `farol.start` input: a new agent session in the default workspace, with
/// `text` (typed or dictated on the board) as its first prompt.
#[derive(Serialize, Debug, PartialEq)]
pub struct Start {
    pub device: String,
    pub text: String,
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
    /// The board acting, for Paseo's timeline ("farol-3fa2").
    pub device: String,
    pub kind: ActKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub choice: Option<u8>,
    /// For a stop: the turn the board showed (`Agent::turn`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turn: Option<String>,
}

/// The plugin's answer to `farol.act` and `farol.start`.
#[derive(Deserialize, Clone, Debug, PartialEq, Default)]
pub struct ActResult {
    pub ok: bool,
    /// "already_resolved", "read_only", "not_found", "failed"; for a start
    /// also "no_workspace".
    #[serde(default)]
    pub reason: String,
    #[serde(default)]
    pub message: String,
    /// `farol.start`: the new agent.
    #[serde(default)]
    pub agent: Option<String>,
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
    /// The screen in front: "usage", "home" or "deck".
    pub screen: &'a str,
    /// Firmware version, for the plugin's list of boards.
    pub fw: &'a str,
    /// Settings changed on the board that the plugin has not confirmed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prefs: Option<BoardPrefs>,
}

/// `farol.focus` input: wakes a waiting `farol.next` with the new focus, and
/// carries settings changed on the board at once.
#[derive(Serialize, Debug)]
pub struct Focus<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub focus: Option<&'a str>,
    pub device: &'a str,
    /// The screen in front: "usage", "home" or "deck".
    pub screen: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prefs: Option<BoardPrefs>,
}

/// Settings changed on the board (control centre), for the plugin to mirror
/// in Paseo. Sent until a `farol.next` answer confirms them.
#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BoardPrefs {
    pub sound: bool,
    pub haptic: bool,
    /// Screen brightness, 5..=100.
    pub brightness: u8,
    /// Do not disturb is on.
    pub dnd: bool,
}

/// Board settings made in Paseo (the plugin's settings page), for every board.
#[derive(Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    /// Increases with every change made in Paseo; boards apply a newer one.
    pub rev: u64,
    pub sound: bool,
    /// Interaction-module LED brightness, 0..=100.
    pub led_brightness: u8,
    /// Quiet hours, minutes after local midnight: no sound or vibration from
    /// `quiet_start` until `quiet_end` (may wrap past midnight; equal = none).
    #[serde(default)]
    pub quiet_start: u16,
    #[serde(default)]
    pub quiet_end: u16,
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
    fn usage_windows_take_an_optional_group_and_name() {
        let s: Snapshot = serde_json::from_str(
            r#"{"v":1,"rev":1,"now":0,"usage":[
                {"label":"Claude · Session","used":55},
                {"label":"GLM · 5-hour","group":"GLM Coding Plan","name":"5-hour","used":0}]}"#,
        )
        .unwrap();
        assert_eq!(s.usage[0].group, None);
        assert_eq!(s.usage[1].group.as_deref(), Some("GLM Coding Plan"));
        assert_eq!(s.usage[1].name.as_deref(), Some("5-hour"));
    }

    #[test]
    fn a_stop_names_the_turn_it_saw() {
        let stop = Act {
            agent: "a".into(),
            device: "farol-3a1f".into(),
            kind: ActKind::Stop,
            request_id: None,
            choice: None,
            turn: Some("t-7".into()),
        };
        assert_eq!(
            serde_json::to_string(&stop).unwrap(),
            r#"{"agent":"a","device":"farol-3a1f","kind":"stop","turn":"t-7"}"#
        );
    }

    #[test]
    fn act_serializes_compactly() {
        let a = Act {
            agent: "a".into(),
            device: "farol-3a1f".into(),
            kind: ActKind::Answer,
            request_id: Some("r".into()),
            choice: Some(1),
            turn: None,
        };
        assert_eq!(
            serde_json::to_string(&a).unwrap(),
            r#"{"agent":"a","device":"farol-3a1f","kind":"answer","requestId":"r","choice":1}"#
        );
    }
}
