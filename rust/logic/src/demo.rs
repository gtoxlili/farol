//! Built-in sample agents that act like a small Paseo fleet, for the
//! simulator and for trying the board without a daemon. A 90-second script
//! repeats: an agent works through a todo list and finishes, another asks a
//! question, one failed on its usage limit. Answers and stops on the cards
//! change the samples the way the real plugin would.

use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

use crate::lang::Lang;
use crate::model::{
    Act, ActKind, ActResult, Agent, Ask, AskKind, Compose, Fuel, Snapshot, Start, State, Todo,
    Window,
};

/// The demo's wall clock when the real time is unknown: 2026-11-02 09:41 in UTC+8.
pub const EPOCH_MS: i64 = 1_793_583_660_000;
const LOOP_MS: u64 = 90_000;
const ASK_AT_MS: u64 = 12_000;
const FINISH_AT_MS: u64 = 70_000;
const STEP_MS: u64 = 2_500;
const MINUTE: i64 = 60_000;

const COMMANDS: [&str; 6] = [
    "cargo test -p runtime --lib",
    "edit  src/agent/spawn.rs",
    "grep  SubAgentError",
    "cargo clippy --all-targets",
    "read  docs/agents.md",
    "edit  tests/spawn_retry.rs",
];

pub struct Demo {
    start: u64,
    offset: u64,
    epoch: i64,
    rev: u64,
    emitted: Option<(u64, u64)>,
    /// Loop number in which the question was answered, and the choice.
    answered: Option<(u64, u8)>,
    /// Agent id to (loop, script time) of the stop.
    stopped: BTreeMap<String, (u64, u64)>,
    lang: Lang,
    /// Sessions started from the board: (first prompt, wall-clock start).
    started: Vec<(String, i64)>,
}

impl Demo {
    /// `now` is the local clock; `epoch` the wall-clock time to pretend it is;
    /// `offset` starts the script that far in (simulator checks jump ahead).
    pub fn new(now: u64, epoch: i64, offset: u64, lang: Lang) -> Self {
        Self {
            start: now,
            offset,
            epoch: epoch - offset as i64,
            rev: 0,
            emitted: None,
            answered: None,
            stopped: BTreeMap::new(),
            lang,
            started: Vec::new(),
        }
    }

    /// `farol.start`: a new session in the demo's workspace, working at once.
    pub fn start(&mut self, start: &Start, now: u64) -> ActResult {
        let epoch = self.epoch + self.elapsed(now) as i64;
        self.started.push((start.text.clone(), epoch));
        self.emitted = None;
        ActResult {
            ok: true,
            agent: Some(alloc::format!("new-{}", self.started.len())),
            ..Default::default()
        }
    }

    /// Script time: local time since start, plus the starting offset.
    fn elapsed(&self, now: u64) -> u64 {
        now.saturating_sub(self.start) + self.offset
    }

    fn phase(&self, now: u64) -> (u64, u64) {
        let t = self.elapsed(now);
        (t / LOOP_MS, t % LOOP_MS)
    }

    /// The state that changes with time, as a key: emit when it moves.
    fn key(&self, now: u64) -> (u64, u64) {
        let (lap, t) = self.phase(now);
        (lap, t / STEP_MS)
    }

    /// The next snapshot when anything changed since the last call.
    pub fn snapshot(&mut self, now: u64) -> Option<Snapshot> {
        let key = self.key(now);
        if self.emitted == Some(key) {
            return None;
        }
        self.emitted = Some(key);
        self.rev += 1;
        Some(self.build(now))
    }

    /// When the script next moves, on the local clock.
    pub fn next_change(&self) -> u64 {
        match self.emitted {
            Some((lap, step)) => {
                (self.start + lap * LOOP_MS + (step + 1) * STEP_MS).saturating_sub(self.offset)
            }
            None => self.start,
        }
    }

