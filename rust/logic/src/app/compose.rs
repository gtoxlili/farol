//! A new agent session from the board, part of [`App`]: the compose page,
//! push-to-talk through Paseo's dictation (hold the AI key or the page's
//! button), typing, and `farol.start`. The page is drawn in `view.rs`.

use alloc::string::String;

use super::{App, Entry, Overlay};
use crate::dictation::Heard;
use crate::lang::Lang;
use crate::model::{ActResult, Start};
use crate::sys::{Haptic, Sys};
use crate::tr;
use crate::ui::Ui;

/// The longest recording the board takes.
pub(crate) const LISTEN_MAX_MS: u64 = 60_000;
/// Demo: how long the pretend transcription takes.
const DEMO_TRANSCRIBE_MS: u64 = 900;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub(crate) enum Voice {
    #[default]
    Idle,
    /// Recording while the key or the button is held.
    Listening,
    /// Let go: Paseo is transcribing.
    Transcribing,
    /// `farol.start` went out.
    Sending,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct Compose {
    pub text: String,
    pub voice: Voice,
    /// Why the last step did not work.
    pub note: Option<String>,
    /// The `farol.start` in flight.
    pub token: Option<u32>,
    /// Demo: when the pretend transcript arrives.
    pub demo_heard_at: Option<u64>,
    /// Recording stops here at the latest.
    pub listen_until: Option<u64>,
}

impl App {
    /// The compose page's buttons and the welcome page's language switch;
    /// false when `action` is not one of them.
    pub(super) fn compose_call(
        &mut self,
        action: u32,
        ui: &mut impl Ui,
        sys: &mut impl Sys,
    ) -> bool {
        use gsp_sys::*;
        match action {
            GSP_ACT_ID_MIC_PRESS => self.talk(sys),
            GSP_ACT_ID_MIC_RELEASE => self.talked(sys),
            GSP_ACT_ID_CP_KEYS => self.type_prompt(ui, sys),
            GSP_ACT_ID_CP_SEND => self.send_compose(sys),
            GSP_ACT_ID_CP_CANCEL => self.close_compose(sys),
            GSP_ACT_ID_LANG_TOGGLE => {
                self.prefs.lang = self.prefs.lang.other();
                sys.save_prefs(&self.prefs);
                self.feel(Haptic::Tick, sys);
            }
            _ => return false,
        }
        true
    }

    /// Why a session cannot start from the board now, in both languages.
    pub(crate) fn compose_blocked(&self) -> Option<(&'static str, &'static str)> {
        if self.demo.is_none() && !self.session.is_ready() {
            Some((
                "连上 Paseo 后才能开始新会话",
                "Connect to Paseo to start a session",
            ))
        } else if self.snapshot.read_only {
            Some((
                "只读模式：在 Paseo 里开始新会话",
                "Read-only: start sessions in Paseo",
            ))
        } else if self.snapshot.compose.is_none() {
            Some((
                "先在 Paseo 的 Farol 设置里选一个默认 workspace",
                "Pick a default workspace in Paseo's Farol settings first",
            ))
        } else {
            None
        }
    }

    /// Paseo can take dictation for a new session now.
    pub(crate) fn can_talk(&self) -> bool {
        self.compose_blocked().is_none() && (self.demo.is_some() || self.session.can_dictate())
    }

    pub(super) fn open_compose(&mut self) {
        if self.overlay != Overlay::Compose {
            self.overlay = Overlay::Compose;
            if self.compose.voice == Voice::Idle {
                self.compose.note = None;
            }
        }
    }

    /// The AI key held, or the page's button pressed: start recording.
    pub(super) fn talk(&mut self, sys: &mut impl Sys) {
        self.open_compose();
        if self.compose.voice != Voice::Idle || !self.can_talk() {
            return;
        }
        if self.demo.is_none() {
            if !self.session.dictate() {
                return;
            }
            sys.listen(true);
        }
        self.compose.voice = Voice::Listening;
        self.compose.note = None;
        self.compose.listen_until = Some(self.now + LISTEN_MAX_MS);
        self.feel(Haptic::Confirm, sys);
    }

    /// Let go: Paseo transcribes what was said (the platform's last audio
    /// and [`App::audio_end`] follow).
    pub(super) fn talked(&mut self, sys: &mut impl Sys) {
        if self.compose.voice != Voice::Listening {
            return;
        }
        self.compose.voice = Voice::Transcribing;
        self.compose.listen_until = None;
        if self.demo.is_some() {
            self.compose.demo_heard_at = Some(self.now + DEMO_TRANSCRIBE_MS);
        } else {
            sys.listen(false);
        }
        self.feel(Haptic::Tick, sys);
    }

    pub(super) fn audio(&mut self, pcm: &[u8]) {
        if matches!(self.compose.voice, Voice::Listening | Voice::Transcribing) {
            self.session.dictate_audio(pcm);
        }
    }

    /// The recording ended (let go, or the board's limit).
    pub(super) fn audio_end(&mut self) {
        if matches!(self.compose.voice, Voice::Listening | Voice::Transcribing) {
            self.compose.voice = Voice::Transcribing;
            self.compose.listen_until = None;
            self.session.dictate_end(self.now);
        }
    }

    /// Time-driven compose work: the recording limit, the demo's transcript.
    pub(super) fn poll_compose(&mut self, sys: &mut impl Sys) {
        if self.compose.listen_until.is_some_and(|at| self.now >= at) {
            self.talked(sys);
        }
        if self.compose.demo_heard_at.is_some_and(|at| self.now >= at) {
            self.compose.demo_heard_at = None;
            let text = self.lang().pick(
                "把登录页的报错提示改得友好一点，再补一个测试",
                "Make the login page's error message friendlier and add a test",
            );
            self.heard(Heard::Final(String::from(text)), sys);
        }
    }

    pub(super) fn compose_deadline(&self) -> u64 {
        let listen = self.compose.listen_until.unwrap_or(u64::MAX);
        listen.min(self.compose.demo_heard_at.unwrap_or(u64::MAX))
    }

    /// What Paseo's dictation heard.
    pub(super) fn heard(&mut self, heard: Heard, sys: &mut impl Sys) {
        let l = self.lang();
        match heard {
            Heard::Partial(text) => {
                if self.compose.voice != Voice::Idle && !text.is_empty() {
                    self.compose.text = text;
                }
            }
            Heard::Final(text) => {
                self.compose.voice = Voice::Idle;
                if text.is_empty() {
                    self.compose.note = Some(String::from(
                        l.pick("没有听清，再说一次", "Didn't catch that; try again"),
                    ));
                } else {
                    self.compose.text = text;
                    self.feel(Haptic::Tick, sys);
                }
            }
            Heard::Failed(why) => {
                if self.compose.voice == Voice::Listening {
                    sys.listen(false);
                }
                self.compose.voice = Voice::Idle;
                self.compose.listen_until = None;
                self.compose.note = Some(match why.as_str() {
                    "timeout" => String::from(
                        l.pick("转写超时，再试一次", "Transcription timed out; try again"),
                    ),
                    "disconnected" => String::from(l.pick("连接断了", "Disconnected")),
                    _ => tr!(l, "语音输入没成功：{}", "Dictation failed: {}", why),
                });
                self.feel(Haptic::Fail, sys);
            }
        }
    }

    /// The keyboard page for the prompt.
    pub(super) fn type_prompt(&mut self, ui: &mut impl Ui, sys: &mut impl Sys) {
        if self.compose.voice != Voice::Idle {
            return;
        }
        if self.compose_blocked().is_some() {
            self.feel(Haptic::Fail, sys);
            return;
        }
        self.open_input(Entry::Prompt, ui);
    }

    /// Typed text from the keyboard page.
    pub(super) fn typed(&mut self, text: String) {
        if !text.is_empty() {
            self.compose.text = text;
        }
        self.compose.note = None;
        self.overlay = Overlay::Compose;
    }

    /// "发送": start the session.
    pub(super) fn send_compose(&mut self, sys: &mut impl Sys) {
        if self.compose.text.is_empty() || self.compose.voice != Voice::Idle {
            return;
        }
        if let Some((zh, en)) = self.compose_blocked() {
            self.compose.note = Some(String::from(self.lang().pick(zh, en)));
            self.feel(Haptic::Fail, sys);
            return;
        }
        let start = Start {
            device: alloc::format!("farol-{}", self.device),
            text: self.compose.text.clone(),
        };
        self.next_token = self.next_token.wrapping_add(1);
        let token = self.next_token;
        if let Some(demo) = &mut self.demo {
            let result = demo.start(&start, self.now);
            self.compose.token = Some(token);
            self.started(token, &result, sys);
            self.tick_demo(sys);
        } else if self.session.start(&start, token) {
            self.compose.token = Some(token);
            self.compose.voice = Voice::Sending;
            self.feel(Haptic::Confirm, sys);
        } else {
            let l = self.lang();
            self.compose.note = Some(String::from(l.pick(
                "未连接 Paseo，没有发出去",
                "Not connected to Paseo; nothing sent",
            )));
            self.feel(Haptic::Fail, sys);
        }
    }

    /// The plugin's answer to `farol.start`; false when `token` is not it.
    pub(super) fn started(&mut self, token: u32, result: &ActResult, sys: &mut impl Sys) -> bool {
        if self.compose.token != Some(token) {
            return false;
        }
        self.compose.token = None;
        self.compose.voice = Voice::Idle;
        let l = self.lang();
        if result.ok {
            self.compose.text.clear();
            self.compose.note = None;
            if self.overlay == Overlay::Compose {
                self.overlay = Overlay::None;
            }
            // Its card shows as soon as a snapshot carries it.
            self.show_next = result.agent.clone();
            self.feel(Haptic::Confirm, sys);
            return true;
        }
        self.compose.note = Some(match result.reason.as_str() {
            "no_workspace" => String::from(l.pick(
                "先在 Paseo 的 Farol 设置里选一个默认 workspace",
                "Pick a default workspace in Paseo's Farol settings first",
            )),
            "read_only" => String::from(l.pick(
                "只读模式：在 Paseo 里开始新会话",
                "Read-only: start sessions in Paseo",
            )),
            "disconnected" => {
                String::from(l.pick("连接断了，没有发出去", "Disconnected; nothing sent"))
            }
            _ if result.message.is_empty() => String::from(l.pick("没有开始", "Didn't start")),
            _ => tr!(l, "没有开始：{}", "Didn't start: {}", result.message),
        });
        self.feel(Haptic::Fail, sys);
        true
    }

    /// "取消": drop the recording and the page.
    pub(super) fn close_compose(&mut self, sys: &mut impl Sys) {
        if self.compose.voice == Voice::Listening && self.demo.is_none() {
            sys.listen(false);
        }
        self.session.dictate_cancel();
        let keep = self.compose.token;
        self.compose = Compose {
            token: keep,
            ..Compose::default()
        };
        self.overlay = Overlay::None;
    }

    /// The compose page's caption: where the session will start.
    pub(crate) fn compose_caption(&self, l: Lang) -> String {
        match &self.snapshot.compose {
            Some(c) => tr!(l, "在 {} 里开始", "Starts in {}", c.workspace),
            None => String::new(),
        }
    }
}
