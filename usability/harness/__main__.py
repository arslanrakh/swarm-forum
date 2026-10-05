"""The command line of the usability test. `usability/run.sh` builds the server and calls this.

    run.sh list
    run.sh run --scenario s0-smoke --model claude:claude-sonnet-5-5 [--model codex:gpt-6.1-sol]
               [--arm g,nog] [--runs 1] [--out DIR] [--parallel N] [--force]
               [--schema-adapter auto|on|off] [--text-only] [--mcp-negotiation legacy|client]
               [--allow-unisolated] [--interview all|none|N]
    run.sh campaign [--model claude:claude-sonnet-5-5 ...] [--allow-unisolated]
               [--runs 10] [--out DIR] [--parallel N]        (every ready campaign scenario, both arms)
    run.sh probe --model claude:claude-sonnet-5-5 [--out DIR]  (the isolation probe)

`campaign` runs the campaign's four models (`CAMPAIGN_MODELS`: claude:claude-sonnet-5-5,
claude:claude-opus-5-5, codex:gpt-6.1-sol, and codex:gpt-6-astra) unless `--model` names others.

After a run is counted, the interview resumes each of its agent sessions once in its own client,
with no tool, and asks a fixed debrief (interview.py); `--interview` chooses the runs: all, none, or
the first N runs of each cell (model, scenario, and arm), 3 by default. `run`, `campaign`, and
`interview` first tidy what runs that stopped before their end left in the Claude client's
directory; a Ctrl-C stops the runs in progress (each finishes as `interrupted`, not counted) and
starts no other.

A Codex model runs only with `CODEX_HOME` naming a dedicated Codex home and after a probe of it
passed there (`probe` first); `--allow-unisolated` runs it anyway, and its runs are left out of every
count. `run` and `campaign` record what they were asked for in `<out>/manifest.json`, against
which the report's pass criteria look for missing cells.
    run.sh recount --out DIR                                   (the slip counter again, on saved runs)
    run.sh interview --out DIR [--interview all|none|N] [--again]  (interview saved runs not yet interviewed)
    run.sh report --out DIR [--to FILE]                        (the report from saved runs)

A model is `<vendor>:<model>`, with vendor `claude` (the `claude` CLI) or `codex` (the `codex`
CLI). Keys and logins come from the environment and the CLIs' own logins, never from the
repository. Output defaults to `usability/out/` (ignored by git).
"""

from __future__ import annotations

import argparse
import concurrent.futures
import json
import sys
import threading
import time
from pathlib import Path

from . import agents as A
from . import interview as IV
from . import isolation as ISO
from . import params as P
from . import proxy as X
from . import report as R
from . import runner as RUN
from . import scenario as SC
from . import server as S
from . import slips as SL

HERE = Path(__file__).resolve().parents[1]

# The campaign's models, two of each vendor: `campaign` runs these unless `--model` says otherwise,
# so that campaigns run at different times compare the same models.
CAMPAIGN_MODELS = [
    "claude:claude-sonnet-5-5",
    "claude:claude-opus-5-5",
    "codex:gpt-6.1-sol",
    "codex:gpt-6-astra",
]


def binary(args) -> Path:
    b = Path(args.binary).resolve() if args.binary else S.repo_root() / "target" / "debug" / "forum-server"
    if not b.exists():
        print("building forum-server with the usability feature ...", flush=True)
        b = S.build()
    return b


def parse_model(m: str) -> tuple[str, str]:
    vendor, _, model = m.partition(":")
    if vendor not in ("claude", "codex", "script") or not model:
        raise SystemExit(f"--model {m!r}: expected claude:<model>, codex:<model>, or script:<name>")
    if vendor == "claude" and "haiku" in model.lower():
        # As decided, no Haiku model takes part in this test (agents.CLAUDE_SMALL_MODEL_VARS).
        raise SystemExit(f"--model {m!r}: Haiku models are not used in this test")
    return vendor, model


