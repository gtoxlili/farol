// Reply markdown -> "Farol markdown", the subset the board renders (see
// protocol/README.md, "Reply bodies"). Block lines: `# `/`## `/`### `, `- `
// and one nested `  - `, `1. `, `> `, fenced code; a blank line between
// blocks; inline only `**strong**`. Everything else maps onto that or goes,
// and characters the board's font lacks (emoji) become text or are dropped.

/** About how long a reply body may be, in code points, markup included. */
export const BODY_MAX = 320;
/** A cut line keeps at least this many characters of its own, else it is left out. */
const MIN_PARTIAL = 12;
const ELLIPSIS = "…";

type Block = { kind: "text"; lines: string[] } | { kind: "code"; lines: string[] };

/** Emoji with an obvious text equivalent in the board's font. */
const EMOJI_TEXT: Array<[RegExp, string]> = [
  [/[✅✔☑]️?/gu, "✓"],
  [/[❌❎✖]️?/gu, "✗"],
  [/[⚠❗❕]️?/gu, "!"],
  [/[❓❔]️?/gu, "?"],
  [/‼️?/gu, "!!"],
  [/⁉️?/gu, "!?"],
  [/⭐️?/gu, "★"],
  [/➡️?/gu, "→"],
  [/⬅️?/gu, "←"],
  [/⬆️?/gu, "↑"],
  [/⬇️?/gu, "↓"],
];

/**
 * Pictographs the font lacks, with their joiners, variation selectors, skin
 * tones, keycaps and flags. Latin-1 (©, ®) and ™ stay.
 */
const EMOJI_DROP =
  /(?![©®™])\p{Extended_Pictographic}|[‍︎️⃣]|[\u{1F3FB}-\u{1F3FF}]|[\u{1F1E6}-\u{1F1FF}]|[\u{E0020}-\u{E007F}]/gu;

export function stripEmoji(text: string): string {
  let out = text;
  for (const [pattern, replacement] of EMOJI_TEXT) out = out.replace(pattern, replacement);
  return out.replace(EMOJI_DROP, "");
}

const ENTITIES: Record<string, string> = {
  amp: "&",
  lt: "<",
  gt: ">",
  quot: '"',
  apos: "'",
  nbsp: " ",
  middot: "·",
  hellip: "…",
  mdash: "—",
  ndash: "–",
  minus: "−",
  times: "×",
  rarr: "→",
  larr: "←",
  copy: "©",
  reg: "®",
  trade: "™",
};

function decodeEntities(text: string): string {
  return text.replace(/&(#x[0-9a-f]+|#\d+|[a-z]+);/gi, (whole, name: string) => {
    if (name[0] === "#") {
      const code = name[1] === "x" || name[1] === "X" ? parseInt(name.slice(2), 16) : parseInt(name.slice(1), 10);
      return Number.isFinite(code) && code > 0 && code <= 0x10ffff ? String.fromCodePoint(code) : whole;
    }
    return ENTITIES[name.toLowerCase()] ?? whole;
  });
}

// Placeholders for text no later inline rule may touch (code spans, escapes).
const HOLD_OPEN = "";
const HOLD_CLOSE = "";

