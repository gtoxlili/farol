//! A finished card's reply, set as a small typographic preview.
//!
//! The plugin sends "Farol markdown": `#` headings, `- ` bullets (one nested
//! level), `1. ` items, `> ` quotes, fenced code, blank lines between blocks
//! and `**strong**` inline. Older plugins send raw markdown, so [`layout`] is
//! tolerant: `*`/`+` bullets and `__strong__` too; single `*x*`, `_x_` and
//! backticks are stripped, links keep their text.
//!
//! GSP labels have one text bind, a fixed color and no runtime width, and
//! every label and dynamic property costs room in the 1 MB UI partition. So a
//! card draws the reply with two sets of three multi-line labels, one per ink
//! (bright, normal, quiet): set A on a grid of whole lines from the top, set B
//! half a line lower. Blocks alternate between the sets where a half-line gap
//! separates them. Within a line, each ink's label holds only its own runs,
//! moved to their measured x with spaces and en spaces (GSP keeps leading
//! spaces and advances glyphs by their fractional widths, as `metrics.rs`
//! measures them). Code is one Geist Mono label on a panel; a quote gets a
//! rule. [`layout`] ends the last line with `…` when the rest does not fit.

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use super::{advance, break_lines, measure};

/// Body text size, px (Noto Sans SC), and GSP's line pitch for it.
pub const SIZE: u32 = 19;
pub const LINE: i32 = 29;
/// Code: Geist Mono size, GSP's pitch for it, the panel's padding and the
/// text's inset.
pub const CODE_SIZE: u32 = 15;
pub const CODE_LINE: i32 = 21;
pub const CODE_PAD: i32 = 7;
pub const CODE_X: i32 = 12;
/// Geist Mono's advance: 600 units of 1000.
const MONO_ADVANCE_PERMILLE: u32 = 600;
/// List gutter per level; a quote's text inset (its rule sits left of it).
const GUTTER: i32 = 24;
const QUOTE_X: i32 = 16;
const BULLETS: [&str; 2] = ["•", "–"];
/// Non-ASCII characters the Geist Mono subset carries (tools/fonts/build_fonts.py LATIN).
const MONO_EXTRA: &str = "·•…–—‘’“”′″°×÷±−→←↑↓›‹«»€£¥©®™§¶†‡№";
/// Padding characters: space and en space.
const SPACE: char = ' ';
const EN_SPACE: char = '\u{2002}';
/// Label sets: A on whole lines, B half a line lower.
pub const SETS: usize = 2;

/// The y of half-line `p`: even halves are set A's lines, odd ones set B's.
pub const fn half_y(p: i32) -> i32 {
    p * LINE / 2
}

/// The label a run is drawn by, within its set.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Ink {
    /// Strong text and headings (INK1).
    Strong = 0,
    /// Body text (INK2).
    Normal = 1,
    /// Quotes, bullets and numbers.
    Dim = 2,
}
pub const INKS: usize = 3;

/// A run of one ink on one line, at (x, y) from the reply's top-left.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Run {
    pub ink: Ink,
    pub x: i32,
    pub y: i32,
    pub text: String,
    /// Exact x, 1/64 px.
    x64: u32,
    /// Half-line index (`half_y`): its set is `half % 2`, its line `half / 2`.
    half: i32,
}

/// A vertical extent: the code panel or a quote's rule.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Band {
    pub y: i32,
    pub h: i32,
}

/// The code label: its y and its lines.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Code {
    pub y: i32,
    pub lines: Vec<String>,
}

