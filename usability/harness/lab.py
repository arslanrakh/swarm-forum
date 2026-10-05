"""The usability lab: seeded forums at several stages of their life, three agents of three models each.

The lab is the quick loop beside the WP-U campaign: no arms, no slip criteria, every session
interviewed, and a digest per run that an analyst reads end to end. Each seed (`seeds.py`) runs on its
own server; its three agents get three different models from `--mix`, rotated across seeds so that
every model meets every kind of task. Codex agents get the forum's tools as direct functions and the
server's instructions before their role (`runner.CODEX_INSTRUCTIONS`), as a future system prompt
would carry them, since Codex shows a model no MCP server's instructions.

    usability/run.sh lab --out DIR [--seed l1,l3] [--mix codex:gpt-6.1-sol,codex:gpt-6-astra,claude:claude-sonnet-5-5]

Each run directory holds what `runner.Run` writes, plus `digest.md` (every turn, call, response,
reply, and interview answer) and `stats.json`; `DIR/summary.json` and `DIR/summary.md` sum them up.
"""

from __future__ import annotations

import concurrent.futures
import json
import time
from pathlib import Path

from . import interview as IV
from . import runner as RUN
from . import seeds as SD

DEFAULT_MIX = ["codex:gpt-6.1-sol", "codex:gpt-6-astra", "claude:claude-sonnet-5-5"]
RESPONSE_SHOWN = 3000


def models_for(seed: dict, j: int, mix: list[str]) -> dict:
    """Agent i of the j-th seed gets mix[(i + j) % len(mix)]."""
    return {a["id"]: mix[(i + j) % len(mix)] for i, a in enumerate(seed["agents"])}


def short(args: dict) -> str:
    a = {k: v for k, v in (args or {}).items() if k not in ("session", "op_id")}
    return json.dumps(a, ensure_ascii=False)


def stats(run_dir: Path) -> dict:
    meta = json.loads((run_dir / "run.json").read_text())
    out: dict = {"seed": meta["scenario"], "status": meta.get("status"), "agents": {}}
    iv = IV.load(run_dir) or {}
    by_agent = {s.get("agent"): s for s in iv.get("sessions", [])}
    for p in sorted((run_dir / "agents").glob("*.transcript.json")):
        tr = json.loads(p.read_text())
        aid = tr["agent"]
        calls = [c for t in tr["turns"] for c in t["calls"]]
        usage = {"input": 0, "output": 0}
        for t in tr["turns"]:
            u = t.get("usage") or {}
            i = int(u.get("input_all_tokens") or u.get("input_tokens") or 0)
            o = int(u.get("output_tokens") or 0)
            if tr["vendor"] == "codex":
                # A resumed Codex thread reports the thread's running totals at each turn: keep the last.
                usage["input"], usage["output"] = max(usage["input"], i), max(usage["output"], o)
            else:
                usage["input"] += i
                usage["output"] += o
        out["agents"][aid] = {
            "model": f"{tr['vendor']}:{tr['model']}",
            "turns": len(tr["turns"]),
            "calls": len(calls),
            "refused": sum(1 for c in calls if c.get("refused")),
            "response_bytes": sum(len(c.get("text") or "") for c in calls),
            "tools": sorted({f"{c['tool']} {(c.get('args') or {}).get('action', '')}".strip() for c in calls}),
            "tokens": usage,
            "ended_by": [t.get("ended_by") for t in tr["turns"]],
            "interviewed": (by_agent.get(aid) or {}).get("status"),
        }
    return out


def digest(run_dir: Path, seed: dict) -> str:
    meta = json.loads((run_dir / "run.json").read_text())
    iv = IV.load(run_dir) or {}
    by_agent = {s.get("agent"): s for s in iv.get("sessions", [])}
    roles = {a["id"]: a for a in seed["agents"]}
    st = stats(run_dir)
    lines = [
        f"# {seed['id']}: {seed['title']}",
        "",
        f"Stage: {seed.get('stage')}. {seed.get('about', '')}",
        "",
        "Intended flow (for the reader; the agents never saw it):",
        "",
        *[f"- {x}" for x in seed.get("intended", [])],
        "",
        f"Status: {meta.get('status')}; prelude {len(seed.get('prelude', []))} steps; the forum's clock started "
        f"{meta.get('clock_lag_ms', 0) // 86_400_000} days back. Protocol {meta.get('protocol_version')}.",
        "",
    ]
    for p in sorted((run_dir / "agents").glob("*.transcript.json")):
        tr = json.loads(p.read_text())
        aid = tr["agent"]
        a = st["agents"].get(aid, {})
        lines += [
            f"## Agent {aid}: {tr['vendor']}:{tr['model']}",
            "",
            f"Role: {roles.get(aid, {}).get('role', '')}",
            "",
            f"Turns {a.get('turns')}, forum calls {a.get('calls')}, refused {a.get('refused')}, response text "
            f"{a.get('response_bytes')} bytes, tokens in {a.get('tokens', {}).get('input')} out "
            f"{a.get('tokens', {}).get('output')}; turns ended by {a.get('ended_by')}.",
            "",
        ]
        for t in tr["turns"]:
            lines += [f"### Turn {t['n']} (ended by {t.get('ended_by')})", "", "Prompt:", "", "> " +
                      (t.get("prompt") or "").replace("\n", "\n> "), ""]
            for c in t["calls"]:
                text = c.get("text") or ""
                flag = " REFUSED" if c.get("refused") else (" CLIENT-ERROR" if c.get("is_error") else "")
                lines += [f"**[{c['i']}] {c['tool']} {(c.get('args') or {}).get('action', '')}**{flag} "
                          f"`{short(c.get('args'))}` -> {len(text)} bytes", "", "```",
                          text[:RESPONSE_SHOWN] + ("\n[... cut]" if len(text) > RESPONSE_SHOWN else ""), "```", ""]
            lines += ["Reply:", "", "> " + (t.get("text") or "(none)").replace("\n", "\n> "), ""]
        s = by_agent.get(aid)
        if s:
            lines += ["### Interview", ""]
            answers = s.get("answers") or {}
            if answers:
                for k in sorted(answers, key=lambda x: int(x) if str(x).isdigit() else 99):
                    lines += [f"{k}. {answers[k]}", ""]
            else:
                lines += [(s.get("reply") or s.get("why") or s.get("status") or "").strip(), ""]
    deliveries = meta.get("deliveries") or []
    if deliveries:
        lines += ["## Lines delivered", ""]
        for d in deliveries:
            lines += [f"- to {d['agent']} from {d['from']}: {d['text'][:400]}"]
    return "\n".join(lines) + "\n"


