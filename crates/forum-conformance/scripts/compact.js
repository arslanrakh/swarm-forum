#!/usr/bin/env node
// Compacts and selects ITF traces for the conformance fixtures (WP-F1, D.10.8).
//
// Usage: compact.js <keep> <out-dir> <prefix> <trace.itf.json>...
//
//   <keep>     how many traces to write: the richest first (below); 0 writes every trace
//   <out-dir>  where to write <prefix>-<n>.itf.json, n = 0, 1, …
//
// Compaction keeps the ITF valid (ADR-015) and changes no value the driver reads:
// - the `prev` and `mid` variables are dropped (the driver replays from the seed and never reads
//   them);
// - an integer within JSON's safe range is written as a number instead of {"#bigint": "…"},
//   which ADR-015 allows;
// - `#meta.description` and `#meta.timestamp` (the generation time) are dropped, and
//   `#meta.source` is cut to its path inside the repository, so that a regeneration with the same
//   seeds writes the same bytes on any machine, and no fixture records a local directory.
//
// Richness: the number of distinct actions that succeeded (`OutOk` or `OutReplay`), then the
// number of steps that succeeded, then the input order. A slice's random traces spend many steps
// on calls of sessions that have retired; the richest are kept.

"use strict";
const fs = require("fs");
const path = require("path");

const [keepArg, outDir, prefix, ...files] = process.argv.slice(2);
if (keepArg === undefined || outDir === undefined || prefix === undefined) {
  console.error("usage: compact.js <keep> <out-dir> <prefix> <trace.itf.json>...");
  process.exit(2);
}
const keep = Number(keepArg);

function conv(v) {
  if (Array.isArray(v)) return v.map(conv);
  if (v !== null && typeof v === "object") {
    if (Object.keys(v).length === 1 && "#bigint" in v) {
      const n = BigInt(v["#bigint"]);
      if (n <= BigInt(Number.MAX_SAFE_INTEGER) && n >= BigInt(Number.MIN_SAFE_INTEGER)) return Number(n);
      return v;
    }
    const o = {};
    for (const k of Object.keys(v)) o[k] = conv(v[k]);
    return o;
  }
  return v;
}

function lastAction(state) {
  const k = Object.keys(state).find((x) => x.endsWith("::last_action") || x === "last_action");
  return k === undefined ? undefined : state[k];
}

function richness(t) {
  const names = new Set();
  let ok = 0;
  for (const s of t.states) {
    const la = lastAction(s);
    if (la === undefined) continue;
    const tag = la.outcome && la.outcome.tag;
    if (tag === "OutOk" || tag === "OutReplay") {
      names.add(la.name);
      ok += 1;
    }
  }
  return [names.size, ok];
}

const traces = files.map((f, i) => {
  const t = JSON.parse(fs.readFileSync(f, "utf8"));
  const drop = (k) => ["prev", "mid"].some((v) => k.endsWith(`::${v}`) || k === v);
  t.vars = t.vars.filter((k) => !drop(k)).filter((k, j, a) => a.indexOf(k) === j);
  t.states = t.states.map((s) => {
    const o = {};
    for (const k of Object.keys(s)) if (!drop(k)) o[k] = conv(s[k]);
    return o;
  });
  if (t["#meta"]) {
    delete t["#meta"].description;
    delete t["#meta"].timestamp;
    if (typeof t["#meta"].source === "string") {
      t["#meta"].source = t["#meta"].source.replace(/^.*?\/(?=(crates|model)\/)/, "");
    }
  }
  return { t, i, score: richness(t) };
});

const chosen =
  keep > 0
    ? [...traces]
        .sort((a, b) => b.score[0] - a.score[0] || b.score[1] - a.score[1] || a.i - b.i)
        .slice(0, keep)
        .sort((a, b) => a.i - b.i)
    : traces;

fs.mkdirSync(outDir, { recursive: true });
chosen.forEach((c, n) => {
  const out = path.join(outDir, `${prefix}-${n}.itf.json`);
  fs.writeFileSync(out, JSON.stringify(c.t) + "\n");
  console.log(`${out}: ${c.t.states.length} states, ${c.score[0]} actions and ${c.score[1]} steps succeeded`);
});
