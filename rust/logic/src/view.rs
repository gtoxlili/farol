//! Rendering: describes the whole screen from the [`App`] state on every
//! update. [`Painter`] drops writes that would not change anything, so this
//! costs a few hundred cache lookups and only the changes reach GSP.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use gsp_sys as gsp;

use crate::app::{App, DECK, Entry, Overlay, SLOTS};
use crate::lang::Lang;
use crate::model::VERSION;
use crate::model::{Agent, AskKind, State};
use crate::paseo::Down;
use crate::scene::{self, Card, Reply, Variant};
use crate::sys::NetState;
use crate::text::markdown::{self, Frame, Layout};
use crate::text::{self, time};
use crate::tr;
use crate::ui::{Painter, Ui};
use crate::usage;

/// Colors, 0xRRGGBB; the same tokens as tools/ui/gen_scene.py.
pub mod palette {
    pub const INK1: u32 = 0xF2EFE8;
    pub const INK2: u32 = 0x8C8A84;
    pub const DIM: u32 = 0x2E2D2A;
    pub const WORK: u32 = 0x6FA8FF;
    pub const CALL: u32 = 0xFFB13B;
    pub const DONE: u32 = 0xF2EFE8;
    pub const FAIL: u32 = 0xFF5A4E;
    pub const SEEN: u32 = 0x3A3936;
    pub const LAMP_IDLE: u32 = 0x5E5C57;
    pub const TILE: u32 = 0x171716;
    pub const TILE_ON: u32 = 0x3A3936;
    /// The lamp while you talk (tools/ui/gen_scene.py VOICE).
    pub const VOICE: u32 = 0xA99BFF;
}
use palette::*;

/// The scene's translated labels, in the board's language.
fn static_texts(app: &App, p: &mut Painter, ui: &mut impl Ui) {
    let l = app.lang();
    for (bind, zh, en) in scene::TEXTS {
        p.text(ui, bind, l.pick(zh, en));
    }
    p.text(ui, bind(gsp::GSP_BIND_ST_LANG_T), l.pick("English", "中文"));
}

/// Index into a [`Variant`] array (call, work, done, fail, dim).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Tone {
    Call = 0,
    Work = 1,
    Done = 2,
    Fail = 3,
    Dim = 4,
}

fn color(state: State) -> u32 {
    match state {
        State::Call => CALL,
        State::Work => WORK,
        State::Done => DONE,
        State::Fail => FAIL,
        State::Seen => SEEN,
    }
}

/// Shows `text` in the variant of `tone` and hides the others.
fn variant(
    p: &mut Painter,
    ui: &mut impl Ui,
    set: &[Variant; scene::VARIANTS],
    tone: Option<Tone>,
    text: &str,
) {
    for (i, v) in set.iter().enumerate() {
        let on = tone.is_some_and(|t| t as usize == i);
        if on {
            p.text(ui, v.text, text);
        }
        p.visible(ui, v.vis, on);
    }
}

const fn bind(b: u32) -> u16 {
    b as u16
}

impl App {
    pub(crate) fn render(&mut self, ui: &mut impl Ui) {
        let mut p = core::mem::take(&mut self.painter);
        static_texts(self, &mut p, ui);
        status_bar(self, &mut p, ui);
        home(self, &mut p, ui);
        deck(self, &mut p, ui);
        usage(self, &mut p, ui);
        control(self, &mut p, ui);
        overlays(self, &mut p, ui);
        self.painter = p;
    }
}

// ---------------------------------------------------------------- status bar

fn status_bar(app: &App, p: &mut Painter, ui: &mut impl Ui) {
    // "--:--" until Paseo has told the time.
    let clock = app
        .epoch()
        .map_or_else(|| String::from("--:--"), |e| time::clock(e, app.tz));
    p.text(ui, bind(gsp::GSP_BIND_SB_TIME), &clock);

    let power = app.power;
    // No fuel gauge (yet): no battery at all, rather than an empty one.
    p.visible(ui, bind(gsp::GSP_BIND_SB_BATT_BOX_VIS), power.known);
    let pct = if power.known {
        format!("{}%", power.percent)
    } else {
        String::new()
    };
    p.text(ui, bind(gsp::GSP_BIND_SB_BATT_PCT), &pct);
    let low = power.known && power.percent < 20 && !power.charging;
    let level = if power.known {
        i32::from(power.percent)
    } else {
        0
    };
    p.value(ui, gsp::GSP_OBJ_KEY_SB_BATT, level);
    p.value(ui, gsp::GSP_OBJ_KEY_SB_BATT_LOW, level);
    p.visible(ui, bind(gsp::GSP_BIND_SB_BATT_VIS), !low);
    p.visible(ui, bind(gsp::GSP_BIND_SB_BATT_LOW_VIS), low);
    p.visible(
        ui,
        bind(gsp::GSP_BIND_SB_BOLT_VIS),
        power.known && power.charging,
    );

    let bars = wifi_bars(app);
    for (i, b) in [
        gsp::GSP_BIND_SB_WIFI0,
        gsp::GSP_BIND_SB_WIFI1,
        gsp::GSP_BIND_SB_WIFI2,
    ]
    .into_iter()
    .enumerate()
    {
        p.color(ui, bind(b), if (i as u8) < bars { INK1 } else { DIM });
    }
    let offline = app.net.state != NetState::Online && app.net.state != NetState::Off;
    p.visible(ui, bind(gsp::GSP_BIND_SB_WIFI_OFF_VIS), offline);

    let ready = app.session.is_ready() || app.is_demo();
    let failing = !ready && app.session.is_configured() && app.session.failures() > 0;
    p.visible(ui, bind(gsp::GSP_BIND_SB_LINK_VIS), !failing);
    p.visible(ui, bind(gsp::GSP_BIND_SB_LINK_OFF_VIS), failing);
    p.visible(ui, bind(gsp::GSP_BIND_SB_LINK_DOT_VIS), ready);
    p.visible(ui, bind(gsp::GSP_BIND_SB_DND_VIS), app.dnd());
}

fn wifi_bars(app: &App) -> u8 {
    if app.net.state != NetState::Online {
        return 0;
    }
    match app.net.rssi {
        r if r >= -60 => 3,
        r if r >= -72 => 2,
        _ => 1,
    }
}

// ---------------------------------------------------------------- home

fn count(app: &App, state: State) -> usize {
    app.snapshot
        .agents
        .iter()
        .filter(|a| a.state == state)
        .count()
}

fn first(app: &App, state: State) -> Option<&Agent> {
    app.order
        .iter()
        .map(|&i| &app.snapshot.agents[i])
        .find(|a| a.state == state)
}

