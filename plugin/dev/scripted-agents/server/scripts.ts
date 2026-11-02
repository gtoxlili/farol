// Scripts the fake agents play. The prompt's first word picks one ("work",
// "ask", "plan", "permission", "fail", "long", "quick"); the rest is ignored.
// Steps are data so the provider stays a plain interpreter.

export type Step =
  | { kind: "say"; text: string }
  | { kind: "tool"; name: string; detail: Record<string, unknown>; ms: number }
  | { kind: "todo"; items: Array<{ text: string; status: "pending" | "in_progress" | "completed" }> }
  | { kind: "wait"; ms: number }
  | {
      kind: "ask";
      request: {
        name: string;
        kind: "question" | "plan" | "tool";
        title?: string;
        input?: Record<string, unknown>;
        detail?: Record<string, unknown>;
      };
    }
  | { kind: "fail"; message: string }
  | { kind: "usage"; cost: number; used: number; max: number };

const todo = (done: number, labels: string[]): Step => ({
  kind: "todo",
  items: labels.map((text, i) => ({
    text,
    status: i < done ? "completed" : i === done ? "in_progress" : "pending",
  })),
});

const shell = (command: string, ms: number): Step => ({
  kind: "tool",
  name: "Bash",
  detail: { type: "shell", command },
  ms,
});

const WORK_TODO = ["Read the failing test", "Find the cause", "Fix it", "Run the suite"];

export const SCRIPTS: Record<string, (pace: number) => Step[]> = {
  quick: () => [{ kind: "say", text: "Done: nothing to change." }, { kind: "usage", cost: 0.01, used: 2000, max: 200000 }],
  work: (p) => [
    todo(0, WORK_TODO),
    { kind: "tool", name: "Read", detail: { type: "read", filePath: "src/lib.rs" }, ms: 2 * p },
    todo(1, WORK_TODO),
    { kind: "tool", name: "Grep", detail: { type: "search", toolName: "grep", query: "SpawnError" }, ms: 2 * p },
    todo(2, WORK_TODO),
    { kind: "tool", name: "Edit", detail: { type: "edit", filePath: "src/spawn.rs" }, ms: 2 * p },
    todo(3, WORK_TODO),
    shell("cargo test -p runtime --lib", 4 * p),
    todo(4, WORK_TODO),
    { kind: "say", text: "Fixed the spawn error mapping and added a regression test. All 212 tests pass." },
    { kind: "usage", cost: 0.42, used: 82000, max: 200000 },
  ],
  ask: (p) => [
    { kind: "tool", name: "Read", detail: { type: "read", filePath: "docs/orbs.md" }, ms: p },
    {
      kind: "ask",
      request: {
        name: "AskUserQuestion",
        kind: "question",
        title: "Question",
        input: {
          questions: [
            {
              question: "Which lamp style for the home screen?",
              header: "Lamp",
              multiSelect: false,
              options: [
                { label: "Halo", description: "Light pools at the bottom" },
                { label: "Plasma", description: "Cool to warm gradient" },
                { label: "Text only", description: "No orb" },
              ],
            },
          ],
        },
      },
    },
    { kind: "say", text: "Going with {answer}." },
    { kind: "usage", cost: 0.05, used: 12000, max: 200000 },
  ],
  plan: (p) => [
    { kind: "wait", ms: p },
    {
      kind: "ask",
      request: {
        name: "ExitPlanMode",
        kind: "plan",
        title: "Plan",
        detail: { type: "plan", text: "1. Add the long poll. 2. Add focus. 3. Add actions." },
      },
    },
    { kind: "say", text: "Plan {answer}." },
  ],
  permission: (p) => [
    { kind: "wait", ms: p },
    {
      kind: "ask",
      request: {
        name: "Bash",
        kind: "tool",
        title: "Run command",
        detail: { type: "shell", command: "rm -rf build" },
      },
    },
    { kind: "say", text: "Permission {answer}." },
  ],
  fail: (p) => [
    shell("npm run build", 2 * p),
    { kind: "say", text: "Stopped at step 5 of 8. Continue after the limit resets." },
    { kind: "fail", message: "usage limit reached · resets 15:00" },
  ],
  long: (p) => [
    todo(0, ["Run the long suite"]),
    shell("sleep 600 && cargo test --workspace", 600 * p),
    { kind: "say", text: "Long suite finished." },
  ],
};

export function scriptFor(prompt: string): { name: string; steps: Step[] } {
  const words = prompt.trim().toLowerCase().split(/\s+/);
  const name = words.find((w) => w in SCRIPTS) ?? "quick";
  // "fast" shortens every delay for tests.
  const pace = words.includes("fast") ? 150 : 600;
  return { name, steps: SCRIPTS[name]!(pace) };
}