/// The room for the reply.
#[derive(Clone, Copy, Debug)]
pub struct Frame {
    pub width: u32,
    pub height: i32,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Layout {
    pub runs: Vec<Run>,
    pub code: Option<Code>,
    pub panel: Option<Band>,
    pub rule: Option<Band>,
    /// Bottom of the last thing placed.
    pub height: i32,
    /// Something did not fit; the last line ends with `…`.
    pub clipped: bool,
}

impl Layout {
    /// The text of each label: `[set][ink]`, lines joined with `\n`, each
    /// run moved to its x with spaces.
    pub fn labels(&self) -> [[String; INKS]; SETS] {
        let mut out: [[String; INKS]; SETS] = Default::default();
        for (set, inks) in out.iter_mut().enumerate() {
            for (ink, text) in inks.iter_mut().enumerate() {
                let mut runs: Vec<&Run> = self
                    .runs
                    .iter()
                    .filter(|r| r.half.rem_euclid(2) as usize == set && r.ink as usize == ink)
                    .collect();
                if runs.is_empty() {
                    continue;
                }
                runs.sort_by_key(|r| (r.half, r.x64));
                let mut line = 0;
                let mut pen = 0u32;
                for r in runs {
                    while line < r.half / 2 {
                        text.push('\n');
                        line += 1;
                        pen = 0;
                    }
                    pad(text, r.x64.saturating_sub(pen));
                    text.push_str(&r.text);
                    pen = r.x64 + measure(&r.text, SIZE);
                }
            }
        }
        out
    }
}

/// Appends blanks about `gap` (1/64 px) wide: never more than a quarter
/// pixel short, as little over as spaces and en spaces allow.
fn pad(out: &mut String, gap: u32) {
    let (space, en) = (advance(SPACE, SIZE), advance(EN_SPACE, SIZE));
    let want = gap.saturating_sub(16);
    if want == 0 {
        return;
    }
    let mut best = (u32::MAX, 0, 0);
    for ens in 0..=want / en + 1 {
        let rest = want.saturating_sub(ens * en);
        let spaces = rest.div_ceil(space);
        let over = ens * en + spaces * space - want;
        if over < best.0 {
            best = (over, ens, spaces);
        }
    }
    out.extend(core::iter::repeat_n(EN_SPACE, best.1 as usize));
    out.extend(core::iter::repeat_n(SPACE, best.2 as usize));
}

// ---------------------------------------------------------------- blocks

#[derive(Clone, Debug, PartialEq, Eq)]
enum Kind {
    Para,
    Heading,
    /// A list item: nesting depth (0 or 1) and its number, if numbered.
    Item(u8, Option<String>),
    Quote,
    Code,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Block {
    kind: Kind,
    /// Inline markdown; `\n` is a line break. Code: the lines, `\n`-joined.
    text: String,
}

fn heading(t: &str) -> Option<&str> {
    let hashes = t.chars().take_while(|&c| c == '#').count();
    if hashes == 0 || hashes > 6 {
        return None;
    }
    let rest = &t[hashes..];
    if !(rest.is_empty() || rest.starts_with(' ')) {
        return None;
    }
    Some(rest.trim().trim_end_matches('#').trim_end())
}

/// A list marker: `- `, `* `, `+ `, `1. ` or `1) `; the number and the rest.
fn item(t: &str) -> Option<(Option<String>, &str)> {
    for bullet in ["- ", "* ", "+ "] {
        if let Some(rest) = t.strip_prefix(bullet) {
            return Some((None, rest.trim_start()));
        }
    }
    let digits = t.chars().take_while(char::is_ascii_digit).count();
    if (1..=3).contains(&digits) {
        let rest = &t[digits..];
        if let Some(rest) = rest.strip_prefix(". ").or_else(|| rest.strip_prefix(") ")) {
            return Some((Some(String::from(&t[..digits])), rest.trim_start()));
        }
    }
    None
}

/// `---`, `***`, `___`: a thematic break, which the preview leaves out.
fn rule(t: &str) -> bool {
    let compact: String = t.chars().filter(|c| !c.is_whitespace()).collect();
    compact.len() >= 3
        && ['-', '*', '_']
            .iter()
            .any(|&m| compact.chars().all(|c| c == m))
}

fn indent(line: &str) -> usize {
    line.chars()
        .take_while(|c| c.is_whitespace())
        .map(|c| if c == '\t' { 4 } else { 1 })
        .sum()
}

fn parse(md: &str) -> Vec<Block> {
    let mut blocks: Vec<Block> = Vec::new();
    // The open block plain lines continue (a paragraph, item or quote).
    let mut open = false;
    let mut fence: Option<Vec<String>> = None;
    for raw in md.lines() {
        let line = raw.trim_end();
        if let Some(code) = &mut fence {
            if line.trim_start().starts_with("```") {
                blocks.push(Block {
                    kind: Kind::Code,
                    text: code.join("\n"),
                });
                fence = None;
            } else {
                code.push(line.replace('\t', "    "));
            }
            continue;
        }
        let t = line.trim_start();
        if t.starts_with("```") {
            fence = Some(Vec::new());
            open = false;
            continue;
        }
        if t.is_empty() || rule(t) {
            open = false;
            continue;
        }
        if let Some(text) = heading(t) {
            blocks.push(Block {
                kind: Kind::Heading,
                text: String::from(text),
            });
            open = false;
            continue;
        }
        if let Some((number, text)) = item(t) {
            let depth = u8::from(indent(line) >= 2);
            blocks.push(Block {
                kind: Kind::Item(depth, number),
                text: String::from(text),
            });
            open = true;
            continue;
        }
        let quoted = t.strip_prefix('>').map(str::trim_start);
        let last = blocks.last_mut().filter(|_| open);
        match (quoted, last) {
            (Some(text), Some(b)) if b.kind == Kind::Quote => {
                b.text.push('\n');
                b.text.push_str(text);
            }
            (Some(text), _) => {
                blocks.push(Block {
                    kind: Kind::Quote,
                    text: String::from(text),
                });
                open = true;
            }
            // Plain lines continue a paragraph or a quote; an item only when indented.
            (None, Some(b))
                if b.kind == Kind::Para
                    || b.kind == Kind::Quote
                    || (matches!(b.kind, Kind::Item(..)) && indent(line) >= 2) =>
            {
                b.text.push('\n');
                b.text.push_str(t);
            }
            (None, _) => {
                blocks.push(Block {
                    kind: Kind::Para,
                    text: String::from(t),
                });
                open = true;
            }
        }
    }
    if let Some(code) = fence {
        blocks.push(Block {
            kind: Kind::Code,
            text: code.join("\n"),
        });
    }
    blocks
}

// ---------------------------------------------------------------- inline

/// `[text](url)` and `![alt](url)` keep their text; `<https://…>` its address.
fn links(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find(['[', '<']) {
        out.push_str(&rest[..i]);
        let tail = &rest[i..];
        if tail.starts_with('<')
            && let Some(end) = tail.find('>')
            && tail[1..end].contains("://")
            && !tail[1..end].contains(' ')
        {
            out.push_str(&tail[1..end]);
            rest = &tail[end + 1..];
            continue;
        }
        if tail.starts_with('[')
            && let Some(close) = tail.find("](")
            && let Some(end) = tail[close..].find(')')
            && !tail[1..close].contains('[')
        {
            if out.ends_with('!') {
                out.pop();
            }
            out.push_str(&tail[1..close]);
            rest = &tail[close + end + 1..];
            continue;
        }
        out.push_str(&tail[..1]);
        rest = &tail[1..];
    }
    out.push_str(rest);
    out
}

fn push_char(out: &mut Vec<(String, bool)>, ch: char, strong: bool) {
    match out.last_mut() {
        Some((text, s)) if *s == strong => text.push(ch),
        _ => out.push((String::from(ch), strong)),
    }
}

/// A later `mm` pair (`**` or `__`) from `from` on.
fn has_pair(c: &[char], from: usize, m: char) -> bool {
    (from..c.len().saturating_sub(1)).any(|k| c[k] == m && c[k + 1] == m)
}

/// The closer of a single `*x*` or `_x_` opened before `from`.
fn single_closer(c: &[char], from: usize, m: char) -> Option<usize> {
    (from + 1..c.len()).find(|&k| {
        c[k] == m
            && !c[k - 1].is_whitespace()
            && c[k - 1] != m
            && c.get(k + 1) != Some(&m)
            && (m == '*' || !c.get(k + 1).is_some_and(|n| n.is_alphanumeric()))
    })
}

/// Inline markdown to `(text, strong)` spans.
fn inline(text: &str) -> Vec<(String, bool)> {
    let c: Vec<char> = links(text).chars().collect();
    let mut drop = vec![false; c.len()];
    let mut out: Vec<(String, bool)> = Vec::new();
    let mut strong: Option<char> = None;
    let mut i = 0;
    while i < c.len() {
        let ch = c[i];
        if drop[i] {
            i += 1;
            continue;
        }
        if ch == '\\' && c.get(i + 1).is_some_and(char::is_ascii_punctuation) {
            push_char(&mut out, c[i + 1], strong.is_some());
            i += 2;
            continue;
        }
        if ch == '`' {
            let run = c[i..].iter().take_while(|&&b| b == '`').count();
            let close = (i + run..c.len())
                .find(|&k| c[k..].iter().take_while(|&&b| b == '`').count() == run);
            let end = close.unwrap_or(i + run);
            for &code in &c[i + run..end] {
                push_char(&mut out, code, strong.is_some());
            }
            i = if close.is_some() { end + run } else { end };
            continue;
        }
        if (ch == '*' || ch == '_') && c.get(i + 1) == Some(&ch) {
            if strong == Some(ch) {
                strong = None;
                i += 2;
                continue;
            }
            let boundary = ch == '*' || i == 0 || !c[i - 1].is_alphanumeric();
            if strong.is_none() && boundary && has_pair(&c, i + 2, ch) {
                strong = Some(ch);
                i += 2;
                continue;
            }
            if ch == '*' {
                // A stray `**` (a reply cut short) is dropped.
                i += 2;
                continue;
            }
        }
        if ch == '*' || ch == '_' {
            let opens = c.get(i + 1).is_some_and(|n| !n.is_whitespace())
                && (ch == '*' || i == 0 || !c[i - 1].is_alphanumeric());
            if opens && let Some(k) = single_closer(&c, i, ch) {
                drop[k] = true;
                i += 1;
                continue;
            }
        }
        push_char(&mut out, ch, strong.is_some());
        i += 1;
    }
    out
}

// ---------------------------------------------------------------- layout

/// Where the next block goes, in half-lines after the last one.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Gap {
    /// Right under it (the next item of a list).
    None,
    /// Half a line: between blocks.
    Half,
    /// A whole line: above a heading.
    Line,
}

/// What the ellipsis attaches to: the runs of the last line placed (from
/// `first`, after any list marker), or the last code line.
#[derive(Clone, Copy)]
enum Last {
    Runs { first: usize },
    Code,
}

struct Placer<'a> {
    frame: &'a Frame,
    out: Layout,
    /// The next free half-line.
    end: i32,
    last: Option<Last>,
}

