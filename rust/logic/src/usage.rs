//! The usage page: every quota window, grouped by provider under a header,
//! one compact row each, in two columns balanced by height. What does not fit
//! the page or the scene's bars is counted for a "+N" note.
//!
//! Each column is a few multi-line labels on one grid of [`PITCH`] px lines
//! (tools/ui/gen_scene.py), so the plan places things on lines: a header takes
//! one, a row two (name and percentage, then bar and reset), a gap one.

use alloc::string::String;
use alloc::vec::Vec;

use crate::model::Window;

/// Left edge of each column and its width, px.
pub const COLUMNS: [i32; 2] = [28, 252];
pub const COLUMN_W: u32 = 200;
/// Top of the columns, the grid's pitch, px, and its lines.
pub const TOP: i32 = 84;
pub const PITCH: i32 = 22;
pub const LINES: i32 = 17;
const HEIGHT: i32 = LINES;
/// Lines a provider header, a window row and the space between groups take,
/// and the line the "+N" note takes at the bottom of the right column.
const HEAD_H: i32 = 1;
const ROW_H: i32 = 2;
const GROUP_GAP: i32 = 1;
const MORE_H: i32 = 1;

/// A provider header: its column and line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Head {
    pub text: String,
    pub col: usize,
    pub line: i32,
}

/// A window's row: which window, its name, its column and first line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub window: usize,
    pub name: String,
    pub col: usize,
    pub line: i32,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Plan {
    pub heads: Vec<Head>,
    pub rows: Vec<Row>,
    /// Windows left out.
    pub hidden: usize,
}

/// A provider name without a trailing account: "Claude (work)" -> "Claude".
fn without_account(group: &str) -> &str {
    let group = group.trim();
    for (open, close) in [(" (", ')'), ("（", '）'), ("(", ')')] {
        if group.ends_with(close)
            && let Some(i) = group.rfind(open)
            && i > 0
        {
            return group[..i].trim_end();
        }
    }
    group
}

/// A window's provider and name: the plugin's `group`/`name`, or else its
/// label split at the first " · ".
pub fn group_and_name(w: &Window) -> (String, String) {
    let (group, name) = match w.label.split_once(" · ") {
        Some((g, n)) => (without_account(g), n.trim()),
        None => ("", w.label.trim()),
    };
    (
        w.group.clone().unwrap_or_else(|| String::from(group)),
        w.name.clone().unwrap_or_else(|| String::from(name)),
    )
}

/// Windows in provider order (first appearance), each provider's in order.
fn grouped(windows: &[Window]) -> Vec<(String, Vec<(usize, String)>)> {
    let mut groups: Vec<(String, Vec<(usize, String)>)> = Vec::new();
    for (i, w) in windows.iter().enumerate() {
        let (group, name) = group_and_name(w);
        match groups.iter_mut().find(|(g, _)| *g == group) {
            Some((_, rows)) => rows.push((i, name)),
            None => groups.push((group, alloc::vec![(i, name)])),
        }
    }
    groups
}

/// One column's items, in order: a header or a row.
enum Item<'a> {
    Head(&'a str),
    Row(usize, &'a str),
}

/// A column's items: a header before each group's first row in it (a group
/// continued from the left column repeats its header). Each item comes with
/// the space above it.
fn column<'a>(flat: &[(&'a str, usize, &'a str)]) -> Vec<(i32, Item<'a>)> {
    let mut items = Vec::new();
    let mut current: Option<&str> = None;
    for &(group, window, name) in flat {
        let first = current.is_none();
        let gap = if first { 0 } else { GROUP_GAP };
        if current == Some(group) {
            items.push((0, Item::Row(window, name)));
            continue;
        }
        current = Some(group);
        if group.is_empty() {
            items.push((gap, Item::Row(window, name)));
        } else {
            items.push((gap, Item::Head(group)));
            items.push((0, Item::Row(window, name)));
        }
    }
    items
}

