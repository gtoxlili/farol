// Reply bodies in Farol markdown: the subset the board renders, see protocol/README.md.
import { describe, expect, it } from "vitest";
import { BODY_MAX, farolMarkdown } from "../server/markdown";

const len = (s: string) => Array.from(s).length;
const strongBalanced = (s: string) => (s.split("**").length - 1) % 2 === 0;

// A Sonnet reply from the user's daemon.
const BTC =
  "比特币现在约 **76,878 USDT**（Binance BTCUSDT 现货）。\n\n- **24 小时涨跌：** −1,017 USDT（−1.31%）\n- **24 小时区间：** 最低 76,705，最高 78,878\n- **24 小时成交量：** 约 12,570 BTC，成交额约 9.79 亿 USDT\n- **盘口：** 买一 76,878.00，卖一 76,878.01\n\n现价离今天的最低点很近，只高出约 173。";

describe("farol markdown", () => {
  it("keeps a reply that is already in the subset as it is", () => {
    expect(farolMarkdown(BTC)).toBe(BTC);
  });

  it("cuts the same reply at a line or word boundary, strong balanced, ending in …", () => {
    for (const max of [40, 60, 90, 120, 160, 200]) {
      const out = farolMarkdown(BTC, max);
      expect(len(out)).toBeLessThanOrEqual(max);
      expect(out.endsWith("…")).toBe(true);
      expect(strongBalanced(out)).toBe(true);
      expect(out).not.toMatch(/[^*]\*…|^\*…/);
    }
    expect(farolMarkdown(BTC, 120)).toBe(
      "比特币现在约 **76,878 USDT**（Binance BTCUSDT 现货）。\n\n- **24 小时涨跌：** −1,017 USDT（−1.31%）\n- **24 小时区间：** 最低 76,705，最高 78,878…",
    );
    // Never inside a number or a Latin word.
    expect(farolMarkdown("alpha beta gamma delta epsilon zeta eta theta iota kappa", 30)).toBe("alpha beta gamma delta…");
  });

  it("keeps the block lines the board renders", () => {
    const md = "# One\n## Two\n#### Four\n\n- a\n  - b\n      - c\n\n1. x\n2) y\n\n> q\n> > qq\n\n```ts\nlet a = 1;\n```";
    expect(farolMarkdown(md, 1000)).toBe(
      "# One\n\n## Two\n\n### Four\n\n- a\n  - b\n  - c\n\n1. x\n2. y\n\n> q\n> qq\n\n```\nlet a = 1;\n```",
    );
  });

  it("maps inline markdown onto strong or plain text", () => {
    expect(
      farolMarkdown(
        "*em* _em_ __strong__ ***both*** ~~gone~~ `code` [link](https://x.y/(z)) ![alt](a.png) snake_case_name a\\*b",
      ),
    ).toBe("em em **strong** **both** gone **code** link alt snake_case_name a*b");
  });

  it("turns tables into rows, drops rules, strips HTML and decodes entities", () => {
    const md = "| Name | `Qty` |\n| :--- | ---: |\n| **apple** | 2 |\n| pear | 3 |\n\n---\n\n<b>x</b> &amp; &lt;y&gt;<br>z &#x2212;1";
    expect(farolMarkdown(md)).toBe("**Name · Qty**\n**apple** · 2\npear · 3\n\nx & <y> z −1");
  });

  it("other bullets become `- `, setext headings become headings, task boxes become ✓", () => {
    expect(farolMarkdown("Intro\n* a\n+ b\n\nTitle\n=====\n\n- [x] done\n- [ ] open")).toBe(
      "Intro\n\n- a\n- b\n\n# Title\n\n- ✓ done\n- open",
    );
  });

  it("maps emoji with a text equivalent and drops the rest", () => {
    expect(farolMarkdown("✅ ok ❌ no ⚠️ careful 🚀 launch 👍🏽 🇨🇳 −5")).toBe("✓ ok ✗ no ! careful launch −5");
  });

  it("cuts inside a code fence and still closes it", () => {
    const code = Array.from({ length: 40 }, (_, i) => `line ${i}`).join("\n");
    const out = farolMarkdown("Run:\n\n```sh\n" + code + "\n```", 80);
    expect(len(out)).toBeLessThanOrEqual(80);
    expect(out.startsWith("Run:\n\n```\nline 0\n")).toBe(true);
    expect(out.endsWith("\n…\n```")).toBe(true);
  });

  it("an open strong is dropped rather than shown", () => {
    expect(farolMarkdown("**open and never closed")).toBe("open and never closed");
  });

  it("long replies stay within the body limit", () => {
    const long = Array.from({ length: 30 }, (_, i) => `- **item ${i}:** some words about it`).join("\n");
    const out = farolMarkdown(long);
    expect(len(out)).toBeLessThanOrEqual(BODY_MAX);
    expect(out.endsWith("…")).toBe(true);
    expect(farolMarkdown("")).toBe("");
    expect(farolMarkdown("🚀")).toBe("");
  });
});
