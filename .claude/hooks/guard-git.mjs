#!/usr/bin/env node
// PreToolUse (Bash|PowerShell): protect the integration branches.
// - No direct pushes to main/develop: everything lands via PR (docs/08-development-workflow.md).
// - No force pushes to main/develop (force-with-lease on your own feature branch is fine).
// - No --no-verify: fix the hook failure instead of skipping it.
import { readFileSync } from "node:fs";

const input = JSON.parse(readFileSync(0, "utf8"));
const cmd = String(input.tool_input?.command ?? "");

const deny = (reason) => {
  console.log(
    JSON.stringify({
      hookSpecificOutput: {
        hookEventName: "PreToolUse",
        permissionDecision: "deny",
        permissionDecisionReason: `Blocked by project hook: ${reason}`,
      },
    }),
  );
  process.exit(0);
};

// Inspect each git invocation separately (commands may be chained with && ; |).
for (const part of cmd.split(/&&|\|\||;|\n/)) {
  const words = part.trim().split(/\s+/);
  const g = words.indexOf("git");
  if (g === -1) continue;
  const args = words.slice(g + 1);
  if (args.includes("--no-verify")) {
    deny("`--no-verify` skips the repo's git hooks. Fix the underlying failure instead.");
  }
  if (args[0] !== "push") continue;
  const protectedRef = args.some((a) => /^(\+|HEAD:|[^:]*:)?(refs\/heads\/)?(main|develop)$/.test(a));
  if (protectedRef) {
    deny(
      "direct pushes to main/develop are not allowed. Push your feature/session branch and open a " +
        "PR against develop (`gh pr create --base develop`). See docs/08-development-workflow.md.",
    );
  }
  if (args.some((a) => a === "--force" || a === "-f") && !args.some((a) => a.startsWith("--force-with-lease"))) {
    deny("plain --force push. Use --force-with-lease on your own feature branch if you really need it.");
  }
}
process.exit(0);