fn height(items: &[(i32, Item)]) -> i32 {
    items
        .iter()
        .map(|(gap, item)| {
            gap + match item {
                Item::Head(_) => HEAD_H,
                Item::Row(..) => ROW_H,
            }
        })
        .sum()
}

/// Places every window: providers in order, split into two columns where the
/// taller column is shortest (between providers when that fits, else inside
/// one). Rows are limited to `rows` (the scene's bars).
pub fn plan(windows: &[Window], rows: usize) -> Plan {
    let groups = grouped(windows);
    let flat: Vec<(&str, usize, &str)> = groups
        .iter()
        .flat_map(|(g, rs)| rs.iter().map(move |(i, n)| (g.as_str(), *i, n.as_str())))
        .collect();
    let n = flat.len();
    let best = (0..=n)
        .min_by_key(|&s| {
            let tall = height(&column(&flat[..s])).max(height(&column(&flat[s..])));
            let between = s == 0 || s == n || flat[s - 1].0 != flat[s].0;
            (tall > HEIGHT, !between, tall, s.abs_diff(n / 2))
        })
        .unwrap_or(0);
    let mut out = place(&flat, best, rows, HEIGHT);
    if out.hidden > 0 {
        out = place(&flat, best, rows, HEIGHT - MORE_H);
    }
    out
}

fn place(flat: &[(&str, usize, &str)], split: usize, rows: usize, last: i32) -> Plan {
    let mut out = Plan::default();
    for (c, part) in [&flat[..split], &flat[split..]].into_iter().enumerate() {
        let room = if c == 1 { last } else { HEIGHT };
        let mut y = 0;
        let mut full = false;
        for (gap, item) in column(part) {
            let top = y + gap;
            match item {
                Item::Head(text) => {
                    // A header needs its first row under it.
                    if full || top + HEAD_H + ROW_H > room {
                        full = true;
                        continue;
                    }
                    out.heads.push(Head {
                        text: String::from(text),
                        col: c,
                        line: top,
                    });
                    y = top + HEAD_H;
                }
                Item::Row(window, name) => {
                    if full || out.rows.len() >= rows || top + ROW_H > room {
                        full = true;
                        out.hidden += 1;
                        continue;
                    }
                    out.rows.push(Row {
                        window,
                        name: String::from(name),
                        col: c,
                        line: top,
                    });
                    y = top + ROW_H;
                }
            }
        }
    }
    out
}