fn who(agent: &Agent) -> String {
    match (agent.model.is_empty(), agent.project.is_empty()) {
        (false, false) => format!("{} · {}", agent.model, agent.project),
        (false, true) => agent.model.clone(),
        (true, false) => agent.project.clone(),
        (true, true) => String::new(),
    }
}

fn home(app: &App, p: &mut Painter, ui: &mut impl Ui) {
    let l = app.lang();
    let (calls, fails, works, dones) = (
        count(app, State::Call),
        count(app, State::Fail),
        count(app, State::Work),
        count(app, State::Done),
    );
    let (lamp, tone, number, word, focus, hint): (u32, Tone, usize, &str, Option<&Agent>, &str) =
        if calls > 0 {
            (
                CALL,
                Tone::Call,
                calls,
                l.pick("需要你", "Needs you"),
                first(app, State::Call),
                l.pick("轻点光，去回答", "Tap the light to answer"),
            )
        } else if fails > 0 {
            (
                FAIL,
                Tone::Fail,
                fails,
                l.pick("失败了", "Failed"),
                first(app, State::Fail),
                l.pick("轻点光，去看看", "Tap the light to look"),
            )
        } else if works > 0 {
            (
                WORK,
                Tone::Work,
                works,
                l.pick("在工作", "Working"),
                first(app, State::Work),
                "",
            )
        } else if dones > 0 {
            (
                DONE,
                Tone::Done,
                dones,
                l.pick("完成了", "Done"),
                first(app, State::Done),
                l.pick("轻点光，去看看", "Tap the light to look"),
            )
        } else {
            (
                LAMP_IDLE,
                Tone::Dim,
                0,
                if app.has_snapshot || app.is_demo() {
                    l.pick("都完成了", "All done")
                } else {
                    l.pick("等待 Paseo", "Waiting for Paseo")
                },
                None,
                "",
            )
        };
    let resting = number == 0;
    p.visible(ui, bind(gsp::GSP_BIND_HOME_COUNT_BOX_VIS), !resting);
    p.visible(ui, bind(gsp::GSP_BIND_HOME_TIME_BOX_VIS), resting);
    p.text(ui, bind(gsp::GSP_BIND_HOME_COUNT), &number.to_string());
    // "--:--" until Paseo has told the time.
    let clock = app
        .epoch()
        .map_or_else(|| String::from("--:--"), |e| time::clock(e, app.tz));
    p.text(ui, bind(gsp::GSP_BIND_HOME_TIME), &clock);
    variant(p, ui, &scene::HOME_WORD, Some(tone), word);

    let (who_line, what_line) = match focus {
        Some(a) if a.state == State::Call => {
            let what = a.ask.as_ref().map_or(a.title.as_str(), |q| {
                if q.text.is_empty() { &a.title } else { &q.text }
            });
            (who(a), text::fit(what, 400, 22))
        }
        Some(a) if a.state == State::Fail => (
            who(a),
            text::fit(a.error.as_deref().unwrap_or(&a.title), 400, 22),
        ),
        Some(a) if a.state == State::Work => {
            let now = if a.now.is_empty() {
                a.title.clone()
            } else {
                format!("› {}", a.now)
            };
            (who(a), now)
        }
        Some(a) => (who(a), text::fit(&a.title, 400, 22)),
        None => (app.host.clone(), String::new()),
    };
    p.text(ui, bind(gsp::GSP_BIND_HOME_WHO), &who_line);
    p.text(ui, bind(gsp::GSP_BIND_HOME_WHAT), &what_line);
    // A note for no card (the last card was just read) shows under the lamp.
    let hint = app
        .note
        .as_ref()
        .filter(|n| n.agent.is_empty())
        .map_or(hint, |n| n.text.as_str());
    p.text(ui, bind(gsp::GSP_BIND_HOME_HINT), hint);

    // The lamp: color = urgency, level = the tightest quota left, sparks = running agents.
    let key = gsp::GSP_OBJ_KEY_LAMP;
    p.property_color(ui, key, gsp::GSP_PROP_KEY_FG_COLOR, app.lamp_color(lamp));
    let level = app
        .snapshot
        .fuel
        .as_ref()
        .map_or(if resting { 88 } else { 60 }, |f| i32::from(f.left));
    p.glide_value(ui, key, level, 900);
    p.visible(
        ui,
        bind(gsp::GSP_BIND_LAMP_PULSE_VIS),
        app.now < app.pulse_until,
    );
    p.property(ui, key, gsp::GSP_PROP_KEY_PARTICLES, works.min(6) as i32);
    let period = match works {
        0 => 6000,
        1 => 4200,
        2 => 3400,
        _ => 2800,
    };
    p.property(ui, key, gsp::GSP_PROP_KEY_PERIOD_MS, period);
    p.property(
        ui,
        key,
        gsp::GSP_PROP_KEY_STRENGTH,
        if resting { 40 } else { 85 },
    );

    // The fleet as dots under the line, in deck order.
    let n = app.order.len().min(scene::HOME_DOTS.len());
    let x0 = 240 - (n as i32 - 1) * 9 - 3;
    for (i, dot) in scene::HOME_DOTS.iter().enumerate() {
        if i < n {
            let agent = app.agent_at(i).expect("i < order.len()");
            p.property(ui, dot.key, gsp::GSP_PROP_KEY_X, x0 + i as i32 * 18);
            p.color(ui, dot.color, color(agent.state));
        } else {
            p.property(ui, dot.key, gsp::GSP_PROP_KEY_X, 480);
        }
    }
}

// ---------------------------------------------------------------- deck

fn deck(app: &App, p: &mut Painter, ui: &mut impl Ui) {
    p.visible(ui, bind(gsp::GSP_BIND_DECK_EMPTY_VIS), app.order.is_empty());
    for (slot, card) in scene::CARDS.iter().enumerate().take(SLOTS) {
        let agent = app.slots[slot].as_deref().and_then(|id| app.agent(id));
        render_card(app, p, ui, card, agent);
    }
    // The rail: one dot per agent down the right edge, a ring on the current one.
    let n = app.order.len().min(scene::RAIL_DOTS.len());
    let y0 = 240 - (n as i32 - 1) * 7 - 3;
    for (i, dot) in scene::RAIL_DOTS.iter().enumerate() {
        if i < n {
            let agent = app.agent_at(i).expect("i < order.len()");
            p.property(ui, dot.key, gsp::GSP_PROP_KEY_Y, y0 + i as i32 * 14);
            p.color(ui, dot.color, color(agent.state));
        } else {
            p.property(ui, dot.key, gsp::GSP_PROP_KEY_Y, 480);
        }
    }
    let cursor = if n > 1 && app.deck < n {
        y0 + app.deck as i32 * 14 - 3
    } else {
        480
    };
    p.property(
        ui,
        gsp::GSP_OBJ_KEY_RAIL_CURSOR,
        gsp::GSP_PROP_KEY_Y,
        cursor,
    );
}