/** One line of inline markdown -> plain text with only `**strong**`. */
export function inline(text: string): string {
  const held: string[] = [];
  const hold = (value: string) => `${HOLD_OPEN}${held.push(value) - 1}${HOLD_CLOSE}`;
  let s = text;
  // Code spans become strong; their content is literal.
  s = s.replace(/(`+)([^`]|[^`][\s\S]*?[^`])\1(?!`)/g, (_m, _ticks, code: string) => {
    const content = stripEmoji(code.trim().replace(/\*\*/g, ""));
    return content ? hold(`**${content}**`) : "";
  });
  // Backslash escapes stay literal.
  s = s.replace(/\\([\\`*_{}[\]()#+\-.!|>~<])/g, (_m, c: string) => hold(c));
  // Images -> alt text, links -> their text, autolinks -> the address.
  s = s.replace(/!\[([^\]]*)\]\([^)]*\)/g, "$1");
  s = s.replace(/!\[([^\]]*)\]\[[^\]]*\]/g, "$1");
  s = s.replace(/\[([^\]]+)\]\((?:[^()]|\([^)]*\))*\)/g, "$1");
  s = s.replace(/\[([^\]]+)\]\[[^\]]*\]/g, "$1");
  s = s.replace(/<((?:https?|mailto):[^>\s]+)>/gi, "$1");
  // HTML: line breaks become spaces, other tags go, entities decode (held, so
  // a decoded "*" or "<" stays literal).
  s = s.replace(/<br\s*\/?>/gi, " ");
  s = s.replace(/<\/?[a-z][a-z0-9-]*(?:\s[^<>]*)?\/?>/gi, "");
  s = s.replace(/<!--[\s\S]*?-->/g, "");
  s = s.replace(/&(?:#x[0-9a-f]+|#\d+|[a-z]+);/gi, (entity) => {
    const decoded = decodeEntities(entity);
    return decoded === entity ? entity : hold(decoded);
  });
  // Emphasis: strong stays strong, everything else is plain.
  s = s.replace(/\*\*\*(?=\S)([\s\S]*?\S)\*\*\*/g, "**$1**");
  s = s.replace(/(^|[^\w])__(?=\S)([\s\S]*?\S)__(?![\w])/g, "$1**$2**");
  s = s.replace(/~~(?=\S)([\s\S]*?\S)~~/g, "$1");
  s = s.replace(/(^|[^*\w])\*(?=[^\s*])([^*]*?[^\s*])\*(?![*\w])/g, "$1$2");
  s = s.replace(/(^|[^\w])_(?=[^\s_])([^_]*?[^\s_])_(?![\w])/g, "$1$2");
  s = balanceStrong(s);
  s = stripEmoji(s).replace(/[ \t]{2,}/g, " ").trim();
  return s.replace(new RegExp(`${HOLD_OPEN}(\\d+)${HOLD_CLOSE}`, "g"), (_m, i: string) => held[Number(i)] ?? "");
}

/** Drops the last `**` when their count is odd, so the board never sees an open strong. */
function balanceStrong(s: string): string {
  const count = s.split("**").length - 1;
  if (count % 2 === 0) return s;
  const last = s.lastIndexOf("**");
  return s.slice(0, last) + s.slice(last + 2);
}