impl Placer<'_> {
    fn fits(&self, y: i32, h: i32) -> bool {
        y + h <= self.frame.height
    }

    /// Places one line of runs at half-line `half`, starting at `x` (1/64 px),
    /// with an optional marker in the gutter; false when it does not fit.
    fn line(
        &mut self,
        x: u32,
        half: i32,
        runs: &[(String, Ink)],
        marker: Option<(u32, &str)>,
    ) -> bool {
        let y = half_y(half);
        if !self.fits(y, LINE) {
            return false;
        }
        let place = |out: &mut Layout, ink: Ink, x64: u32, text: &str| {
            // A run starts at its first visible character.
            let body = text.trim_start();
            let x64 = x64 + measure(&text[..text.len() - body.len()], SIZE);
            if !body.is_empty() {
                out.runs.push(Run {
                    ink,
                    x: (x64 / 64) as i32,
                    y,
                    text: String::from(body),
                    x64,
                    half,
                });
            }
        };
        if let Some((mx, mark)) = marker {
            place(&mut self.out, Ink::Dim, mx, mark);
        }
        let first = self.out.runs.len();
        let mut at = x;
        for (text, ink) in runs {
            place(&mut self.out, *ink, at, text);
            at += measure(text, SIZE);
        }
        self.last = Some(Last::Runs { first });
        self.out.height = y + LINE;
        true
    }

    /// Ends the last line placed with `…`, shortening it to make room.
    fn ellipsis(&mut self) {
        self.out.clipped = true;
        match self.last {
            Some(Last::Runs { first }) => {
                let limit = self.frame.width * 64;
                let runs = &mut self.out.runs;
                let dots = advance('…', SIZE);
                let end = |runs: &[Run]| runs.last().map_or(0, |r| r.x64 + measure(&r.text, SIZE));
                while runs.len() > first && end(&runs[first..]) + dots > limit {
                    let last = runs.last_mut().expect("len > first");
                    last.text.pop();
                    if last.text.trim_end().is_empty() {
                        runs.pop();
                    }
                }
                if let Some(last) = runs.get_mut(first..).and_then(<[Run]>::last_mut) {
                    let kept = last.text.trim_end().len();
                    last.text.truncate(kept);
                    last.text.push('…');
                }
            }
            Some(Last::Code) => {
                let max = code_chars(self.frame.width);
                if let Some(line) = self.out.code.as_mut().and_then(|c| c.lines.last_mut()) {
                    while line.chars().count() + 1 > max {
                        line.pop();
                    }
                    let kept = line.trim_end().len();
                    line.truncate(kept);
                    line.push('…');
                }
            }
            None => {}
        }
    }

    /// A paragraph, heading, item or quote from half-line `top`; false when
    /// it was cut short.
    fn text(&mut self, kind: &Kind, text: &str, top: i32) -> bool {
        let px = |v: i32| v.max(0) as u32 * 64;
        let (x, marker) = match kind {
            Kind::Item(depth, number) => {
                let base = i32::from(*depth) * GUTTER;
                let mark = number
                    .as_ref()
                    .map_or(String::from(BULLETS[usize::from(*depth).min(1)]), |n| {
                        alloc::format!("{n}.")
                    });
                let w = measure(&mark, SIZE).div_ceil(64) as i32;
                let gutter = if number.is_some() {
                    GUTTER.max(w + 8)
                } else {
                    GUTTER
                };
                // Numbers right-align in the gutter; bullets sit near its start.
                let mx = if number.is_some() {
                    base + gutter - 6 - w
                } else {
                    base + 3
                };
                (base + gutter, Some((px(mx), mark)))
            }
            Kind::Quote => (QUOTE_X, None),
            _ => (0, None),
        };
        let ink = |strong: bool| match (kind, strong) {
            (Kind::Heading, _) | (Kind::Para | Kind::Item(..), true) => Ink::Strong,
            (Kind::Quote, true) | (Kind::Para | Kind::Item(..), false) => Ink::Normal,
            _ => Ink::Dim,
        };
        let spans = inline(text);
        let pieces: Vec<(&str, Ink)> = spans.iter().map(|(t, s)| (t.as_str(), ink(*s))).collect();
        let limit = (self.frame.width as i32 - x).max(1) as u32 * 64;
        let lines = break_lines(&pieces, SIZE, limit);
        let mut placed = 0;
        for (k, line) in lines.iter().enumerate() {
            let half = top + 2 * k as i32;
            let mark = marker
                .as_ref()
                .filter(|_| k == 0)
                .map(|(mx, m)| (*mx, m.as_str()));
            if !self.line(px(x), half, line, mark) {
                break;
            }
            placed += 1;
            self.end = half + 2;
        }
        if *kind == Kind::Quote && placed > 0 && self.out.rule.is_none() {
            let y = half_y(top);
            self.out.rule = Some(Band {
                y: y + 4,
                h: placed * LINE - 8,
            });
        }
        placed == lines.len() as i32
    }

    /// A fenced block on the panel from half-line `top`; false when it was
    /// cut short. A card has one panel: a second code block ends the reply.
    fn code(&mut self, text: &str, top: i32) -> bool {
        if self.out.code.is_some() {
            return false;
        }
        let y = half_y(top);
        let width = (self.frame.width as i32 - 2 * CODE_X).max(0) as u32;
        let mut lines = Vec::new();
        let mut complete = true;
        for src in text.split('\n') {
            let line_y = y + CODE_PAD + lines.len() as i32 * CODE_LINE;
            if !self.fits(line_y, CODE_LINE + CODE_PAD) {
                complete = false;
                break;
            }
            lines.push(code_line(src, width));
        }
        if lines.is_empty() {
            return false;
        }
        let h = lines.len() as i32 * CODE_LINE + 2 * CODE_PAD;
        self.out.code = Some(Code {
            y: y + CODE_PAD,
            lines,
        });
        self.out.panel = Some(Band { y, h });
        self.out.height = y + h;
        self.last = Some(Last::Code);
        // The next block starts on the half-line grid below the panel.
        self.end = ((y + h) * 2 + LINE - 1) / LINE;
        complete
    }
}