def run_dir(out: Path, sid: str, vendor: str, model: str, arm: str, k: int) -> Path:
    return out / sid / f"{vendor}-{model}" / arm / f"run-{k:02d}"


# The runs in progress, and whether the harness was interrupted: a Ctrl-C reaches only the main
# thread, while the runs go on in the pool's threads (`cmd_run`), so the main thread stops them.
ACTIVE: set = set()
ACTIVE_LOCK = threading.Lock()
STOP = threading.Event()


def one(job: tuple) -> dict:
    scen, vendor, model, arm, d, b, opts = job
    tag = f"{scen['id']} {vendor}:{model} {arm} {d.name}"
    if STOP.is_set():
        return {"status": "not started: the harness was interrupted"}
    print(f"[{time.strftime('%H:%M:%S')}] start {tag}", flush=True)
    opts = dict(opts, max_minutes=scen.get("max_minutes", opts.get("max_minutes", 60.0)))
    r = RUN.Run(scen, vendor, model, arm, d, b, log=lambda m: print(f"  {tag}: {m}", flush=True), **opts)
    with ACTIVE_LOCK:
        ACTIVE.add(r)
    if STOP.is_set():
        r.interrupt()
    try:
        meta = r.execute()
    except Exception as e:  # noqa: BLE001 - one broken run must not stop the others
        meta = dict(r.meta, status="error", error=repr(e))
        d.mkdir(parents=True, exist_ok=True)
        (d / "run.json").write_text(json.dumps(meta, indent=1))
    finally:
        with ACTIVE_LOCK:
            ACTIVE.discard(r)
    print(f"[{time.strftime('%H:%M:%S')}] end {tag}: {meta['status']}, slips {meta.get('slips_total')}", flush=True)
    return meta


def interrupt_runs() -> None:
    """After a Ctrl-C: no run starts any more, and each run in progress stops and finishes, its
    client's sessions tidied (runner.Run.interrupt)."""
    STOP.set()
    with ACTIVE_LOCK:
        runs = list(ACTIVE)
    for r in runs:
        r.interrupt()


def codex_gate(out: Path, model: str) -> str | None:
    """Why a Codex model may not run yet, or None: it needs a dedicated `CODEX_HOME` and a probe of
    it that passed with that home (isolation.py)."""
    if not ISO.codex_home_dedicated():
        return (
            "CODEX_HOME must name a dedicated Codex home, logged in once and holding no AGENTS.md, skills, or "
            "roles: the operator's own ~/.codex adds them to every agent's context"
        )
    p = out / "probe" / "probe.json"
    results = json.loads(p.read_text()) if p.exists() else {}
    r = results.get(f"codex:{model}")
    if not r:
        return f"no probe of codex:{model} in {p}: run `usability/run.sh probe --model codex:{model}` first"
    if not r.get("passed"):
        return f"the probe of codex:{model} failed: {r.get('problems')}"
    if r.get("codex_home") != str(ISO.codex_home()):
        return f"the probe of codex:{model} ran with another CODEX_HOME ({r.get('codex_home')})"
    return None


def write_manifest(out: Path, models: list[str], scenarios: list[str], arms: list[str], runs: int) -> None:
    """What the campaign asked for, merged over invocations: the report's pass criteria read it."""
    p = out / "manifest.json"
    m = json.loads(p.read_text()) if p.exists() else {}
    real = [x for x in models if not x.startswith("script:")]
    if not real:
        return
    m["models"] = sorted(set(m.get("models", [])) | set(real))
    m["scenarios"] = sorted(set(m.get("scenarios", [])) | set(scenarios))
    m["arms"] = sorted(set(m.get("arms", [])) | set(arms), key=lambda a: (a != "g", a))
    m["runs"] = max(int(m.get("runs", 0)), runs)
    m["updated"] = time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())
    out.mkdir(parents=True, exist_ok=True)
    p.write_text(json.dumps(m, indent=1) + "\n")


