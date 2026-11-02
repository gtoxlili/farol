// Stopping a turn. Paseo's public SDK has no cancel (0.14), so Farol runs the
// documented CLI, `paseo stop <id>`, against the daemon it lives in: the
// plugin process inherits the daemon's environment, including PASEO_HOME.
import { execFile } from "node:child_process";
import { accessSync, constants, existsSync } from "node:fs";
import path from "node:path";

const STOP_TIMEOUT_MS = 15_000;

function executable(file: string): boolean {
  try {
    accessSync(file, constants.X_OK);
    return true;
  } catch {
    return false;
  }
}

/**
 * Finds the paseo CLI: $FAROL_PASEO_CLI, then PATH, then the CLI package this
 * daemon runs from (the plugin worker lives inside @getpaseo/cli's tree).
 */
export function findPaseoCli(env: NodeJS.ProcessEnv = process.env, argv: string[] = process.argv): string | null {
  if (env.FAROL_PASEO_CLI && executable(env.FAROL_PASEO_CLI)) return env.FAROL_PASEO_CLI;
  for (const dir of (env.PATH ?? "").split(path.delimiter)) {
    if (!dir) continue;
    const candidate = path.join(dir, "paseo");
    if (executable(candidate)) return candidate;
  }
  let dir = argv[1] ? path.dirname(argv[1]) : "";
  while (dir && dir !== path.dirname(dir)) {
    const bin = path.join(dir, "bin", "paseo");
    if (path.basename(dir) === "cli" && existsSync(path.join(dir, "package.json")) && executable(bin)) {
      return bin;
    }
    const nested = path.join(dir, "node_modules", "@getpaseo", "cli", "bin", "paseo");
    if (executable(nested)) return nested;
    dir = path.dirname(dir);
  }
  return null;
}

export function stopArgs(agentId: string, env: NodeJS.ProcessEnv = process.env): string[] {
  const args = ["stop", agentId, "--json"];
  if (env.PASEO_HOME) args.push("--home", env.PASEO_HOME);
  return args;
}

export function runStop(cli: string, agentId: string, env: NodeJS.ProcessEnv = process.env): Promise<void> {
  return new Promise((resolve, reject) => {
    execFile(cli, stopArgs(agentId, env), { timeout: STOP_TIMEOUT_MS, env }, (error, _stdout, stderr) => {
      if (error) reject(new Error(stderr.trim() || error.message));
      else resolve();
    });
  });
}