fn mono(ch: char) -> bool {
    (' '..='~').contains(&ch) || MONO_EXTRA.contains(ch)
}

/// Monospaced characters that fit on a code line.
fn code_chars(width: u32) -> usize {
    let advance = CODE_SIZE * MONO_ADVANCE_PERMILLE / 1000;
    ((width as i32 - 2 * CODE_X).max(0) as u32 / advance.max(1)) as usize
}

/// A code line cut to `width` px: Geist Mono advances, and the body font's
/// for what Geist Mono lacks (GSP draws those from the fallback font).
fn code_line(src: &str, width: u32) -> String {
    let limit = width * 64;
    let w = |ch: char| {
        if mono(ch) {
            CODE_SIZE * MONO_ADVANCE_PERMILLE * 64 / 1000
        } else {
            advance(ch, CODE_SIZE)
        }
    };
    let total: u32 = src.chars().map(w).sum();
    if total <= limit {
        return String::from(src);
    }
    let mut out = String::new();
    let mut used = w('…');
    for ch in src.chars() {
        if used + w(ch) > limit {
            break;
        }
        used += w(ch);
        out.push(ch);
    }
    out.push('…');
    out
}

/// Lays `md` out in `frame`: runs on the label sets' lines, the code label,
/// its panel and a quote rule, from the top-left of the reply's area.
pub fn layout(md: &str, frame: &Frame) -> Layout {
    let mut p = Placer {
        frame,
        out: Layout::default(),
        end: 0,
        last: None,
    };
    let mut prev: Option<&Kind> = None;
    let blocks = parse(md);
    for block in &blocks {
        let gap = match (prev, &block.kind) {
            (None, _) => Gap::None,
            (Some(Kind::Item(..)), Kind::Item(..)) => Gap::None,
            (_, Kind::Heading) => Gap::Line,
            _ => Gap::Half,
        };
        let top = p.end
            + match gap {
                Gap::None => 0,
                Gap::Half => 1,
                Gap::Line => 2,
            };
        let complete = match &block.kind {
            Kind::Code => p.code(&block.text, top),
            kind => p.text(kind, &block.text, top),
        };
        if !complete {
            p.ellipsis();
            break;
        }
        prev = Some(&block.kind);
    }
    p.out
}