def cmd_run(args, scenarios: list[str]) -> int:
    out = Path(args.out)
    policy = args.interview
    # What runs that stopped before their end (a crash, a kill) left in the Claude client's directory.
    IV.tidy_orphans(out)
    b = binary(args)
    arms = args.arm.split(",")
    write_manifest(out, args.model, [SC.load(s)["id"] for s in scenarios], arms, args.runs)
    blocked: dict[str, str] = {}
    unisolated: dict[str, str] = {}
    for m in args.model:
        vendor, model = parse_model(m)
        if vendor == "codex" and not args.prelude_only:
            why = codex_gate(out, model)
            if why and not args.allow_unisolated:
                blocked[m] = why
                print(f"refuse {m}: {why} (--allow-unisolated runs it, and its runs are left out of the counts)")
            elif why:
                unisolated[m] = f"run without a passed isolation probe: {why}"
    jobs = []
    for sid in scenarios:
        scen = SC.load(sid)
        if SC.missing(scen) and not args.force:
            print(f"skip {sid}: needs {', '.join(SC.missing(scen))} (--force runs it)")
            continue
        for m in args.model:
            if m in blocked:
                continue
            vendor, model = parse_model(m)
            for arm in arms:
                for k in range(1, args.runs + 1):
                    d = run_dir(out, scen["id"], vendor, model, arm, k)
                    if (d / "run.json").exists() and not args.again:
                        print(f"exists, kept: {d}")
                        continue
                    opts = {
                        "schema_adapter": args.schema_adapter,
                        "text_only": args.text_only,
                        "mcp_negotiation": args.mcp_negotiation,
                        "max_minutes": args.max_minutes,
                        "prelude_only": args.prelude_only,
                        "isolation_problems": [unisolated[m]] if m in unisolated else [],
                        "interview": IV.chosen(k, policy, vendor) and not args.prelude_only,
                    }
                    jobs.append((scen, vendor, model, arm, d, b, opts))
    if not jobs:
        return 0
    STOP.clear()
    ex = concurrent.futures.ThreadPoolExecutor(max_workers=max(1, args.parallel))
    try:
        list(ex.map(one, jobs))
    except KeyboardInterrupt:
        print("interrupted: stopping the runs in progress and tidying their client sessions", flush=True)
        interrupt_runs()
        ex.shutdown(wait=True, cancel_futures=True)
        return 130
    ex.shutdown(wait=True)
    to = Path(args.to) if args.to else out / "report.md"
    to.write_text(R.render(out))
    print(f"report: {to}")
    return 0


def cmd_probe(args) -> int:
    out = Path(args.out) / "probe"
    b = binary(args)
    results = {}
    for m in args.model:
        vendor, model = parse_model(m)
        d = out / f"{vendor}-{model}"
        p = P.load(SC.scenarios_dir(), {"profile": "short"})
        srv = S.Server(b, d / "server", p, "g", {"probe": {"class": "probe"}})
        srv.start()
        px = X.Proxy(srv.url, d / "proxy.jsonl", dump_dir=d).start()
        try:
            px.register(
                srv.tokens["probe"],
                "probe",
                flatten_schemas=(vendor == "claude" and args.schema_adapter != "off") or args.schema_adapter == "on",
                text_only=args.text_only,
            )
            # A Claude client keeps its record of the probe's session, as in a run, so that the check
            # can read what it added from its login; the record is copied into the probe's
            # directory and the client's removed after (interview.tidy_run), or by a later start of
            # the harness if the probe is killed (interview.tidy_orphans).
            sess = A.make(
                vendor,
                "probe",
                model,
                px.url,
                srv.tokens["probe"],
                "You are a probe of a test harness.",
                d / "agents",
                mcp_negotiation=args.mcp_negotiation,
                persist=True,
            )
            if getattr(sess, "persist", False):
                IV.write_record(d / "agents", "probe", sess.vendor, str(sess.workdir), sess.session_id)
            try:
                results[m] = ISO.probe(sess)
                (d / "agents" / "probe.transcript.json").write_text(json.dumps(sess.transcript(), indent=1))
            finally:
                sess.close()
                kept = IV.tidy_run(d)
            if kept:
                results[m]["client_files"] = kept
        finally:
            px.stop()
            srv.stop()
        print(f"{m}: {'passed' if results[m]['passed'] else 'FAILED'}: {results[m]['problems']}")
    # Merged with earlier probes, so that each model's latest probe stays on record for `run`.
    path = out / "probe.json"
    merged = json.loads(path.read_text()) if path.exists() else {}
    merged.update(results)
    path.write_text(json.dumps(merged, indent=1))
    return 0 if all(r["passed"] for r in results.values()) else 1