fn render_card(app: &App, p: &mut Painter, ui: &mut impl Ui, card: &Card, agent: Option<&Agent>) {
    let l = app.lang();
    let Some(a) = agent else {
        variant(p, ui, &card.word, None, "");
        p.text(ui, card.meta, "");
        p.text(ui, card.title, "");
        p.visible(ui, card.ask, false);
        p.visible(ui, card.run, false);
        p.visible(ui, card.done, false);
        p.visible(ui, card.seen_ok, false);
        return;
    };
    let now = app.epoch();
    let sent = app.sent.get(&a.id).map(|(_, option)| *option);
    let note = app
        .note
        .as_ref()
        .filter(|n| n.agent == a.id)
        .map(|n| n.text.as_str());

    let (word, tone) = match a.state {
        State::Call if sent.is_some() => (
            l.pick("已回答 · 等它继续", "Answered · waiting for it"),
            Tone::Work,
        ),
        State::Call => match a.ask.as_ref().map(|q| q.kind) {
            Some(AskKind::Permission) => {
                (l.pick("需要你允许", "Needs your permission"), Tone::Call)
            }
            Some(AskKind::Plan) => (l.pick("等你批准计划", "Plan needs approval"), Tone::Call),
            _ => (l.pick("需要你", "Needs you"), Tone::Call),
        },
        State::Work => (l.pick("工作中", "Working"), Tone::Work),
        State::Done => (l.pick("完成", "Done"), Tone::Done),
        State::Fail => (l.pick("失败", "Failed"), Tone::Fail),
        State::Seen => (l.pick("已看过", "Seen"), Tone::Dim),
    };
    p.color(
        ui,
        card.dot,
        if sent.is_some() { WORK } else { color(a.state) },
    );
    variant(p, ui, &card.word, Some(tone), word);

    let mut meta = who(a);
    if let (Some(now), Some(since)) = (now, a.since) {
        let when = if a.state == State::Done || a.state == State::Seen {
            String::new()
        } else {
            time::ago(now, since, l)
        };
        if !when.is_empty() && a.state != State::Work {
            meta = format!("{meta} · {when}");
        }
    }
    if a.state == State::Work
        && let Some(ctx) = a.ctx
    {
        meta = format!("{meta} · ctx {ctx}%");
    }
    p.text(ui, card.meta, &text::fit(&meta, 420, 15));
    p.text(ui, card.title, &text::wrap(&a.title, 420, 26, 2));

    // Asking.
    let asking = a.state == State::Call && a.ask.is_some();
    p.visible(ui, card.ask, asking);
    if let Some(ask) = a.ask.as_ref().filter(|_| asking) {
        p.text(ui, card.ask_text, &text::wrap(&ask.text, 400, 19, 2));
        let pressed = app
            .press
            .as_ref()
            .filter(|pr| pr.agent == a.id)
            .map(|pr| pr.option);
        for (j, opt) in card.options.iter().enumerate() {
            let label = ask.options.get(j);
            let shown = label.is_some();
            p.visible(ui, opt.vis, shown);
            let label = label.map_or("", String::as_str);
            p.text(ui, opt.text, &text::fit(label, 300, 18));
            let is_sent = sent == Some(j);
            p.visible(ui, opt.fill, is_sent);
            p.text(ui, opt.fill_text, &text::fit(label, 270, 18));
            p.visible(ui, opt.ring, pressed == Some(j));
            p.color(
                ui,
                opt.key_color,
                if pressed == Some(j) {
                    CALL
                } else if shown {
                    INK2
                } else {
                    DIM
                },
            );
        }
        let hint = match (note, sent) {
            (Some(n), _) => n,
            (None, Some(_)) => l.pick("已发送，等 agent 接着做", "Sent · the agent carries on"),
            (None, None) if app.snapshot.read_only => {
                l.pick("只读模式：在 Paseo 里回答", "Read-only: answer in Paseo")
            }
            (None, None) => l.pick("长按选项发送", "Hold an option to send"),
        };
        p.text(ui, card.ask_hint, hint);
    }

    // Working.
    let working = a.state == State::Work;
    p.visible(ui, card.run, working);
    if working {
        let elapsed = match (now, a.since) {
            (Some(now), Some(since)) => time::stopwatch(now - since),
            _ => String::from("--:--"),
        };
        p.text(ui, card.timer, &elapsed);
        let line = if a.now.is_empty() {
            String::from("› …")
        } else {
            format!("› {}", a.now)
        };
        p.text(ui, card.now, &line);
        p.visible(ui, card.alive, true);
        for (k, b) in card.recent.iter().enumerate() {
            p.text(ui, *b, a.recent.get(k).map_or("", String::as_str));
        }
        let (bar, todo) = match &a.todo {
            Some(t) if t.total > 0 => (
                i32::from(t.done) * 100 / i32::from(t.total),
                format!("{} / {}  {}", t.done, t.total, t.current),
            ),
            _ => (0, String::new()),
        };
        p.glide_value(ui, card.bar, bar, 500);
        p.text(ui, card.todo, &text::fit(&todo, 420, 16));
        let hint = match note {
            Some(n) => n,
            None if !a.stoppable => "",
            None if app.snapshot.read_only => l.pick("只读模式", "Read-only"),
            None => l.pick("按住停止这一轮", "Hold to stop this turn"),
        };
        p.text(ui, card.run_hint, hint);
    } else {
        p.visible(ui, card.alive, false);
    }
    if !finished_state(a) {
        p.visible(ui, card.seen_ok, false);
    }

    // Finished, failed, seen.
    let finished = finished_state(a);
    p.visible(ui, card.done, finished);
    if finished {
        let err = if a.state == State::Fail {
            a.error.as_deref().unwrap_or("")
        } else {
            ""
        };
        p.text(ui, card.err, &text::fit(err, 400, 17));
        let top = if err.is_empty() {
            REPLY_TOP
        } else {
            REPLY_TOP_ERR
        };
        let layout = markdown::layout(&a.body, &reply_frame(top));
        reply(p, ui, &card.reply, &layout, top);
        let leaving = app.leaving.as_ref().is_some_and(|lv| lv.agent == a.id);
        p.visible(ui, card.seen_ok, leaving);
        let mut cost = String::new();
        if let Some(c) = &a.cost {
            cost.push_str(c);
        }
        if let Some(ctx) = a.ctx {
            if !cost.is_empty() {
                cost.push_str(" · ");
            }
            cost.push_str(&format!("ctx {ctx}%"));
        }
        if let (Some(now), Some(since), State::Done | State::Seen) = (now, a.since, a.state) {
            if !cost.is_empty() {
                cost.push_str(" · ");
            }
            cost.push_str(&time::ago(now, since, l));
        }
        p.text(ui, card.cost, note.unwrap_or(&cost));
    }
}