def run_seed(job: tuple) -> dict:
    seed, d, b, models, opts, log = job
    tag = seed["id"]
    log(f"[{time.strftime('%H:%M:%S')}] start {tag}: {models}")
    r = RUN.Run(
        seed,
        "lab",
        "mixed",
        "g",
        d,
        b,
        agent_models=models,
        lab={"codex_direct": True, "codex_instructions": True, "sim": True,
             "codex_effort": opts.get("codex_effort", "low")},
        interview=opts.get("interview", True),
        max_minutes=seed.get("max_minutes", 40),
        prelude_only=opts.get("prelude_only", False),
        mcp_negotiation="client",
        log=lambda m: log(f"  {tag}: {m}"),
    )
    try:
        meta = r.execute()
    except Exception as e:  # noqa: BLE001 - one broken seed must not stop the others
        meta = dict(r.meta, status="error", error=repr(e))
    if (d / "run.json").exists() and (d / "agents").exists():
        (d / "digest.md").write_text(digest(d, seed))
        (d / "stats.json").write_text(json.dumps(stats(d), indent=1))
    log(f"[{time.strftime('%H:%M:%S')}] end {tag}: {meta.get('status')}")
    return meta


def summary(out: Path) -> dict:
    rows = []
    for p in sorted(out.glob("*/stats.json")):
        rows.append(json.loads(p.read_text()))
    by_model: dict = {}
    for r in rows:
        for aid, a in r["agents"].items():
            m = by_model.setdefault(a["model"], {"sessions": 0, "calls": 0, "refused": 0, "bytes": 0, "in": 0, "out": 0})
            m["sessions"] += 1
            m["calls"] += a["calls"]
            m["refused"] += a["refused"]
            m["bytes"] += a["response_bytes"]
            m["in"] += a["tokens"]["input"]
            m["out"] += a["tokens"]["output"]
    doc = {"runs": rows, "by_model": by_model}
    (out / "summary.json").write_text(json.dumps(doc, indent=1))
    md = ["| seed | agent | model | calls | refused | response bytes | tokens in | ended by |", "|---|---|---|---|---|---|---|---|"]
    for r in rows:
        for aid, a in r["agents"].items():
            md.append(f"| {r['seed']} | {aid} | {a['model']} | {a['calls']} | {a['refused']} | {a['response_bytes']} | "
                      f"{a['tokens']['input']} | {','.join(str(x) for x in a['ended_by'])} |")
    md += ["", "| model | sessions | calls | refused | response bytes | tokens in | tokens out |", "|---|---|---|---|---|---|---|"]
    for m, v in sorted(by_model.items()):
        md.append(f"| {m} | {v['sessions']} | {v['calls']} | {v['refused']} | {v['bytes']} | {v['in']} | {v['out']} |")
    (out / "summary.md").write_text("\n".join(md) + "\n")
    return doc


def cmd_lab(args, binary: Path) -> int:
    out = Path(args.out)
    out.mkdir(parents=True, exist_ok=True)
    IV.tidy_orphans(out)
    ids = [x for spec in (args.seed or ["all"]) for x in spec.split(",")]
    if ids == ["all"]:
        ids = list(SD.SEEDS)
    mix = [x for spec in (args.mix or [",".join(DEFAULT_MIX)]) for x in spec.split(",")]
    for m in mix:
        vendor, _, model = m.partition(":")
        if vendor not in ("claude", "codex") or not model or "haiku" in model.lower():
            raise SystemExit(f"--mix {m!r}: expected claude:<model> or codex:<model>, never Haiku")
    jobs = []
    plan = {}
    for j, sid in enumerate(ids):
        seed = SD.load(sid)
        models = models_for(seed, j + args.rotate, mix)
        plan[seed["id"]] = models
        d = out / seed["id"]
        if (d / "run.json").exists() and not args.again:
            print(f"exists, kept: {d}")
            continue
        opts = {"interview": args.interview != "none" and not args.prelude_only, "prelude_only": args.prelude_only,
                "codex_effort": args.codex_effort}
        jobs.append((seed, d, binary, models, opts, lambda m: print(m, flush=True)))
    (out / "plan.json").write_text(json.dumps({"mix": mix, "models": plan, "started": time.time()}, indent=1))
    if jobs:
        with concurrent.futures.ThreadPoolExecutor(max_workers=max(1, args.parallel)) as ex:
            list(ex.map(run_seed, jobs))
    summary(out)
    print(f"summary: {out / 'summary.md'}")
    return 0