def cmd_recount(args) -> int:
    out = Path(args.out)
    for meta_path in sorted(out.glob("**/run.json")):
        d = meta_path.parent
        meta = json.loads(meta_path.read_text())
        if meta.get("status") not in ("done", "timeout"):
            continue
        scen = SC.load(meta["scenario"])
        hist_path = d / "server" / "export.jsonl"
        history = [json.loads(x) for x in hist_path.read_text().splitlines() if x.strip()] if hist_path.exists() else []
        proxy_log = [json.loads(x) for x in (d / "proxy.jsonl").read_text().splitlines() if x.strip()]
        transcripts = {}
        for p in (d / "agents").glob("*.transcript.json"):
            aid = p.name.split(".")[0]
            transcripts[aid] = A.backfill_usage(json.loads(p.read_text()), d / "agents" / f"{aid}.raw.jsonl")
            p.write_text(json.dumps(transcripts[aid], indent=1))
        result = SL.count(scen, meta, history, proxy_log, transcripts)
        (d / "slips.json").write_text(json.dumps(result, indent=1))
        print(f"{d}: slips {result['total']}")
    return 0


def cmd_interview(args) -> int:
    """Interviews saved runs that the policy chooses and that have no interview yet (or all of them
    with --again): a run whose interview failed, or one run before the interview existed. Only a
    session its client kept can be resumed (a run keeps every Claude session; runs made before R3's
    fixes kept only those of runs chosen for the interview); any other is recorded as not resumed."""
    out = Path(args.out)
    policy = args.interview
    IV.tidy_orphans(out)
    for meta_path in sorted(out.glob("**/run.json")):
        d = meta_path.parent
        meta = json.loads(meta_path.read_text())
        if meta.get("status") not in ("done", "timeout") or not (d / "slips.json").exists():
            continue
        if not IV.chosen(IV.run_index(d), policy, meta.get("vendor", "")):
            continue
        if (d / "interview.json").exists() and not args.again:
            continue
        scen = SC.load(meta["scenario"])
        transcripts = {
            p.name.split(".")[0]: json.loads(p.read_text()) for p in sorted((d / "agents").glob("*.transcript.json"))
        }
        print(f"{d}: interviewing {len(transcripts)} sessions", flush=True)
        meta["interview"] = IV.interview_run(d, scen, meta, transcripts, log=lambda m: print(m, flush=True))
        meta_path.write_text(json.dumps(meta, indent=1))
    to = Path(args.to) if args.to else out / "report.md"
    to.write_text(R.render(out))
    print(f"report: {to}")
    return 0


def interview_policy(value: str) -> int | None:
    try:
        return IV.parse_policy(value)
    except ValueError as e:
        raise argparse.ArgumentTypeError(str(e)) from None