const FENCE = /^\s{0,3}(`{3,}|~{3,})/;
const HEADING = /^\s{0,3}(#{1,6})\s+(.*?)(?:\s+#+)?\s*$/;
const RULE = /^\s{0,3}([-*_])(?:\s*\1){2,}\s*$/;
const QUOTE = /^\s{0,3}>\s?(.*)$/;
const ITEM = /^(\s*)([-*+]|\d{1,9}[.)])\s+(.*)$/;
const TABLE_SEPARATOR = /^\s*\|?\s*:?-{1,}:?\s*(\|\s*:?-{1,}:?\s*)*\|?\s*$/;
const REFERENCE = /^\s{0,3}\[[^\]]+\]:\s*\S+/;

function cells(row: string): string[] {
  const trimmed = row.trim().replace(/^\|/, "").replace(/\|$/, "");
  return trimmed.split(/(?<!\\)\|/).map((c) => c.trim());
}

function isTableStart(lines: string[], i: number): boolean {
  const line = lines[i]!;
  const next = lines[i + 1];
  return line.includes("|") && next !== undefined && next.includes("-") && TABLE_SEPARATOR.test(next) && next.includes("|");
}

/** Parses reply markdown into Farol blocks (no clipping). */
export function toBlocks(markdown: string): Block[] {
  const lines = markdown.replace(/\r\n?/g, "\n").split("\n");
  const blocks: Block[] = [];
  /** Lines of the text block being built (held in an object: closures assign it). */
  const open: { lines: string[] | null } = { lines: null };
  /** The current block is a list and its last line can take a lazy continuation. */
  let inList = false;
  let inQuote = false;

  const close = () => {
    if (open.lines?.length) blocks.push({ kind: "text", lines: open.lines });
    open.lines = null;
    inList = false;
    inQuote = false;
  };
  const push = (line: string) => {
    (open.lines ??= []).push(line);
  };

  for (let i = 0; i < lines.length; i++) {
    const raw = lines[i]!;
    const fence = FENCE.exec(raw);
    if (fence) {
      close();
      const marker = fence[1]!;
      const code: string[] = [];
      let j = i + 1;
      for (; j < lines.length; j++) {
        const l = lines[j]!;
        if (l.trim().startsWith(marker[0]!.repeat(marker.length)) && /^\s*[`~]+\s*$/.test(l)) break;
        code.push(stripEmoji(l.replace(/\t/g, "  ").trimEnd()));
      }
      i = j;
      while (code.length && !code[code.length - 1]) code.pop();
      while (code.length && !code[0]) code.shift();
      if (code.length) blocks.push({ kind: "code", lines: code });
      continue;
    }
    if (!raw.trim()) {
      close();
      continue;
    }
    if (REFERENCE.test(raw)) continue;
    // Setext headings: a paragraph line underlined with === or ---.
    const paragraph: string[] | null = !inList && !inQuote ? open.lines : null;
    if (paragraph?.length && /^\s{0,3}(=+|-+)\s*$/.test(raw)) {
      const title = paragraph.pop()!;
      close();
      blocks.push({ kind: "text", lines: [`${raw.trim()[0] === "=" ? "#" : "##"} ${title}`] });
      continue;
    }
    if (RULE.test(raw)) {
      close();
      continue;
    }
    const heading = HEADING.exec(raw);
    if (heading) {
      close();
      const body = inline(heading[2]!);
      if (body) blocks.push({ kind: "text", lines: [`${"#".repeat(Math.min(3, heading[1]!.length))} ${body}`] });
      continue;
    }
    if (isTableStart(lines, i)) {
      close();
      const rows: string[] = [];
      const header = cells(raw).map((c) => inline(c).replace(/\*\*/g, "")).filter(Boolean);
      if (header.length) rows.push(`**${header.join(" · ")}**`);
      let j = i + 2;
      for (; j < lines.length && lines[j]!.includes("|") && lines[j]!.trim(); j++) {
        const row = cells(lines[j]!).map(inline).filter(Boolean);
        if (row.length) rows.push(row.join(" · "));
      }
      i = j - 1;
      if (rows.length) blocks.push({ kind: "text", lines: rows });
      continue;
    }
    const quote = QUOTE.exec(raw);
    if (quote) {
      if (!inQuote) close();
      inQuote = true;
      // Nested quotes and lists inside a quote flatten to quoted text.
      const body = inline(quote[1]!.replace(/^(\s*>\s?)+/, "").replace(/^\s*([-*+]|\d{1,9}[.)])\s+/, ""));
      if (body) push(`> ${body}`);
      continue;
    }
    const item = ITEM.exec(raw);
    if (item && !RULE.test(raw)) {
      if (!inList) close();
      inList = true;
      const nested = item[1]!.replace(/\t/g, "    ").length >= 2;
      let body = item[3]!;
      const task = /^\[([ xX])\]\s+/.exec(body);
      if (task) body = (task[1] === " " ? "" : "✓ ") + body.slice(task[0].length);
      const content = inline(body);
      if (!content) continue;
      const marker = item[2]!;
      if (nested) push(`  - ${content}`);
      else if (/^\d/.test(marker)) push(`${marker.slice(0, -1)}. ${content}`);
      else push(`- ${content}`);
      continue;
    }
    const content = inline(raw);
    if (!content) continue;
    const last = open.lines;
    if ((inList || inQuote) && last?.length) {
      // A lazy continuation of the list item or quote line before it.
      last[last.length - 1] += ` ${content}`;
      continue;
    }
    push(content);
  }
  close();
  return blocks;
}

function render(blocks: Block[]): string {
  return blocks
    .map((b) => (b.kind === "code" ? ["```", ...b.lines, "```"] : b.lines).join("\n"))
    .join("\n\n");
}

const len = (s: string) => Array.from(s).length;