// ---------------------------------------------------------------- reply

/// Where a finished card's reply starts in its group, without and with the
/// error line, and where it must end (above the cost line and the button).
const REPLY_TOP: i32 = 12;
const REPLY_TOP_ERR: i32 = 52;
const REPLY_BOTTOM: i32 = 236;
const REPLY_W: u32 = 420;

fn finished_state(a: &Agent) -> bool {
    matches!(a.state, State::Done | State::Fail | State::Seen)
        || (a.state == State::Call && a.ask.is_none())
}

fn reply_frame(top: i32) -> Frame {
    Frame {
        width: REPLY_W,
        height: REPLY_BOTTOM - top,
    }
}

/// Sets a card's reply labels from its layout.
fn reply(p: &mut Painter, ui: &mut impl Ui, r: &Reply, l: &Layout, top: i32) {
    p.property(ui, r.key, gsp::GSP_PROP_KEY_Y, top);
    for (labels, texts) in r.labels.iter().zip(l.labels()) {
        for (label, text) in labels.iter().zip(&texts) {
            p.text(ui, label.text, text);
        }
    }
    match &l.code {
        Some(code) => {
            p.property(ui, r.code.key, gsp::GSP_PROP_KEY_Y, code.y);
            p.text(ui, r.code.text, &code.lines.join("\n"));
        }
        None => p.text(ui, r.code.text, ""),
    }
    for (rect, band) in [(&r.panel, l.panel), (&r.rule, l.rule)] {
        p.visible(ui, rect.vis, band.is_some());
        if let Some(b) = band {
            p.property(ui, rect.key, gsp::GSP_PROP_KEY_Y, b.y);
            p.property(ui, rect.key, gsp::GSP_PROP_KEY_H, b.h.max(1));
        }
    }
}

// ---------------------------------------------------------------- usage

/// Width of a full usage bar, px, and where it sits on a row's second line
/// (tools/ui/gen_scene.py USAGE_BAR_W).
const USAGE_BAR_W: i32 = 104;
const USAGE_BAR_DY: i32 = 10;
/// Sizes of the usage page's header, name and percentage text.
const USAGE_HEAD_PX: u32 = 14;
const USAGE_NAME_PX: u32 = 14;
/// Room for a window's name before the percentages' tab stop.
const USAGE_NAME_W: u32 = 144;

fn usage(app: &App, p: &mut Painter, ui: &mut impl Ui) {
    let l = app.lang();
    let windows = &app.snapshot.usage;
    p.visible(ui, bind(gsp::GSP_BIND_USAGE_EMPTY_VIS), windows.is_empty());
    let updated = app.clock.map_or(String::new(), |(epoch, _)| {
        tr!(l, "{} 更新", "Updated {}", time::clock(epoch, app.tz))
    });
    p.text(ui, bind(gsp::GSP_BIND_USAGE_UPDATED), &updated);
    let plan = usage::plan(windows, scene::USAGE_BARS.len());
    // Each column's labels, line by line.
    let lines = usage::LINES as usize;
    let mut cols: [[Vec<String>; 4]; 2] = Default::default();
    for col in &mut cols {
        for label in col.iter_mut() {
            label.resize(lines, String::new());
        }
    }
    for h in &plan.heads {
        cols[h.col][0][h.line as usize] = text::fit(&h.text, usage::COLUMN_W, USAGE_HEAD_PX);
    }
    for (k, bar) in scene::USAGE_BARS.iter().enumerate() {
        let Some(row) = plan.rows.get(k) else {
            p.visible(ui, bar.vis, false);
            continue;
        };
        let w = &windows[row.window];
        let used = w.used.min(100);
        let at = row.line as usize;
        cols[row.col][1][at] = text::fit_keeping_tail(&row.name, " · ", USAGE_NAME_W, USAGE_NAME_PX);
        cols[row.col][2][at] = alloc::format!("{used}%");
        if let (Some(now), Some(resets)) = (app.epoch(), w.resets) {
            cols[row.col][3][at + 1] = time::reset(now, resets, app.tz, l);
        }
        p.visible(ui, bar.vis, true);
        p.property(ui, bar.key, gsp::GSP_PROP_KEY_X, usage::COLUMNS[row.col]);
        p.property(
            ui,
            bar.key,
            gsp::GSP_PROP_KEY_Y,
            usage::TOP + (row.line + 1) * usage::PITCH + USAGE_BAR_DY,
        );
        let fill = i32::from(used) * USAGE_BAR_W / 100;
        p.property(ui, bar.fill, gsp::GSP_PROP_KEY_X, fill - USAGE_BAR_W);
        p.color(ui, bar.color, usage::warmth(used));
    }
    for (column, labels) in scene::USAGE_COLUMNS.iter().zip(&cols) {
        let binds = [column.head, column.name, column.pct, column.reset];
        for (bind, lines) in binds.into_iter().zip(labels) {
            let text = lines.join("\n");
            p.text(ui, bind, text.trim_end_matches('\n'));
        }
    }
    let more = if plan.hidden > 0 {
        tr!(l, "+{} 项", "+{} more", plan.hidden)
    } else {
        String::new()
    };
    p.text(ui, bind(gsp::GSP_BIND_USAGE_MORE), &more);
}

// ---------------------------------------------------------------- control centre