#[cfg(test)]
mod tests {
    use super::*;

    const BTC: &str = "比特币现在约 **76,878 USDT**（Binance BTCUSDT 现货）。\n\n- **24 小时涨跌：** −1,017 USDT（−1.31%）\n- **24 小时区间：** 最低 76,705，最高 78,878\n- **24 小时成交量：** 约 12,570 BTC，成交额约 9.79 亿 USDT\n- **盘口：** 买一 76,878.00，卖一 76,878.01\n\n现价离今天的最低点很近，只高出约 173。";

    const ROOMY: Frame = Frame {
        width: 420,
        height: 2000,
    };
    /// A finished card without an error line (view.rs).
    const CARD: Frame = Frame {
        width: 420,
        height: 224,
    };

    /// The runs read in order, a gap between runs on a line read as a space.
    fn text_of(l: &Layout) -> String {
        let mut runs = l.runs.clone();
        runs.sort_by_key(|r| (r.y, r.x64));
        let mut out = String::new();
        let mut end: Option<(i32, u32)> = None;
        for r in &runs {
            if end.is_some_and(|(y, x)| y == r.y && r.x64 > x + 64) {
                out.push(' ');
            }
            out.push_str(&r.text);
            end = Some((r.y, r.x64 + measure(&r.text, SIZE)));
        }
        out
    }