/** Whether a cut between `before` and `after` splits a Latin word or number. */
function splitsWord(before: string, after: string): boolean {
  return /[\p{L}\p{N}]$/u.test(before) && /^[\p{L}\p{N}]/u.test(after) && /[A-Za-z0-9]$/.test(before) && /^[A-Za-z0-9]/.test(after);
}

/** The start of `line` within `room` code points, cut at a word boundary, strong balanced, ending in "…". */
function cutLine(line: string, room: number): string | null {
  const chars = Array.from(line);
  const prefix = /^(#{1,3} |\s*- |\d{1,9}\. |> )/.exec(line)?.[0] ?? "";
  // Room for the ellipsis and a closing "**".
  const budget = room - 3;
  if (budget - len(prefix) < MIN_PARTIAL) return null;
  let cut = budget;
  for (let i = budget; i > len(prefix) + MIN_PARTIAL; i--) {
    const before = chars.slice(0, i).join("");
    const after = chars.slice(i).join("");
    if (!splitsWord(before, after) && !(before.endsWith("*") && after.startsWith("*"))) {
      cut = i;
      break;
    }
  }
  let head = chars.slice(0, cut).join("").trimEnd();
  // Never leave half of a "**" behind.
  if (head.endsWith("*") && !head.endsWith("**")) head = head.slice(0, -1).trimEnd();
  if ((head.split("**").length - 1) % 2 === 1) {
    if (head.endsWith("**")) head = head.slice(0, -2).trimEnd();
    else head += "**";
  }
  if (len(head) - len(prefix) < 1) return null;
  return head + ELLIPSIS;
}

/** Fits blocks into `max` code points, cutting at a block, line or word boundary. */
export function clipBlocks(blocks: Block[], max: number): string {
  const whole = render(blocks);
  if (len(whole) <= max) return whole;
  const out: Block[] = [];
  let used = 0;
  for (const block of blocks) {
    const sep = out.length ? 2 : 0;
    const size = len(render([block]));
    // Reserve room for the ending ("…", or a "\n…" line inside a fence).
    if (used + sep + size <= max - 2) {
      out.push(block);
      used += sep + size;
      continue;
    }
    const room = max - used - sep;
    const part = cutBlock(block, room);
    if (part) out.push(part);
    else endWithEllipsis(out);
    return render(out);
  }
  endWithEllipsis(out);
  return render(out);
}

function cutBlock(block: Block, room: number): Block | null {
  const lines: string[] = [];
  if (block.kind === "code") {
    // "```\n" + lines + "\n…\n```"
    let left = room - 4 - 6;
    for (const line of block.lines) {
      const cost = len(line) + 1;
      if (cost > left) {
        if (!lines.length && left > MIN_PARTIAL) lines.push(Array.from(line).slice(0, left - 1).join(""));
        break;
      }
      lines.push(line);
      left -= cost;
    }
    if (!lines.length) return null;
    lines.push(ELLIPSIS);
    return { kind: "code", lines };
  }
  let left = room;
  for (const line of block.lines) {
    const sep = lines.length ? 1 : 0;
    const cost = sep + len(line);
    if (cost <= left - 1) {
      lines.push(line);
      left -= cost;
      continue;
    }
    const part = cutLine(line, left - sep);
    if (part) lines.push(part);
    else if (lines.length) lines[lines.length - 1] += ELLIPSIS;
    return lines.length ? { kind: "text", lines } : null;
  }
  if (!lines.length) return null;
  lines[lines.length - 1] += ELLIPSIS;
  return { kind: "text", lines };
}

function endWithEllipsis(out: Block[]): void {
  const last = out[out.length - 1];
  if (!last) {
    out.push({ kind: "text", lines: [ELLIPSIS] });
  } else if (last.kind === "code") {
    last.lines = [...last.lines, ELLIPSIS];
  } else {
    last.lines = [...last.lines.slice(0, -1), last.lines[last.lines.length - 1] + ELLIPSIS];
  }
}

/** A reply as a Farol markdown body of about `max` code points; "" when nothing is left. */
export function farolMarkdown(reply: string | null | undefined, max = BODY_MAX): string {
  if (!reply) return "";
  return clipBlocks(toBlocks(reply), max);
}