fn control(app: &App, p: &mut Painter, ui: &mut impl Ui) {
    let l = app.lang();
    let date = app
        .epoch()
        .map_or_else(String::new, |e| time::date(e, app.tz, l));
    p.text(ui, bind(gsp::GSP_BIND_CC_DATE), &date);

    let bars = wifi_bars(app);
    for (i, b) in [
        gsp::GSP_BIND_CC_WB0,
        gsp::GSP_BIND_CC_WB1,
        gsp::GSP_BIND_CC_WB2,
    ]
    .into_iter()
    .enumerate()
    {
        p.color(ui, bind(b), if (i as u8) < bars { INK1 } else { DIM });
    }
    let quality = match bars {
        3 => l.pick("信号很好", "Strong signal"),
        2 => l.pick("信号一般", "Fair signal"),
        _ => l.pick("信号弱", "Weak signal"),
    };
    let wifi = match app.net.state {
        NetState::Online => format!("{} · {quality}", app.net.ssid),
        NetState::Connecting => tr!(l, "正在连接 {}", "Joining {}", app.net.ssid),
        _ => String::from(l.pick("没有连接", "Not connected")),
    };
    p.text(
        ui,
        bind(gsp::GSP_BIND_CC_WIFI_SUB),
        &text::fit(&wifi, 180, 14),
    );

    let ready = app.session.is_ready() || app.is_demo();
    p.visible(ui, bind(gsp::GSP_BIND_CC_LINK_DOT_VIS), ready);
    let host = if app.host.is_empty() {
        String::from("Paseo")
    } else {
        app.host.clone()
    };
    p.text(ui, bind(gsp::GSP_BIND_CC_HOST), &host);
    let paseo = if app.is_demo() {
        String::from(l.pick("演示数据", "Demo data"))
    } else if ready {
        app.session.rtt_ms().map_or_else(
            || String::from(l.pick("已连接", "Connected")),
            |rtt| tr!(l, "已连接 · {rtt} ms", "Connected · {rtt} ms"),
        )
    } else if !app.session.is_configured() {
        String::from(l.pick("还没配对", "Not paired"))
    } else {
        String::from(down_text(app.down(), l))
    };
    p.text(
        ui,
        bind(gsp::GSP_BIND_CC_PASEO_SUB),
        &text::fit(&paseo, 180, 14),
    );

    let toggles = [
        (
            gsp::GSP_BIND_CC_DND_OFF_VIS,
            gsp::GSP_BIND_CC_DND_ON_VIS,
            app.dnd(),
        ),
        (
            gsp::GSP_BIND_CC_SOUND_OFF_VIS,
            gsp::GSP_BIND_CC_SOUND_ON_VIS,
            app.prefs.sound,
        ),
        (
            gsp::GSP_BIND_CC_HAPTIC_OFF_VIS,
            gsp::GSP_BIND_CC_HAPTIC_ON_VIS,
            app.prefs.haptic,
        ),
    ];
    for (off, on, value) in toggles {
        p.visible(ui, bind(off), !value);
        p.visible(ui, bind(on), value);
    }
    p.color(
        ui,
        bind(gsp::GSP_BIND_CC_AUTO_BG),
        if app.prefs.auto && app.module {
            TILE_ON
        } else {
            TILE
        },
    );
    p.value(
        ui,
        gsp::GSP_OBJ_KEY_CC_BRIGHT,
        i32::from(app.prefs.brightness),
    );

    let power = app.power;
    let batt = if !power.known {
        String::new()
    } else if power.charging {
        match power.minutes {
            Some(m) if power.percent < 100 => tr!(
                l,
                "电池 {}% · 充电中，约 {} 充满",
                "Battery {}% · charging, full in {}",
                power.percent,
                time::span(i64::from(m) * 60_000, l)
            ),
            _ => tr!(
                l,
                "电池 {}% · 充电中",
                "Battery {}% · charging",
                power.percent
            ),
        }
    } else {
        match power.minutes {
            Some(m) => tr!(
                l,
                "电池 {}% · 约剩 {}",
                "Battery {}% · about {} left",
                power.percent,
                time::span(i64::from(m) * 60_000, l)
            ),
            None => tr!(l, "电池 {}%", "Battery {}%", power.percent),
        }
    };
    p.text(ui, bind(gsp::GSP_BIND_CC_BATT), &text::fit(&batt, 330, 14));
}

fn down_text(down: &Down, l: Lang) -> &'static str {
    match down {
        Down::None => l.pick("正在连接…", "Connecting…"),
        Down::Unconfigured => l.pick("还没有连接 Paseo", "Not connected to Paseo"),
        Down::Dns => l.pick("找不到这个 Host", "Host not found"),
        Down::Timeout => l.pick("连接超时", "Connection timed out"),
        Down::Tls => l.pick("SSL 证书无效", "SSL certificate not trusted"),
        Down::Refused(403) => l.pick(
            "daemon 不认这个 Host（hostnames）",
            "The daemon doesn't accept this host (hostnames)",
        ),
        Down::Refused(_) => l.pick("对方拒绝了连接", "Connection refused"),
        Down::Auth => l.pick("daemon 密码不对", "Wrong daemon password"),
        Down::NeedPassword => l.pick(
            "这台 Paseo 需要 daemon 密码",
            "This Paseo needs its daemon password",
        ),
        Down::Handshake => l.pick(
            "加密连接失败：请重新扫 Paseo 的配对码",
            "Encrypted connection failed: scan Paseo's pairing code again",
        ),
        Down::RelayAway => l.pick(
            "relay 联系不上这台 Paseo",
            "The relay can't reach this Paseo",
        ),
        Down::Incompatible => l.pick(
            "Paseo 版本不兼容，需要更新",
            "This Paseo is too old: update it",
        ),
        Down::PluginVersion(v) if *v > VERSION => l.pick(
            "Farol 插件比固件新：请更新板子的固件",
            "The plugin is newer than the firmware: update the board",
        ),
        Down::PluginVersion(_) => l.pick(
            "Farol 插件比固件旧：请在 Paseo 里更新插件",
            "The plugin is older than the firmware: update it in Paseo",
        ),
        Down::PluginMissing => l.pick(
            "Paseo 里没有运行 Farol 插件",
            "The Farol plugin isn't running in Paseo",
        ),
        Down::Closed(_) => l.pick("连接断开了", "Disconnected"),
    }
}

// ---------------------------------------------------------------- overlays

/// Every overlay's visibility bind. A board once kept the no-Paseo page up
/// after the session recovered: GSP accepted the hide (while app_display had
/// it paused for a panel command) but the page stayed. The app re-sends these
/// every [`crate::app::OVERLAY_REASSERT_MS`] so a lost write heals itself.
pub(crate) const OVERLAY_VIS: [u16; 13] = [
    bind(gsp::GSP_BIND_OV_STATUS_VIS),
    bind(gsp::GSP_BIND_ST_WELCOME_VIS),
    bind(gsp::GSP_BIND_ST_NOWIFI_VIS),
    bind(gsp::GSP_BIND_ST_NOPASEO_VIS),
    bind(gsp::GSP_BIND_ST_CONNECT_VIS),
    bind(gsp::GSP_BIND_ST_PAIR_VIS),
    bind(gsp::GSP_BIND_ST_TLS_VIS),
    bind(gsp::GSP_BIND_ST_COMPOSE_VIS),
    bind(gsp::GSP_BIND_ST_SCAN_VIS),
    bind(gsp::GSP_BIND_ST_INPUT_VIS),
    bind(gsp::GSP_BIND_ST_WIFI_VIS),
    bind(gsp::GSP_BIND_ST_NETINFO_VIS),
    bind(gsp::GSP_BIND_ST_LINK_VIS),
];

