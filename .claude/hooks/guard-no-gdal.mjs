#!/usr/bin/env node
// PreToolUse (Edit|Write|MultiEdit): refuse to add GDAL bindings to any Cargo manifest.
// D-07: no GDAL in the core. GDAL may only be *invoked as a CLI* by the oracle golden-file scripts
// (tests/corpus/scripts) or live in the optional out-of-process Extended Format Pack (not in this
// workspace). cargo-deny enforces the same rule in CI; this hook gives the agent instant feedback.
import { readFileSync } from "node:fs";

const input = JSON.parse(readFileSync(0, "utf8"));
const ti = input.tool_input ?? {};
const file = String(ti.file_path ?? "").replace(/\\/g, "/");
if (!/(^|\/)Cargo\.toml$/i.test(file)) process.exit(0);

const texts = [ti.content, ti.new_string, ...(ti.edits ?? []).map((e) => e.new_string)];
const gdal = /^\s*(gdal|gdal-sys)\s*(=|\.)|package\s*=\s*"gdal(-sys)?"/m;
if (!texts.some((t) => typeof t === "string" && gdal.test(t))) process.exit(0);

console.log(
  JSON.stringify({
    hookSpecificOutput: {
      hookEventName: "PreToolUse",
      permissionDecision: "deny",
      permissionDecisionReason:
        "Blocked by project hook: GDAL must not be linked into any Whereabouts crate (D-07, D-08 in " +
        "docs/02-decisions.md). Write a native bounded-read reader instead, or — for test " +
        "expectations — call the gdalinfo/ogrinfo CLI from tests/corpus/scripts and commit the " +
        "golden JSON. If you believe D-07 should change, stop and raise it with the owner.",
    },
  }),
);