    pub fn act(&mut self, act: &Act, now: u64) -> ActResult {
        let (lap, t) = self.phase(now);
        let current = self.build(now);
        let Some(agent) = current.agents.iter().find(|a| a.id == act.agent) else {
            return ActResult {
                ok: false,
                reason: "not_found".into(),
                message: String::new(),
                ..Default::default()
            };
        };
        let pending = agent.ask.as_ref().map(|ask| &ask.request_id);
        let accepted = match act.kind {
            ActKind::Answer => pending.is_some() && pending == act.request_id.as_ref(),
            ActKind::Stop => agent.state == State::Work && agent.stoppable,
            ActKind::Seen => true,
        };
        if !accepted {
            return ActResult {
                ok: false,
                reason: "already_resolved".into(),
                message: String::new(),
                ..Default::default()
            };
        }
        match act.kind {
            ActKind::Answer => self.answered = Some((lap, act.choice.unwrap_or(0))),
            ActKind::Stop => {
                self.stopped.insert(act.agent.clone(), (lap, t));
            }
            ActKind::Seen => {
                return ActResult {
                    ok: true,
                    ..Default::default()
                };
            }
        }
        self.emitted = None;
        ActResult {
            ok: true,
            ..Default::default()
        }
    }

    fn build(&self, now: u64) -> Snapshot {
        let l = self.lang;
        let (lap, t) = self.phase(now);
        let epoch = self.epoch + self.elapsed(now) as i64;
        let lap_start = epoch - t as i64;
        let mut agents = Vec::new();

        // farol: works, then asks which orb style; answering sends it back to work.
        let answered = self.answered.filter(|(l, _)| *l == lap).map(|(_, c)| c);
        let stop_at = |id: &str| {
            self.stopped
                .get(id)
                .filter(|(l, _)| *l == lap)
                .map(|(_, at)| *at)
        };
        let farol = if let Some(at) = stop_at("farol") {
            Agent {
                state: State::Done,
                since: Some(lap_start + at as i64),
                body: l.pick("这一轮被你停下了。三种光球的截图还在 docs/orbs，回到 Paseo 里可以接着选。", "You stopped this turn. The three orb screenshots are in docs/orbs; pick one in Paseo to carry on.")
                    .into(),
                ctx: Some(18),
                cost: Some("$0.12".into()),
                ..base("farol", l.pick("给 Farol 首页选一种光球动效", "Pick an orb animation for Farol's home page"), "farol", "opus 5.5")
            }
        } else if t < ASK_AT_MS || answered.is_some() {
            Agent {
                now: if answered.is_some() {
                    "edit  components/app_logic/ui/main.json".into()
                } else {
                    "read  docs/design.md".into()
                },
                state: State::Work,
                since: Some(
                    lap_start
                        + if answered.is_some() {
                            ASK_AT_MS as i64
                        } else {
                            0
                        },
                ),
                ctx: Some(18),
                stoppable: true,
                ..base(
                    "farol",
                    l.pick(
                        "给 Farol 首页选一种光球动效",
                        "Pick an orb animation for Farol's home page",
                    ),
                    "farol",
                    "opus 5.5",
                )
            }
        } else {
            Agent {
                state: State::Call,
                since: Some(lap_start + ASK_AT_MS as i64),
                ask: Some(Ask {
                    kind: AskKind::Question,
                    request_id: alloc::format!("q-{lap}"),
                    text: l.pick("首页的光球用哪种风格？三种都在模拟器里渲染过，截图在 docs/orbs。", "Which orb style for the home page? All three are rendered in the simulator; screenshots in docs/orbs.").into(),
                    options: vec![
                        l.pick("光环 · 底部积光", "Halo · light pooling below").into(),
                        l.pick("等离子 · 冷暖渐变", "Plasma · warm and cool").into(),
                        l.pick("只留文字，不要光球", "Text only, no orb").into(),
                    ],
                }),
                ..base("farol", l.pick("给 Farol 首页选一种光球动效", "Pick an orb animation for Farol's home page"), "farol", "opus 5.5")
            }
        };
        agents.push(farol);

        // runtime: a todo list over the loop, then done (or stopped).
        let stopped = stop_at("runtime");
        let runtime = if stopped.is_some() || t >= FINISH_AT_MS {
            Agent {
                state: State::Done,
                since: Some(lap_start + stopped.unwrap_or(FINISH_AT_MS) as i64),
                body: if stopped.is_some() {
                    l.pick("这一轮被你停下了。已经改好的 spawn.rs 留在工作区里，测试还没跑完。", "You stopped this turn. The spawn.rs changes stay in the workspace; the tests didn't finish.").into()
                } else {
                    l.pick("修好了：子 agent 在父进程退出时没有收到取消信号，重试时又复用了旧的 session id。补了 3 个回归测试，全部通过。", "Fixed: sub-agents missed the cancel signal when the parent exited, and retries reused the old session id. Added 3 regression tests; all pass.").into()
                },
                ctx: Some(46),
                cost: Some("$0.86".into()),
                ..base(
                    "runtime",
                    l.pick(
                        "修复子 agent 调用失败，补回归测试",
                        "Fix failing sub-agent calls and add regression tests",
                    ),
                    "api-server",
                    "gpt-6.1 sol",
                )
            }
        } else {
            let step = (t / STEP_MS) as usize;
            let done = (3 + t * 4 / FINISH_AT_MS) as u16;
            Agent {
                state: State::Work,
                since: Some(lap_start - 9 * MINUTE),
                now: COMMANDS[step % COMMANDS.len()].into(),
                recent: (1..=3)
                    .map(|k| COMMANDS[(step + COMMANDS.len() - k) % COMMANDS.len()].to_string())
                    .collect(),
                todo: Some(Todo {
                    done,
                    total: 7,
                    current: l.pick("正在补回归测试", "Adding regression tests").into(),
                }),
                ctx: Some(41),
                stoppable: true,
                ..base(
                    "runtime",
                    l.pick(
                        "修复子 agent 调用失败，补回归测试",
                        "Fix failing sub-agent calls and add regression tests",
                    ),
                    "api-server",
                    "gpt-6.1 sol",
                )
            }
        };
        agents.push(runtime);

        agents.push(Agent {
            state: State::Done,
            since: Some(epoch - 6 * MINUTE),
            body: l.pick("查到了：定时出粮和「早饭」自动化都在 7:00 触发，两次出粮间隔不到一分钟，粮道还没清空就又推了一次。建议停掉其中一个，或者把自动化挪到 7:05。", "Found it: the feeding schedule and the Breakfast automation both fire at 7:00, under a minute apart, so the chute is pushed again before it clears. Turn one off, or move the automation to 7:05.").into(),
            ctx: Some(23),
            cost: Some("$0.42".into()),
            ..base("feeder", l.pick("查喂食器的出粮记录为什么总是堵", "Find out why the pet feeder keeps jamming"), "notes", "sonnet 5.5")
        });
        agents.push(Agent {
            state: State::Fail,
            since: Some(epoch - 2 * MINUTE),
            error: Some("usage limit reached · resets 15:00".into()),
            body: l
                .pick(
                    "这一轮停在第 5 步（共 8 步）。额度恢复后，在 Paseo 里让它继续。",
                    "Stopped at step 5 of 8. When the quota resets, tell it to continue in Paseo.",
                )
                .into(),
            ..base(
                "rsbuild",
                l.pick(
                    "迁移到 Rsbuild 2.x 并跑一遍构建",
                    "Migrate to Rsbuild 2.x and run a build",
                ),
                "web-console",
                "gpt-6.1 terra",
            )
        });
        agents.push(Agent {
            state: State::Seen,
            since: Some(epoch - 20 * MINUTE),
            body: l
                .pick(
                    "已经把 README 的安装步骤按新的 CLI 重写了一遍。",
                    "Rewrote the README's install steps for the new CLI.",
                )
                .into(),
            ..base(
                "readme",
                l.pick(
                    "按新的 CLI 改写安装说明",
                    "Rewrite the install guide for the new CLI",
                ),
                "docs",
                "flash",
            )
        });

        for (n, (text, at)) in self.started.iter().enumerate() {
            agents.push(Agent {
                state: State::Work,
                since: Some(*at),
                now: "read  src/pages/login.tsx".into(),
                stoppable: true,
                ..base(
                    &alloc::format!("new-{}", n + 1),
                    text,
                    "web-console",
                    "opus 5.5",
                )
            });
        }

        let left = 35u8.saturating_sub((t * 10 / LOOP_MS) as u8);
        Snapshot {
            v: crate::model::VERSION,
            rev: self.rev,
            now: epoch,
            tz: 480,
            host: "demo".into(),
            read_only: false,
            idle: false,
            settings: None,
            lang: None,
            compose: Some(Compose {
                workspace: "web-console".into(),
            }),
            agents,
            usage: vec![
                Window {
                    label: l.pick("Claude · 本窗", "Claude · session").into(),
                    used: 100 - left,
                    resets: Some(epoch + 72 * MINUTE),
                },
                Window {
                    label: l.pick("Claude · 本周", "Claude · week").into(),
                    used: 15,
                    resets: Some(epoch + 4 * 24 * 60 * MINUTE),
                },
                Window {
                    label: l.pick("Codex · 本周", "Codex · week").into(),
                    used: 64,
                    resets: Some(epoch + 52 * 60 * MINUTE),
                },
                Window {
                    label: l.pick("Gemini Argon · 周", "Gemini Argon · week").into(),
                    used: 99,
                    resets: Some(epoch + 121 * 60 * MINUTE),
                },
            ],
            fuel: Some(Fuel {
                label: l.pick("Claude · 本窗", "Claude · session").into(),
                left,
            }),
        }
    }
}

