// Text shaping for a 480 px screen: single lines, bounded lengths.

/** Collapses whitespace and cuts to `max` characters (code points) with an ellipsis. */
export function clip(text: string | null | undefined, max: number): string {
  if (!text) return "";
  const flat = text.replace(/\s+/g, " ").trim();
  const chars = Array.from(flat);
  if (chars.length <= max) return flat;
  return chars.slice(0, Math.max(0, max - 1)).join("").trimEnd() + "…";
}

/** Like `clip` but keeps paragraph breaks, for reply previews. */
export function clipBlock(text: string | null | undefined, max: number): string {
  if (!text) return "";
  const tidy = text
    .replace(/```[\s\S]*?```/g, " ")
    .replace(/[ \t]+/g, " ")
    .replace(/\n{2,}/g, "\n")
    .trim();
  const chars = Array.from(tidy);
  if (chars.length <= max) return tidy;
  return chars.slice(0, Math.max(0, max - 1)).join("").trimEnd() + "…";
}

/**
 * Short model label for the card header: "claude-opus-5-5" -> "opus 5.5",
 * "gpt-6.1-sol" -> "gpt-6.1 sol".
 */
export function modelLabel(model: string | null | undefined): string {
  if (!model) return "";
  let m = model.trim();
  const slash = m.lastIndexOf("/");
  if (slash >= 0) m = m.slice(slash + 1);
  m = m.replace(/^claude-/, "").replace(/\[.*\]$/, "").replace(/-\d{8}$/, "");
  m = m.replace(/(\d+)-(\d+)(?=$|-)/g, "$1.$2");
  return clip(m.replace(/[-_]+/g, " ").replace(/^gpt (?=\d)/, "gpt-"), 24);
}

export function formatCost(usd: number | null | undefined): string | undefined {
  if (usd == null || !Number.isFinite(usd) || usd <= 0) return undefined;
  if (usd < 0.01) return "<$0.01";
  if (usd < 100) return `$${usd.toFixed(2)}`;
  return `$${Math.round(usd)}`;
}

/** Path relative to `cwd` when inside it. */
export function shortPath(path: string, cwd: string | null | undefined): string {
  if (cwd && path.startsWith(cwd.endsWith("/") ? cwd : `${cwd}/`)) {
    return path.slice(cwd.length).replace(/^\/+/, "");
  }
  return path;
}

/** ms since epoch for an ISO string, or null. */
export function epochMs(iso: string | null | undefined): number | null {
  if (!iso) return null;
  const t = Date.parse(iso);
  return Number.isFinite(t) ? t : null;
}

/** Minutes east of UTC for `zone` (IANA) at `at`; empty or invalid follows the host. */
export function tzOffsetMinutes(zone: string, at: number): number {
  if (zone) {
    try {
      const parts = new Intl.DateTimeFormat("en-US", {
        timeZone: zone,
        timeZoneName: "longOffset",
      }).formatToParts(new Date(at));
      const name = parts.find((p) => p.type === "timeZoneName")?.value ?? "";
      const m = /GMT([+-])(\d{2}):?(\d{2})?/.exec(name);
      if (m) return (m[1] === "-" ? -1 : 1) * (Number(m[2]) * 60 + Number(m[3] ?? 0));
      if (name === "GMT") return 0;
    } catch {
      // fall through to the host zone
    }
  }
  return -new Date(at).getTimezoneOffset();
}