/// A bar's color for `used` percent: moonlight blue while half or more is
/// left, warming through lamplight amber to ember red as it fills.
pub fn warmth(used: u8) -> u32 {
    const STOPS: [(u32, u32); 4] = [
        (0, 0x6FA8FF),
        (50, 0x6FA8FF),
        (75, 0xFFB13B),
        (100, 0xFF5A4E),
    ];
    let used = u32::from(used.min(100));
    let k = STOPS[1..]
        .iter()
        .position(|&(at, _)| used <= at)
        .unwrap_or(STOPS.len() - 2);
    let ((a, ca), (b, cb)) = (STOPS[k], STOPS[k + 1]);
    let t = used - a;
    let span = b - a;
    let mix = |shift: u32| {
        let from = (ca >> shift) & 0xFF;
        let to = (cb >> shift) & 0xFF;
        (from * (span - t) + to * t) / span
    };
    (mix(16) << 16) | (mix(8) << 8) | mix(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(label: &str, used: u8) -> Window {
        Window {
            label: label.into(),
            group: None,
            name: None,
            used,
            resets: None,
        }
    }

    fn named(group: &str, name: &str, used: u8) -> Window {
        Window {
            label: alloc::format!("{group} · {name}"),
            group: Some(group.into()),
            name: Some(name.into()),
            used,
            resets: None,
        }
    }

    /// The user's daemon: 12 windows across 4 providers.
    fn real() -> Vec<Window> {
        alloc::vec![
            named("Claude", "Session", 55),
            named("Claude", "Weekly", 33),
            named("Claude", "Weekly · Fable", 0),
            named("Codex", "5-hour", 1),
            named("Codex", "Weekly", 69),
            named("Antigravity", "Gemini · 5h", 0),
            named("Antigravity", "Gemini · Weekly", 0),
            named("Antigravity", "Claude & GPT · 5h", 0),
            named("Antigravity", "Claude & GPT · Weekly", 8),
            named("Antigravity", "Gemini 4 Argon · Weekly", 99),
            named("GLM Coding Plan", "5-hour", 0),
            named("GLM Coding Plan", "MCP calls · Monthly", 2),
        ]
    }

    #[test]
    fn older_labels_split_at_the_first_dot_and_lose_the_account() {
        assert_eq!(
            group_and_name(&w("Claude (work@x.dev) · Weekly · Fable", 0)),
            ("Claude".into(), "Weekly · Fable".into())
        );
        assert_eq!(
            group_and_name(&w("Codex · 5-hour", 0)),
            ("Codex".into(), "5-hour".into())
        );
        assert_eq!(group_and_name(&w("Total", 0)), ("".into(), "Total".into()));
        let mut both = w("x · y", 0);
        both.group = Some("GLM Coding Plan".into());
        assert_eq!(
            group_and_name(&both),
            ("GLM Coding Plan".into(), "y".into())
        );
    }

    #[test]
    fn the_real_set_shows_every_window_in_two_balanced_columns() {
        let p = plan(&real(), 12);
        assert_eq!(p.hidden, 0);
        assert_eq!(p.rows.len(), 12);
        let heads: Vec<(&str, usize, i32)> = p
            .heads
            .iter()
            .map(|h| (h.text.as_str(), h.col, h.line))
            .collect();
        // Claude and Codex on the left (13 lines), Antigravity and GLM on the right (17).
        assert_eq!(
            heads,
            [
                ("Claude", 0, 0),
                ("Codex", 0, 8),
                ("Antigravity", 1, 0),
                ("GLM Coding Plan", 1, 12),
            ]
        );
        for r in &p.rows {
            assert!(r.line + ROW_H <= LINES, "{r:?}");
        }
        // Rows sit under their header, in the plugin's order.
        let argon = p.rows.iter().find(|r| r.window == 9).unwrap();
        assert_eq!(
            (argon.name.as_str(), argon.col, argon.line),
            ("Gemini 4 Argon · Weekly", 1, 9)
        );
        assert_eq!(p.rows.iter().find(|r| r.window == 5).unwrap().line, HEAD_H);
    }

    #[test]
    fn what_does_not_fit_is_counted() {
        let many: Vec<Window> = (0..30)
            .map(|i| named(if i < 15 { "A" } else { "B" }, "w", i as u8))
            .collect();
        let p = plan(&many, 12);
        assert_eq!(p.rows.len() + p.hidden, 30);
        assert!(p.hidden >= 18);
        // The right column leaves room for the note.
        for r in p.rows.iter().filter(|r| r.col == 1) {
            assert!(r.line + ROW_H <= LINES - MORE_H);
        }
    }

    #[test]
    fn one_tall_provider_continues_in_the_right_column_under_its_header() {
        let tall: Vec<Window> = (0..14).map(|i| named("Gemini", "w", i)).collect();
        let p = plan(&tall, 14);
        assert_eq!(p.hidden, 0);
        assert_eq!(p.heads.len(), 2);
        assert_eq!(p.heads[1].col, 1);
    }

    #[test]
    fn bars_warm_as_they_fill() {
        assert_eq!(warmth(0), 0x6FA8FF);
        assert_eq!(warmth(40), 0x6FA8FF);
        assert_eq!(warmth(75), 0xFFB13B);
        assert_eq!(warmth(100), 0xFF5A4E);
        assert_eq!(warmth(200), 0xFF5A4E);
        let red = |c: u32| c >> 16;
        assert!(red(warmth(65)) > red(warmth(55)));
    }
}