    fn line_ys(l: &Layout) -> Vec<i32> {
        let mut ys: Vec<i32> = l.runs.iter().map(|r| r.y).collect();
        ys.sort_unstable();
        ys.dedup();
        ys
    }

    /// Where GSP would draw each character of a label's line: its x, 1/64 px.
    fn drawn(line: &str) -> Vec<(char, u32)> {
        let mut x = 0;
        let mut out = Vec::new();
        for ch in line.chars() {
            if !ch.is_whitespace() {
                out.push((ch, x));
            }
            x += advance(ch, SIZE);
        }
        out
    }

    #[test]
    fn strong_runs_are_bright_and_markers_disappear() {
        let l = layout("约 **76,878 USDT**（现货）", &ROOMY);
        assert_eq!(l.runs.len(), 3);
        assert_eq!(
            (l.runs[1].ink, l.runs[1].text.as_str()),
            (Ink::Strong, "76,878 USDT")
        );
        assert_eq!(l.runs[0].ink, Ink::Normal);
        assert_eq!(l.runs[1].x64, measure("约 ", SIZE));
        assert!(!text_of(&l).contains('*'));
    }

    #[test]
    fn padding_puts_each_run_within_a_quarter_pixel_of_its_place() {
        let l = layout(BTC, &ROOMY);
        let labels = l.labels();
        for r in &l.runs {
            let set = r.half.rem_euclid(2) as usize;
            let line = labels[set][r.ink as usize]
                .split('\n')
                .nth((r.half / 2) as usize)
                .unwrap();
            let first = r.text.chars().next().unwrap();
            let at = drawn(line)
                .into_iter()
                .filter(|&(c, _)| c == first)
                .map(|(_, x)| x)
                .min_by_key(|&x| x.abs_diff(r.x64))
                .unwrap();
            assert!(at + 16 >= r.x64, "{r:?} drawn at {at}: too far left");
            assert!(at <= r.x64 + 5 * 64, "{r:?} drawn at {at}: too far right");
        }
    }