fn overlays(app: &App, p: &mut Painter, ui: &mut impl Ui) {
    let l = app.lang();
    let o = &app.overlay;
    p.visible(ui, bind(gsp::GSP_BIND_OV_STATUS_VIS), *o != Overlay::None);
    p.visible(
        ui,
        bind(gsp::GSP_BIND_ST_WELCOME_VIS),
        *o == Overlay::Welcome,
    );
    p.visible(ui, bind(gsp::GSP_BIND_ST_NOWIFI_VIS), *o == Overlay::NoWifi);
    p.visible(
        ui,
        bind(gsp::GSP_BIND_ST_NOPASEO_VIS),
        *o == Overlay::NoPaseo,
    );
    p.visible(
        ui,
        bind(gsp::GSP_BIND_ST_CONNECT_VIS),
        *o == Overlay::Connect,
    );
    p.visible(
        ui,
        bind(gsp::GSP_BIND_ST_PAIR_VIS),
        matches!(o, Overlay::Pair { .. }),
    );
    p.visible(
        ui,
        bind(gsp::GSP_BIND_ST_TLS_VIS),
        matches!(o, Overlay::Tls { .. }),
    );
    p.visible(
        ui,
        bind(gsp::GSP_BIND_ST_COMPOSE_VIS),
        *o == Overlay::Compose,
    );
    p.visible(ui, bind(gsp::GSP_BIND_ST_SCAN_VIS), *o == Overlay::Scan);
    p.visible(
        ui,
        bind(gsp::GSP_BIND_ST_INPUT_VIS),
        matches!(o, Overlay::Input { .. }),
    );
    p.visible(ui, bind(gsp::GSP_BIND_ST_WIFI_VIS), *o == Overlay::Wifi);
    p.visible(
        ui,
        bind(gsp::GSP_BIND_ST_NETINFO_VIS),
        *o == Overlay::NetInfo,
    );
    p.visible(ui, bind(gsp::GSP_BIND_ST_LINK_VIS), *o == Overlay::Link);
    match o {
        Overlay::Input {
            entry,
            error,
            joining,
        } => input_page(p, ui, entry, error.as_deref(), *joining, l),
        Overlay::Compose => compose_page(app, p, ui),
        Overlay::Wifi => wifi_page(app, p, ui),
        Overlay::NetInfo => net_details(app, p, ui),
        Overlay::Link => link_details(app, p, ui),
        Overlay::Tls { host, port } => p.text(
            ui,
            bind(gsp::GSP_BIND_TL_CAPTION),
            &text::fit(
                &tr!(
                    l,
                    "第 3 步，共 4 步 · {}",
                    "Step 3 of 4 · {}",
                    crate::endpoint::host_port(host, *port)
                ),
                432,
                13,
            ),
        ),
        _ => {}
    }

    p.text(
        ui,
        bind(gsp::GSP_BIND_ST_WELCOME_NAME),
        &format!("farol-{}", app.device),
    );

    // No Wi-Fi.
    let reason = match app.net.state {
        NetState::NoSaved => {
            String::from(l.pick("板子上还没有保存任何网络", "No networks saved on the board"))
        }
        NetState::NotFound => {
            String::from(l.pick("附近没有已保存的网络", "No saved network in range"))
        }
        NetState::WrongPassword => tr!(l, "{} · 密码不对", "{} · wrong password", app.net.ssid),
        NetState::NoAddress => tr!(l, "{} · 拿不到地址", "{} · no IP address", app.net.ssid),
        NetState::Connecting => tr!(l, "正在连接 {}", "Joining {}", app.net.ssid),
        _ => String::new(),
    };
    p.text(
        ui,
        bind(gsp::GSP_BIND_ST_NOWIFI_REASON),
        &text::fit(&reason, 424, 17),
    );
    let saved = if app.net.saved.is_empty() {
        String::new()
    } else {
        tr!(l, "已保存：{}", "Saved: {}", app.net.saved.join("、"))
    };
    p.text(
        ui,
        bind(gsp::GSP_BIND_ST_NOWIFI_SAVED),
        &text::fit(&saved, 424, 14),
    );
    let connecting = app.net.state == NetState::Connecting;
    for (k, b) in [
        gsp::GSP_BIND_ST_NOWIFI_LIT0_VIS,
        gsp::GSP_BIND_ST_NOWIFI_LIT1_VIS,
        gsp::GSP_BIND_ST_NOWIFI_LIT2_VIS,
    ]
    .into_iter()
    .enumerate()
    {
        // Arc 2 is the innermost; the "connecting" wave runs from it outwards.
        p.visible(
            ui,
            bind(b),
            connecting && usize::from(app.wifi_phase) > 2 - k,
        );
    }
    p.color(
        ui,
        bind(gsp::GSP_BIND_ST_NOWIFI_DOT),
        if connecting { INK2 } else { FAIL },
    );

    // No Paseo.
    p.text(
        ui,
        bind(gsp::GSP_BIND_ST_NP_REASON),
        down_text(app.down(), l),
    );
    let retry = match app.session.retry_at() {
        Some(at) => tr!(
            l,
            "已重试 {} 次，{} 秒后再试",
            "Tried {} times · next in {} s",
            app.session.failures(),
            at.saturating_sub(app.now).div_ceil(1000)
        ),
        None => String::from(l.pick("正在连接…", "Connecting…")),
    };
    p.text(ui, bind(gsp::GSP_BIND_ST_NP_RETRY), &retry);
    let foot = match (app.net.state, app.has_snapshot, app.epoch()) {
        (NetState::Online, true, Some(now)) => match app.clock {
            Some((epoch, _)) => tr!(
                l,
                "Wi-Fi：{} 正常 · 屏上的数据停在 {}",
                "Wi-Fi: {} OK · data on screen from {}",
                app.net.ssid,
                time::ago(now, epoch, l)
            ),
            None => String::new(),
        },
        (NetState::Online, _, _) => tr!(l, "Wi-Fi：{} 正常", "Wi-Fi: {} OK", app.net.ssid),
        _ => String::new(),
    };
    p.text(
        ui,
        bind(gsp::GSP_BIND_ST_NP_FOOT),
        &text::fit(&foot, 424, 14),
    );

    // Pair.
    if let Overlay::Pair { config } = o {
        let relay = config.relay().is_some();
        let title = if relay {
            l.pick(
                "通过 relay 连接这台 Paseo？",
                "Connect to this Paseo through the relay?",
            )
        } else {
            l.pick("直接连接这台 Paseo？", "Connect to this Paseo directly?")
        };
        p.text(ui, bind(gsp::GSP_BIND_ST_PAIR_TITLE), title);
        p.text(
            ui,
            bind(gsp::GSP_BIND_ST_PAIR_URL),
            &text::fit(&config.describe(), 424, 16),
        );
        let code = match (config.password.is_empty(), relay) {
            (false, _) => tr!(
                l,
                "daemon 密码 {}",
                "Daemon password {}",
                "•".repeat(config.password.chars().count().min(12))
            ),
            (true, true) => String::from(l.pick(
                "端到端加密 · 需要密码时会再问你",
                "End-to-end encrypted · asks for a password if needed",
            )),
            (true, false) => String::from(l.pick("不使用密码", "No password")),
        };
        p.text(ui, bind(gsp::GSP_BIND_ST_PAIR_CODE), &code);
    }

    let scan_note = match &app.note {
        Some(n) if n.agent.is_empty() && *o == Overlay::Scan => n.text.as_str(),
        _ => "",
    };
    p.text(ui, bind(gsp::GSP_BIND_SCAN_NOTE), scan_note);

    // Hold-to-stop and the banner.
    p.visible(
        ui,
        bind(gsp::GSP_BIND_OV_HOLD_VIS),
        app.hold.as_ref().is_some_and(|h| h.shown) && app.screen == DECK,
    );
    let banner = app.banner();
    p.visible(ui, bind(gsp::GSP_BIND_BANNER_VIS), banner);
    if banner {
        p.text(
            ui,
            bind(gsp::GSP_BIND_BANNER_TEXT),
            &tr!(
                l,
                "连不上 Paseo · {} · 点开看原因",
                "Can't reach Paseo · {} · tap for details",
                down_text(app.down(), l)
            ),
        );
    }
}

