#!/usr/bin/env node
// PostToolUse (Edit|Write|MultiEdit): format the edited Rust file with rustfmt (repo rustfmt.toml).
// Silent no-op when the file is not .rs or rustfmt is not installed; never blocks the agent.
import { readFileSync } from "node:fs";
import { spawnSync } from "node:child_process";

const input = JSON.parse(readFileSync(0, "utf8"));
const file = String(input.tool_input?.file_path ?? "");
if (!file.endsWith(".rs")) process.exit(0);

const r = spawnSync("rustfmt", ["--edition", "2024", file], { encoding: "utf8" });
if (r.error) process.exit(0); // rustfmt not installed yet
if (r.status !== 0 && r.stderr) {
  // Surface syntax errors to the agent without blocking (exit 0 + stderr shown in transcript).
  console.error(`rustfmt could not format ${file}:\n${r.stderr.slice(0, 2000)}`);
}
process.exit(0);