def main(argv: list[str]) -> int:
    ap = argparse.ArgumentParser(
        prog="usability/run.sh", description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    sub = ap.add_subparsers(dest="cmd", required=True)
    sub.add_parser("list")
    common = argparse.ArgumentParser(add_help=False)
    common.add_argument("--out", default=str(HERE / "out"))
    common.add_argument("--binary")
    common.add_argument("--schema-adapter", choices=["auto", "on", "off"], default="off")
    common.add_argument("--text-only", action="store_true")
    common.add_argument(
        "--mcp-negotiation",
        choices=list(A.MCP_NEGOTIATIONS),
        default="client",
        help="Claude agents: client leaves the version to the client (1.5); legacy forces MCP_PROTOCOL_NEGOTIATION=legacy",
    )
    for name in ("run", "campaign"):
        p = sub.add_parser(name, parents=[common])
        if name == "run":
            p.add_argument("--scenario", action="append", required=True)
            p.add_argument("--model", action="append", required=True)
        else:
            p.add_argument(
                "--model", action="append", help=f"default: the campaign's models, {', '.join(CAMPAIGN_MODELS)}"
            )
        p.add_argument("--arm", default="g,nog")
        p.add_argument("--runs", type=int, default=1 if name == "run" else 10)
        p.add_argument("--parallel", type=int, default=1)
        p.add_argument("--force", action="store_true")
        p.add_argument("--again", action="store_true", help="re-run runs that already exist")
        p.add_argument("--max-minutes", type=float, default=60.0)
        p.add_argument(
            "--prelude-only", action="store_true", help="play each scenario's prelude and stop: a dry run of the data"
        )
        p.add_argument("--to")
        p.add_argument(
            "--allow-unisolated",
            action="store_true",
            help="run a Codex model without a dedicated CODEX_HOME and a passed probe; its runs are not counted",
        )
        p.add_argument(
            "--interview",
            type=interview_policy,
            default="3",
            help="interview the agents after the count: all, none, or the first N runs of each cell (default 3)",
        )
    p = sub.add_parser("interview", parents=[common])
    p.add_argument(
        "--interview",
        type=interview_policy,
        default="3",
        help="all, none, or the first N runs of each cell (default 3)",
    )
    p.add_argument("--again", action="store_true", help="interview runs that already have an interview again")
    p.add_argument("--to")
    p = sub.add_parser("lab", parents=[common], help="seeded forums, three agents of three models each (lab.py)")
    p.add_argument("--seed", action="append", help="seed ids, comma-separated, or all (default)")
    p.add_argument("--mix", action="append", help="the models, comma-separated; agent i of seed j gets mix[(i+j) %% n]")
    p.add_argument("--rotate", type=int, default=0, help="shift the models' rotation across seeds")
    p.add_argument("--parallel", type=int, default=7)
    p.add_argument("--again", action="store_true", help="re-run seeds that already ran")
    p.add_argument("--prelude-only", action="store_true", help="play each seed's prelude and stop")
    p.add_argument("--interview", choices=["all", "none"], default="all")
    p.add_argument("--codex-effort", default="low")
    p = sub.add_parser("probe", parents=[common])
    p.add_argument("--model", action="append", required=True)
    sub.add_parser("recount", parents=[common])
    p = sub.add_parser("report", parents=[common])
    p.add_argument("--to")
    args = ap.parse_args(argv)
    if args.cmd == "list":
        for sid in SC.list_ids():
            s = SC.load(sid)
            ready = "needs " + ", ".join(SC.missing(s)) if SC.missing(s) else "ready"
            if s.get("campaign", True) is False:
                ready += " (not in the campaign)"
            print(f"{s['id']:28} {ready:28} {s['title']}")
        return 0
    if args.cmd == "run":
        return cmd_run(args, args.scenario)
    if args.cmd == "campaign":
        args.model = args.model or list(CAMPAIGN_MODELS)
        return cmd_run(args, SC.campaign_ids())
    if args.cmd == "probe":
        return cmd_probe(args)
    if args.cmd == "lab":
        from . import lab

        return lab.cmd_lab(args, binary(args))
    if args.cmd == "recount":
        return cmd_recount(args)
    if args.cmd == "interview":
        return cmd_interview(args)
    if args.cmd == "report":
        to = Path(args.to) if args.to else Path(args.out) / "report.md"
        to.write_text(R.render(Path(args.out)))
        print(f"report: {to}")
        return 0
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