// ---------------------------------------------------------------- setup pages

/// Signal strength as a word and as lit bars out of three.
fn signal(rssi: i8, l: Lang) -> (&'static str, usize) {
    match rssi {
        -60.. => (l.pick("信号很好", "Strong signal"), 3),
        -70..=-61 => (l.pick("信号一般", "Fair signal"), 2),
        _ => (l.pick("信号弱", "Weak signal"), 1),
    }
}

/// The new-session page: the violet lamp that listens, then the card the
/// session will be with its hold-to-send bar.
fn compose_page(app: &App, p: &mut Painter, ui: &mut impl Ui) {
    use crate::app::Voice;
    let l = app.lang();
    let c = &app.compose;
    let review = (c.voice == Voice::Idle && !c.text.is_empty()) || c.voice == Voice::Sending;
    p.visible(ui, bind(gsp::GSP_BIND_CP_STAGE_VIS), !review);
    p.visible(ui, bind(gsp::GSP_BIND_CP_REVIEW_VIS), review);
    let place = app.compose_where();
    let blocked = app.compose_blocked();
    let talk = app.can_talk();
    if review {
        p.text(ui, bind(gsp::GSP_BIND_CP_META), &text::fit(&place, 420, 15));
        p.text(
            ui,
            bind(gsp::GSP_BIND_CP_TITLE),
            &text::wrap(&c.text, 420, 26, 4),
        );
        let again = match &c.note {
            Some(note) => note.as_str(),
            None if talk => l.pick("按住 AI 键重说", "Hold the AI key to redo"),
            None => "",
        };
        p.text(ui, bind(gsp::GSP_BIND_CP_AGAIN), &text::fit(again, 290, 15));
        let send = match (c.voice, c.holding) {
            (Voice::Sending, _) => l.pick("正在开始…", "Starting…"),
            (_, true) => l.pick("继续按住", "Keep holding"),
            _ => l.pick("按住发送", "Hold to send"),
        };
        p.text(ui, bind(gsp::GSP_BIND_CP_SEND_T), send);
        return;
    }

    p.text(
        ui,
        bind(gsp::GSP_BIND_CP_WHERE),
        &text::fit(&place, 340, 14),
    );
    // The lamp: violet while it can listen, grey when it can't; it rises with
    // the voice and breathes slowly while Paseo transcribes.
    let key = gsp::GSP_OBJ_KEY_CP_ORB;
    let level = i32::from(app.voice_level());
    let (value, strength, particles, period, glide) = match c.voice {
        Voice::Listening => (24 + level * 3 / 4, 95, 1 + level / 25, 1300, 110),
        Voice::Transcribing => (30, 55, 0, 5200, 600),
        _ if blocked.is_some() || !talk => (10, 25, 0, 6000, 600),
        _ => (22, 45, 0, 4800, 600),
    };
    let color = if blocked.is_some() { SEEN } else { VOICE };
    p.property_color(ui, key, gsp::GSP_PROP_KEY_FG_COLOR, color);
    p.glide_value(ui, key, value, glide);
    p.property(ui, key, gsp::GSP_PROP_KEY_STRENGTH, strength);
    p.property(ui, key, gsp::GSP_PROP_KEY_PARTICLES, particles);
    p.property(ui, key, gsp::GSP_PROP_KEY_PERIOD_MS, period);

    let ready = c.voice == Voice::Idle;
    let big = if c.voice == Voice::Listening {
        l.pick("在听", "Listening")
    } else {
        ""
    };
    let word = match c.voice {
        Voice::Transcribing => l.pick("转写中", "Transcribing"),
        Voice::Listening | Voice::Sending => "",
        Voice::Idle if blocked.is_some() => l.pick("还不能开始", "Can't start yet"),
        Voice::Idle if !talk => l.pick("Paseo 没开语音输入", "Dictation is off"),
        Voice::Idle => l.pick("按住光，说要做的事", "Hold the light and talk"),
    };
    p.text(ui, bind(gsp::GSP_BIND_CP_BIG), big);
    p.text(ui, bind(gsp::GSP_BIND_CP_WORD), word);
    p.visible(ui, bind(gsp::GSP_BIND_CP_MIC_ICON_VIS), ready && talk);
    let live = match (c.voice, blocked) {
        (Voice::Listening, _) => text::wrap(&c.text, 424, 21, 2),
        (Voice::Idle, Some((zh, en))) => text::wrap(l.pick(zh, en), 424, 21, 2),
        _ => String::new(),
    };
    p.text(ui, bind(gsp::GSP_BIND_CP_LIVE), &live);
    p.visible(
        ui,
        bind(gsp::GSP_BIND_CP_SHIMMER_VIS),
        c.voice == Voice::Transcribing,
    );
    let hint = match (&c.note, c.voice) {
        (Some(note), _) => note.as_str(),
        (None, Voice::Listening) => l.pick("松开结束", "Release to finish"),
        (None, Voice::Idle) if blocked.is_none() && talk => {
            l.pick("也可以按住 AI 键", "Or hold the AI key")
        }
        (None, Voice::Idle) if blocked.is_none() => l.pick(
            "用键盘输入，只有英文字母",
            "Type it instead, Latin letters only",
        ),
        _ => "",
    };
    p.text(ui, bind(gsp::GSP_BIND_CP_HINT), &text::fit(hint, 300, 14));
    p.visible(
        ui,
        bind(gsp::GSP_BIND_CP_KEYS_VIS),
        ready && blocked.is_none(),
    );
}