    #[test]
    fn the_real_reply_fills_a_card_paragraph_then_list() {
        let l = layout(BTC, &CARD);
        let all = text_of(&l);
        assert!(all.starts_with("比特币现在约 76,878 USDT"), "{all}");
        assert!(!all.contains("**") && !all.contains("- "), "{all}");
        let bullets: Vec<&Run> = l.runs.iter().filter(|r| r.text == "•").collect();
        assert_eq!(bullets.len(), 4, "{all}");
        for b in &bullets {
            assert_eq!(b.ink, Ink::Dim);
            let label = l
                .runs
                .iter()
                .find(|r| r.y == b.y && r.ink == Ink::Strong)
                .expect("a strong label on the bullet's line");
            assert_eq!(label.x, GUTTER);
        }
        for r in &l.runs {
            assert!(r.y + LINE <= CARD.height);
            assert!(r.x64 + measure(&r.text, SIZE) <= 420 * 64, "{r:?}");
        }
        // The closing paragraph does not fit: the list's last line says so.
        assert!(l.clipped && all.ends_with('…'), "{all}");
        // The list sits half a line below the paragraph, on the other label set.
        let first_item = bullets[0].half;
        assert_eq!(first_item % 2, 1);
        let labels = l.labels();
        assert!(labels[0][Ink::Strong as usize].contains("76,878 USDT"));
        assert!(labels[1][Ink::Strong as usize].contains("24 小时涨跌："));
        assert_eq!(labels[1][Ink::Dim as usize].matches('•').count(), 4);
    }

    #[test]
    fn the_real_reply_in_full_keeps_every_block() {
        let l = layout(BTC, &ROOMY);
        assert!(!l.clipped);
        let all = text_of(&l);
        assert!(all.contains("−1,017 USDT（−1.31%）"), "{all}");
        assert!(all.ends_with("只高出约 173。"), "{all}");
        let ys = line_ys(&l);
        let steps: Vec<i32> = ys.windows(2).map(|w| w[1] - w[0]).collect();
        // Lines in a block and items in a list sit a line apart, blocks a line and a half.
        assert!(
            steps
                .iter()
                .all(|&s| s == LINE || s == LINE + LINE / 2 || s == LINE + LINE / 2 + 1),
            "{steps:?}"
        );
        assert!(steps.iter().any(|&s| s > LINE), "{steps:?}");
    }

    #[test]
    fn a_strong_run_wraps_across_lines() {
        let text = alloc::format!("{}**{}**尾", "前".repeat(18), "粗体".repeat(6));
        let l = layout(&text, &ROOMY);
        let strong: Vec<&Run> = l.runs.iter().filter(|r| r.ink == Ink::Strong).collect();
        assert_eq!(strong.len(), 2, "{l:?}");
        assert_eq!((strong[1].x, strong[1].y), (0, LINE));
        assert_eq!(
            alloc::format!("{}{}", strong[0].text, strong[1].text),
            "粗体".repeat(6)
        );
    }

    #[test]
    fn overflow_ends_the_last_line_with_an_ellipsis() {
        let small = Frame {
            height: LINE * 2,
            ..ROOMY
        };
        let l = layout(&"很长的内容".repeat(60), &small);
        assert!(l.clipped);
        assert_eq!(line_ys(&l), [0, LINE]);
        let last = l.runs.last().unwrap();
        assert!(last.text.ends_with('…'));
        assert!(last.x64 + measure(&last.text, SIZE) <= 420 * 64);
    }

    #[test]
    fn headings_are_strong_with_a_line_of_air_above() {
        let l = layout("intro\n\n## Result\n\nbody", &ROOMY);
        let h = l.runs.iter().find(|r| r.text == "Result").unwrap();
        assert_eq!(h.ink, Ink::Strong);
        assert_eq!(h.y, 2 * LINE);
        let body = l.runs.iter().find(|r| r.text == "body").unwrap();
        assert_eq!(body.y, half_y(7));
    }

