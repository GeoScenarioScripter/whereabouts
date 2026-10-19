#!/usr/bin/env node
// SessionStart: give the agent a two-line orientation — branch/worktree, active stage, toolchain.
// Output on stdout is added to the session context.
import { readFileSync } from "node:fs";
import { spawnSync } from "node:child_process";
import { join } from "node:path";

const root = process.env.CLAUDE_PROJECT_DIR ?? process.cwd();
const run = (cmd, args) => {
  const r = spawnSync(cmd, args, { cwd: root, encoding: "utf8" });
  return r.error || r.status !== 0 ? null : r.stdout.trim();
};

const branch = run("git", ["branch", "--show-current"]) ?? "(unknown)";
const inWorktree = (run("git", ["rev-parse", "--git-dir"]) ?? "").includes("/worktrees/");

let active = "unknown — see docs/stages/README.md";
try {
  const idx = readFileSync(join(root, "docs", "stages", "README.md"), "utf8");
  const m = idx.match(/^Active stage:\s*(.+)$/m);
  if (m) active = m[1].trim();
} catch {}

const lines = [
  `Whereabouts session context — branch: ${branch}${inWorktree ? " (session worktree)" : ""}; active stage: ${active}.`,
];
if (branch === "main" || branch === "develop") {
  lines.push(
    `You are on the integration branch '${branch}'. Code changes go on a feature/session branch and land via PR (docs/08-development-workflow.md).`,
  );
}
if (!run("cargo", ["--version"])) {
  lines.push(
    "Rust toolchain not found (cargo missing): you cannot build or test locally — say so in your verification notes and rely on CI.",
  );
}
console.log(lines.join("\n"));