fn base(id: &str, title: &str, project: &str, model: &str) -> Agent {
    Agent {
        id: id.into(),
        title: title.into(),
        project: project.into(),
        model: model.into(),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn act(agent: &str, kind: ActKind, request_id: Option<&str>) -> Act {
        Act {
            agent: agent.into(),
            device: "farol-test".into(),
            kind,
            request_id: request_id.map(Into::into),
            choice: Some(1),
            turn: None,
        }
    }

    fn state(demo: &mut Demo, now: u64, id: &str) -> State {
        demo.emitted = None;
        demo.snapshot(now)
            .unwrap()
            .agents
            .into_iter()
            .find(|a| a.id == id)
            .unwrap()
            .state
    }

    #[test]
    fn answers_go_to_the_open_question_only() {
        let mut demo = Demo::new(0, 0, 0, Lang::Zh);
        let at = ASK_AT_MS + 1_000;
        assert_eq!(state(&mut demo, at, "farol"), State::Call);
        assert!(!demo.act(&act("farol", ActKind::Answer, Some("q-7")), at).ok);
        assert!(demo.act(&act("farol", ActKind::Answer, Some("q-0")), at).ok);
        assert_eq!(state(&mut demo, at + 10, "farol"), State::Work);
        assert_eq!(
            demo.act(&act("farol", ActKind::Answer, Some("q-0")), at + 20)
                .reason,
            "already_resolved"
        );
    }

    #[test]
    fn a_stop_ends_the_running_turn_for_this_lap() {
        let mut demo = Demo::new(0, 0, 0, Lang::Zh);
        assert!(demo.act(&act("farol", ActKind::Stop, None), 3_000).ok);
        assert_eq!(state(&mut demo, 3_010, "farol"), State::Done);
        assert_eq!(state(&mut demo, ASK_AT_MS + 1_000, "farol"), State::Done);
        assert!(!demo.act(&act("feeder", ActKind::Stop, None), 3_000).ok);
        assert_eq!(state(&mut demo, LOOP_MS + 3_000, "farol"), State::Work);
    }
}