    #[test]
    fn numbers_right_align_and_nested_bullets_indent() {
        let l = layout("1. one\n2. two\n  - inner\n- outer", &ROOMY);
        let one = l.runs.iter().find(|r| r.text == "1.").unwrap();
        let w = (measure("1.", SIZE).div_ceil(64)) as i32;
        assert_eq!(one.x + w, GUTTER - 6);
        let inner = l.runs.iter().find(|r| r.text == "inner").unwrap();
        assert_eq!(inner.x, 2 * GUTTER);
        assert!(l.runs.iter().any(|r| r.text == "–" && r.x == GUTTER + 3));
        assert_eq!(line_ys(&l), [0, LINE, 2 * LINE, 3 * LINE]);
    }

    #[test]
    fn quotes_are_dim_with_a_rule_and_strong_inside_is_normal() {
        let l = layout("> quoted **key** line\n> second", &ROOMY);
        assert_eq!(
            l.rule,
            Some(Band {
                y: 4,
                h: 2 * LINE - 8
            })
        );
        assert!(l.runs.iter().all(|r| r.x >= QUOTE_X));
        let key = l.runs.iter().find(|r| r.text == "key").unwrap();
        assert_eq!(key.ink, Ink::Normal);
        assert_eq!(l.runs[0].ink, Ink::Dim);
    }

    #[test]
    fn code_is_one_mono_label_on_a_panel_and_text_resumes_on_the_grid() {
        let l = layout(
            "run:\n\n```sh\ncargo test -p logic\n# 中文注释\n```\n\nafter",
            &ROOMY,
        );
        let code = l.code.as_ref().unwrap();
        assert_eq!(code.lines, ["cargo test -p logic", "# 中文注释"]);
        let top = half_y(3);
        assert_eq!(
            l.panel,
            Some(Band {
                y: top,
                h: 2 * CODE_LINE + 2 * CODE_PAD
            })
        );
        assert_eq!(code.y, top + CODE_PAD);
        let after = l.runs.iter().find(|r| r.text == "after").unwrap();
        let bottom = top + 2 * CODE_LINE + 2 * CODE_PAD;
        assert!(
            after.y > bottom && after.y <= bottom + LINE,
            "{} vs {bottom}",
            after.y
        );
        assert_eq!(after.y, half_y(after.half));
    }

    #[test]
    fn long_code_lines_are_cut_and_a_second_block_ends_the_reply() {
        let long = "x".repeat(200);
        let l = layout(
            &alloc::format!("```\n{long}\n```\n\n```\nmore\n```"),
            &ROOMY,
        );
        let code = l.code.as_ref().unwrap();
        assert!(code.lines[0].ends_with('…'));
        assert!(code.lines[0].chars().count() <= code_chars(420));
        assert!(l.clipped);
    }

    #[test]
    fn raw_markdown_is_read_tolerantly() {
        let l = layout(
            "* star __bold__ and *it* and _em_ and `code` and [a link](https://x.y)\n+ plus\nsnake_case_name stays",
            &ROOMY,
        );
        let all = text_of(&l);
        assert!(
            all.contains("• star bold and it and em and code and a link"),
            "{all}"
        );
        assert!(all.contains("snake_case_name stays"), "{all}");
        assert_eq!(l.runs.iter().filter(|r| r.text == "•").count(), 2);
        assert!(
            l.runs
                .iter()
                .any(|r| r.text == "bold" && r.ink == Ink::Strong)
        );
    }

    #[test]
    fn a_reply_cut_mid_strong_drops_the_stray_marker() {
        let l = layout("价格 **76,8", &ROOMY);
        assert_eq!(text_of(&l), "价格 76,8");
    }

    #[test]
    fn plain_lines_in_a_paragraph_keep_their_breaks() {
        let l = layout("first line\nsecond line", &ROOMY);
        assert_eq!(line_ys(&l), [0, LINE]);
        assert_eq!(
            l.labels()[0][Ink::Normal as usize],
            "first line\nsecond line"
        );
    }

    #[test]
    fn empty_input_places_nothing() {
        assert_eq!(layout("  \n\n", &ROOMY), Layout::default());
    }
}