fn input_page(
    p: &mut Painter,
    ui: &mut impl Ui,
    entry: &Entry,
    error: Option<&str>,
    joining: bool,
    l: Lang,
) {
    let direct = || String::from(l.pick("直接连接", "Direct connection"));
    let (title, caption) = match entry {
        Entry::Host => (
            direct(),
            String::from(l.pick(
                "第 1 步，共 4 步 · Host，例如 192.168.1.20 或 paseo.example.net",
                "Step 1 of 4 · host, e.g. 192.168.1.20 or paseo.example.net",
            )),
        ),
        Entry::Port { host, default } => (
            direct(),
            tr!(
                l,
                "第 2 步，共 4 步 · {host} 的端口，直接点 OK 用 {default}",
                "Step 2 of 4 · port of {host}; OK alone uses {default}"
            ),
        ),
        Entry::Password { host, port, .. } => (
            direct(),
            tr!(
                l,
                "第 4 步，共 4 步 · {} 的 daemon 密码，没有就直接点 OK",
                "Step 4 of 4 · daemon password for {}; none? just OK",
                crate::endpoint::host_port(host, *port)
            ),
        ),
        Entry::DaemonPassword { config } => (
            String::from(l.pick("daemon 密码", "Daemon password")),
            tr!(
                l,
                "{} 需要 daemon 密码",
                "{} needs its daemon password",
                config.fallback_name()
            ),
        ),
        Entry::Prompt => (
            String::from(l.pick("新会话", "New session")),
            String::from(l.pick(
                "输入要做的事（键盘只有英文字母），点 OK 返回",
                "Type what to do, then OK",
            )),
        ),
        Entry::WifiPassword { ssid } => (
            tr!(l, "加入「{ssid}」", "Join “{ssid}”"),
            String::from(if joining {
                l.pick("正在连接…", "Connecting…")
            } else {
                l.pick("Wi-Fi 密码", "Wi-Fi password")
            }),
        ),
    };
    p.text(
        ui,
        bind(gsp::GSP_BIND_IN_TITLE),
        &text::fit(&title, 330, 22),
    );
    p.text(
        ui,
        bind(gsp::GSP_BIND_IN_CAPTION),
        &text::fit(&caption, 432, 13),
    );
    p.text(ui, bind(gsp::GSP_BIND_IN_ERROR), error.unwrap_or(""));
}

fn wifi_page(app: &App, p: &mut Painter, ui: &mut impl Ui) {
    let l = app.lang();
    let note = match (app.networks.is_empty(), app.scanning) {
        (true, true) => l.pick("正在扫描…", "Scanning…"),
        (true, false) => l.pick("附近没有 2.4 GHz 网络", "No 2.4 GHz networks nearby"),
        _ => "",
    };
    p.text(ui, bind(gsp::GSP_BIND_WF_NOTE), note);
    for (i, row) in scene::WIFI_ROWS.iter().enumerate() {
        let ap = app.networks.get(i);
        p.visible(ui, row.vis, ap.is_some());
        let Some(ap) = ap else { continue };
        let (word, lit) = signal(ap.rssi, l);
        let mut meta = String::new();
        if ap.saved {
            meta.push_str(l.pick("已保存 · ", "Saved · "));
        }
        meta.push_str(if ap.secure {
            l.pick("需要密码 · ", "Password · ")
        } else {
            l.pick("开放网络 · ", "Open · ")
        });
        meta.push_str(word);
        p.text(ui, row.name, &text::fit(&ap.ssid, 360, 18));
        p.text(ui, row.meta, &meta);
        for (k, bar) in row.bars.iter().enumerate() {
            p.color(ui, *bar, if k < lit { INK1 } else { DIM });
        }
    }
}

fn net_details(app: &App, p: &mut Painter, ui: &mut impl Ui) {
    let l = app.lang();
    let net = &app.net;
    let online = net.state == NetState::Online;
    let ssid = if online {
        net.ssid.as_str()
    } else {
        l.pick("没有连上 Wi-Fi", "No Wi-Fi")
    };
    p.text(ui, bind(gsp::GSP_BIND_NI_SSID), &text::fit(ssid, 432, 28));
    let signal_line = if online {
        format!("{} · {} dBm", signal(net.rssi, l).0, net.rssi)
    } else {
        String::new()
    };
    p.text(ui, bind(gsp::GSP_BIND_NI_SIGNAL), &signal_line);
    let ip = if online && !net.ip.is_empty() {
        format!("IP {}", net.ip)
    } else {
        String::new()
    };
    p.text(ui, bind(gsp::GSP_BIND_NI_IP), &ip);
    for (i, b) in scene::SAVED_ROWS.iter().enumerate() {
        let name = match (i, net.saved.get(i)) {
            (0, None) => l.pick("还没有保存任何网络", "No saved networks"),
            (_, Some(name)) => name.as_str(),
            _ => "",
        };
        p.text(ui, *b, &text::fit(name, 432, 17));
    }
}

fn link_details(app: &App, p: &mut Painter, ui: &mut impl Ui) {
    let l = app.lang();
    let config = app.session.config();
    let host = match config {
        _ if !app.host.is_empty() && app.host != "demo" => app.host.clone(),
        Some(c) => c.fallback_name(),
        None => String::from(l.pick("还没有连接 Paseo", "Not connected to Paseo")),
    };
    p.text(ui, bind(gsp::GSP_BIND_LK_HOST), &text::fit(&host, 432, 28));
    let route = config.map_or(String::new(), |c| {
        let way = if c.relay().is_some() {
            "relay"
        } else {
            l.pick("直接连接", "Direct connection")
        };
        format!("{way} · {}", c.describe())
    });
    p.text(ui, bind(gsp::GSP_BIND_LK_URL), &text::fit(&route, 432, 15));
    let state = if app.session.is_ready() {
        let mut line = String::from(l.pick("已连接", "Connected"));
        if let Some(rtt) = app.session.rtt_ms() {
            line.push_str(&tr!(l, " · 延迟 {rtt} ms", " · {rtt} ms"));
        }
        if let Some(at) = app.last_sync {
            line.push_str(&tr!(
                l,
                " · {}同步",
                " · synced {}",
                time::ago(app.now as i64, at as i64, l)
            ));
        }
        line
    } else if config.is_none() {
        String::new()
    } else {
        String::from(down_text(app.down(), l))
    };
    p.text(ui, bind(gsp::GSP_BIND_LK_STATE), &state);
    p.text(
        ui,
        bind(gsp::GSP_BIND_LK_DEVICE),
        &tr!(l, "这块板子：farol-{}", "This board: farol-{}", app.device),
    );
    let forget = if app.now < app.forget_until {
        l.pick("再点一次，忘记它", "Tap again to forget")
    } else {
        l.pick("忘记这台 Paseo", "Forget this Paseo")
    };
    p.text(ui, bind(gsp::GSP_BIND_LK_FORGET_T), forget);
}
