"""The report (`usability/report.md`, which `docs/verification.md` links): every cell of the
design's WP-U row, from the runs under an output directory.

Per model, scenario, arm, and rule: the slips per run with a 95 % interval; the share of next calls
followed; calls and tokens per task (what the block costs); refusals. Then the misreadings the 1.4
texts address, per model in the arm with guidance, against the Wizard-of-Oz round, with a verdict
per text; and the pass criteria, against the cells the campaign asked for (`manifest.json`).

Intervals:
- a count per run (slips, refusals, calls, late entries, warnings): the exact (Garwood) Poisson
  interval for the mean, from the total over the runs. It does not collapse to a point when every
  run has the same count: no slip in 10 runs gives [0.00, 0.37]. It assumes Poisson counts, so it
  is narrower than the truth when runs differ more than that;
- an amount per run (tokens, cost): the mean with a 95 % percentile bootstrap over runs (2,000
  resamples, a fixed seed, so a report is reproducible), and no interval when every run is equal;
- a share (next calls followed, runs with a misreading): a Wilson interval.

Runs whose client loaded anything besides the forum (isolation problems) are listed and left out of
every count and of the pass criteria.

Last, the interviews (interview.py): what agents said, per model and arm, quoted and cut to a bounded
length; and per scenario and arm, each interviewed session's answers next to its slips, the
misreadings its calls show, its refused calls, how many of its calls the client kept in the record
the interview resumed, and anything the interview did not do as meant (a tool, another model, a
budget reached, the client's files). Where that record lost calls, the answers about what the
session did are marked unreliable (`unreliable`). An interview counts nothing, so the interviews of
runs with isolation problems are shown too, marked. Claude interviews' cost is in USD; Codex
reports no cost, so its interviews' tokens are shown instead.

The header names the protocol's version as `docs/protocol.md` states it (`server.protocol_version`),
and the versions the runs were made on when they differ.
"""

from __future__ import annotations

import json
import math
import random
import time
from pathlib import Path

from . import interview as IV
from . import scenario as SC
from . import server as S
from .slips import NOT_MEASURED, RULES

ARM_NAMES = {"g": "with guidance", "nog": "without guidance"}
ARMS = ("g", "nog")
MIN_RUNS = 10
MIN_MODELS = 3
MIN_VENDORS = 2
WOZ_RUNS = 19

# The Wizard-of-Oz round's counts for the 1.4 texts (report-ru.md, round 1: 19 runs; round 2
# re-ran 8 of them): (what, runs with it, out of, scenario it is limited to, how it is counted here).
WOZ = {
    "R1_principal": (
        "R1 (R7): an invented principal",
        3,
        WOZ_RUNS,
        None,
        "auto: a principal key no human of the scenario has",
    ),
    "R1_thread": (
        "R1: a thread's message logged as from_user",
        2,
        2,
        "s2-person-thread",
        "auto: from_user by an agent that serves a thread, not a human (S2 runs only)",
    ),
    "R1": ("R1: your human, a person in a thread, recording versus sending", 8, WOZ_RUNS, None, "manual"),
    "R5": (
        "R5: a key or an entry id where a node goes (5 calls)",
        4,
        WOZ_RUNS,
        None,
        "auto: at, on, target, or path holding a key or an eid",
    ),
    "R6": ("R6: an intent for a read; intent_done without a check (2 of 5 takeovers)", 5, WOZ_RUNS, None, "manual"),
    "R20": ("R20: a format learned by failing", 6, WOZ_RUNS, None, "auto: refusals whose hint is about a format"),
    "R21": (
        "R21: flat schemas, required arguments only in prose (complaints)",
        9,
        WOZ_RUNS,
        None,
        "auto: refusals for a missing or unknown argument",
    ),
    "R24": ("R24: the agreement warning read as a missing approval", 1, WOZ_RUNS, None, "manual"),
    "session": (
        "a guessed session id (not seen in the round: the simulator always showed the status line)",
        None,
        None,
        None,
        "auto: a call refused for naming a session the agent guessed",
    ),
}
AUTO = {"R1_principal", "R1_thread", "R5", "R20", "R21", "session"}


def load_runs(out: Path) -> list[dict]:
    runs = []
    for meta_path in sorted(out.glob("**/run.json")):
        meta = json.loads(meta_path.read_text())
        slips_path = meta_path.parent / "slips.json"
        meta["_dir"] = str(meta_path.parent)
        meta["_slips"] = json.loads(slips_path.read_text()) if slips_path.exists() else None
        meta["_isolation"] = isolation_problems(meta)
        meta["_counted"] = bool(meta["_slips"]) and not meta["_isolation"]
        meta["_interview"] = IV.load(meta_path.parent)
        runs.append(meta)
    return runs


def isolation_problems(meta: dict) -> list[str]:
    out = set()
    for a in (meta.get("agents") or {}).values():
        for p in (a.get("isolation") or {}).get("problems", []):
            out.add(p)
    for p in meta.get("isolation_problems") or []:
        out.add(p)
    return sorted(out)


def load_manifest(out: Path) -> dict | None:
    p = out / "manifest.json"
    return json.loads(p.read_text()) if p.exists() else None


# -- intervals -------------------------------------------------------------------------------------


def _poisson_cdf(k: int, lam: float) -> float:
    """P(X <= k) for X ~ Poisson(lam), summed in log space."""
    if k < 0:
        return 0.0
    if lam <= 0:
        return 1.0
    logs = [i * math.log(lam) - lam - math.lgamma(i + 1) for i in range(k + 1)]
    top = max(logs)
    return min(1.0, math.exp(top) * sum(math.exp(x - top) for x in logs))


def _solve(f, target: float, hi: float) -> float:
    """The λ in [0, hi] at which the decreasing `f` equals `target`, by bisection."""
    lo = 0.0
    for _ in range(200):
        mid = (lo + hi) / 2
        if f(mid) > target:
            lo = mid
        else:
            hi = mid
    return (lo + hi) / 2


def garwood(k: int, n: int, alpha: float = 0.05) -> tuple[float, float] | None:
    """The exact 95 % interval of a Poisson mean per run, from `k` events in `n` runs."""
    if n <= 0:
        return None
    hi_bound = k + 20 * math.sqrt(k + 1) + 20
    lower = 0.0 if k == 0 else _solve(lambda lam: _poisson_cdf(k - 1, lam), 1 - alpha / 2, hi_bound)
    upper = _solve(lambda lam: _poisson_cdf(k, lam), alpha / 2, hi_bound)
    return lower / n, upper / n


def bootstrap(values: list[float], seed: int = 20261003) -> tuple[float, float] | None:
    """A 95 % percentile bootstrap of the mean; None for fewer than 2 values or all equal."""
    if len(values) < 2 or len(set(values)) == 1:
        return None
    rng = random.Random(seed)
    n = len(values)
    means = sorted(sum(rng.choice(values) for _ in range(n)) / n for _ in range(2000))
    return means[int(0.025 * len(means))], means[int(0.975 * len(means)) - 1]


def wilson(k: int, n: int) -> tuple[float, float] | None:
    if n == 0:
        return None
    z = 1.96
    p = k / n
    d = 1 + z * z / n
    c = (p + z * z / (2 * n)) / d
    h = z * math.sqrt(p * (1 - p) / n + z * z / (4 * n * n)) / d
    return max(0.0, c - h), min(1.0, c + h)


def count_cell(values: list[float], digits: int = 2) -> str:
    """The mean of a count per run, with its Garwood interval."""
    if not values:
        return "no run"
    k = int(round(sum(values)))
    n = len(values)
    lo, hi = garwood(k, n)
    return f"{k / n:.{digits}f} [{lo:.{digits}f}, {hi:.{digits}f}] (n={n})"


def amount_cell(values: list[float], digits: int = 0) -> str:
    """The mean of an amount per run, with its bootstrap interval."""
    if not values:
        return "no run"
    m = sum(values) / len(values)
    ci = bootstrap(values)
    if ci is None:
        why = "one run" if len(values) == 1 else f"all {len(values)} runs equal"
        return f"{m:.{digits}f} (n={len(values)}, {why}: no interval)"
    return f"{m:.{digits}f} [{ci[0]:.{digits}f}, {ci[1]:.{digits}f}] (n={len(values)})"


# Kept for callers of the first version.
mean_cell = count_cell


# -- grouping --------------------------------------------------------------------------------------


def model_name(r: dict) -> str:
    return f"{r['vendor']}:{r['model']}"


def group(runs: list[dict]) -> dict:
    g: dict = {}
    for r in runs:
        g.setdefault(model_name(r), {}).setdefault(r["scenario"], {}).setdefault(r["arm"], []).append(r)
    return g


def counted_of(rs: list[dict]) -> list[dict]:
    return [r for r in rs if r.get("_counted")]


def campaign_ids() -> list[str]:
    return SC.campaign_ids()


def expected(runs: list[dict], manifest: dict | None) -> tuple[list[str], list[str], list[str]]:
    """The models, scenarios, and arms the campaign asked for: the manifest's, or else the models
    that ran, every campaign scenario, and both arms."""
    if manifest:
        models = list(manifest.get("models") or [])
        scenarios = [s for s in manifest.get("scenarios") or [] if s in campaign_ids()] or campaign_ids()
        arms = [a for a in manifest.get("arms") or ARMS]
    else:
        models = sorted({model_name(r) for r in runs if r["vendor"] != "script"})
        scenarios = campaign_ids()
        arms = list(ARMS)
    return models, scenarios, arms


def per_scenario_means(scen: dict, arm: str, value) -> dict[str, float]:
    out = {}
    for sid, arms in scen.items():
        vals = [value(r["_slips"]) for r in counted_of(arms.get(arm, []))]
        vals = [v for v in vals if v is not None]
        if vals:
            out[sid] = sum(vals) / len(vals)
    return out


def compare(scen: dict, value, scenarios: list[str]) -> tuple[float | None, float | None, list[str], list[str]]:
    """Sums of per-scenario means in each arm, over the scenarios both arms have counted runs of."""
    g = per_scenario_means(scen, "g", value)
    nog = per_scenario_means(scen, "nog", value)
    both = [s for s in scenarios if s in g and s in nog]
    left_out = [s for s in scenarios if s not in both]
    if not both:
        return None, None, both, left_out
    return sum(g[s] for s in both), sum(nog[s] for s in both), both, left_out


# -- the report ------------------------------------------------------------------------------------


def render(out: Path, title_note: str = "") -> str:
    runs = load_runs(out)
    manifest = load_manifest(out)
    counted = [r for r in runs if r["_counted"]]
    excluded = [r for r in runs if r.get("_slips") and r["_isolation"]]
    g = group(runs)
    lines: list[str] = []
    w = lines.append
    w("# Usability test with live agents: report")
    w("")
    w(
        f"Generated on {time.strftime('%Y-%m-%d %H:%M UTC', time.gmtime())} from {len(runs)} runs "
        f"({len(counted)} counted; {len(excluded)} left out for isolation problems) under `{out}`. "
        f"{version_words(runs)}; the arm without guidance is the server's test-only `usability` switch "
        "(1.2's instructions and `session` description, no `next_calls`). Design D.13.2, WP-U."
    )
    if title_note:
        w("")
        w(title_note)
    w("")
    w("## Runs")
    w("")
    w(
        "A run is counted when the slip counter read it and its client loaded nothing besides the forum. "
        "A run with isolation problems is listed here and left out of every count below."
    )
    w("")
    w("| Model | Scenario | Arm | Runs | Counted | Status | Schemas | Client view | Isolation problems |")
    w("|---|---|---|---|---|---|---|---|---|")
    for model, scen in sorted(g.items()):
        for sid, arms in sorted(scen.items()):
            for arm, rs in sorted(arms.items()):
                status: dict = {}
                for r in rs:
                    status[r["status"]] = status.get(r["status"], 0) + 1
                adapters = sorted({r.get("schema_adapter", "none") for r in rs})
                view = sorted({"text only" if r.get("text_only") else "as served" for r in rs})
                iso = sorted({p for r in rs for p in r["_isolation"]})
                st = ", ".join(f"{k} {v}" for k, v in sorted(status.items()))
                w(
                    f"| {model} | {sid} | {ARM_NAMES.get(arm, arm)} | {len(rs)} | {len(counted_of(rs))} | {st} | "
                    f"{', '.join(adapters)} | {', '.join(view)} | {'; '.join(iso) or 'none'} |"
                )
    w("")
    w("## Slips per run, by rule")
    w("")
    w(
        "Each cell is the mean number of slips per run, with its exact 95 % Poisson interval. The rules "
        "and the slip each one names are those of §20.9; `README.md` gives the counter's definition of "
        "each, and each run's `slips.json` the slips themselves and what was reported apart."
    )
    for model, scen in sorted(g.items()):
        for sid, arms in sorted(scen.items()):
            w("")
            w(f"### {model}, {sid}")
            w("")
            arm_keys = [a for a in ARMS if a in arms] + [a for a in arms if a not in ARMS]
            w("| Rule | Slip | " + " | ".join(ARM_NAMES.get(a, a) for a in arm_keys) + " |")
            w("|---|---|" + "---|" * len(arm_keys))
            for rule, what in list(RULES.items()) + [("total", "all rules")]:
                cells = []
                for a in arm_keys:
                    vals = []
                    for r in counted_of(arms[a]):
                        s = r["_slips"]
                        v = s["total"] if rule == "total" else s["rules"].get(rule)
                        if v is not None:
                            vals.append(float(v))
                    cells.append("not measured" if rule in NOT_MEASURED else count_cell(vals))
                w(f"| {rule} | {what} | " + " | ".join(cells) + " |")
    w("")
    w("## Next calls followed")
    w("")
    w(
        "A response's first `now` item is followed when the agent's next call has the same tool and "
        "action. Only the arm with guidance shows next calls."
    )
    w("")
    w("| Model | Scenario | Arm | First now items | Followed | Share [95 % interval] |")
    w("|---|---|---|---|---|---|")
    for model, scen in sorted(g.items()):
        for sid, arms in sorted(scen.items()):
            for arm, rs in sorted(arms.items()):
                cs = counted_of(rs)
                k = sum(r["_slips"].get("next_calls", {}).get("followed", 0) for r in cs)
                n = sum(r["_slips"].get("next_calls", {}).get("suggested_now", 0) for r in cs)
                ci = wilson(k, n)
                share = f"{k / n:.2f} [{ci[0]:.2f}, {ci[1]:.2f}]" if n else "no now item"
                w(f"| {model} | {sid} | {ARM_NAMES.get(arm, arm)} | {n} | {k} | {share} |")
    w("")
    w("## Calls and tokens per task")
    w("")
    w("A task is one run of a scenario: every agent session in it.")
    w("")
    w(
        "Input tokens count the whole prompt of every model request, cached or not; the cache reads are part "
        "of them. The cost is what the client reports (Claude Code does; Codex does not), and leaves out a "
        "Claude turn ended before its result, such as one ended at a scenario's fixed point, whose tokens "
        "come from its messages. A Codex turn ended that way reports no tokens."
    )
    w("")
    w(
        "| Model | Scenario | Arm | Forum calls per run | Input tokens | Output tokens | Cache reads | Cost per run (USD) |"
    )
    w("|---|---|---|---|---|---|---|---|")
    for model, scen in sorted(g.items()):
        for sid, arms in sorted(scen.items()):
            for arm, rs in sorted(arms.items()):
                ss = [r["_slips"] for r in counted_of(rs)]

                def u(k, ss=ss):
                    return [float(s["usage"].get(k) or 0) for s in ss]

                cost = amount_cell(u("cost_usd"), 4) if any(u("cost_usd")) else "not reported by the client"
                w(
                    f"| {model} | {sid} | {ARM_NAMES.get(arm, arm)} | {count_cell([float(s['calls']) for s in ss], 1)} | "
                    f"{amount_cell(u('input_all_tokens'))} | {amount_cell(u('output_tokens'))} | "
                    f"{amount_cell(u('cache_read_tokens'))} | {cost} |"
                )
    w("")
    w("## Refusals, late entries, and warnings")
    w("")
    w(
        "A refusal by a command of a core package the server does not have yet (its placeholder's hint, for a "
        "command `harness/packages.py` maps to such a package) is the server's gap, not a slip: it is counted "
        "apart, as not built, and left out of rule E and of the misreadings."
    )
    w("")
    w(
        "| Model | Scenario | Arm | Refusals per run | By code | Not built | Late entries per run | unsettled_requests | open_commitments |"
    )
    w("|---|---|---|---|---|---|---|---|---|")
    for model, scen in sorted(g.items()):
        for sid, arms in sorted(scen.items()):
            for arm, rs in sorted(arms.items()):
                ss = [r["_slips"] for r in counted_of(rs)]
                codes: dict = {}
                nb: dict = {}
                for s in ss:
                    for c, v in s["refusals"].items():
                        codes[c] = codes.get(c, 0) + v
                    for c, v in s.get("refusals_not_built", {}).items():
                        nb[c] = nb.get(c, 0) + v
                w(
                    f"| {model} | {sid} | {ARM_NAMES.get(arm, arm)} | {count_cell([float(s['refusals_total']) for s in ss])} | "
                    f"{', '.join(f'{c} {v}' for c, v in sorted(codes.items())) or 'none'} | "
                    f"{', '.join(f'{c} {v}' for c, v in sorted(nb.items())) or 'none'} | "
                    f"{count_cell([float(s['late_entries']) for s in ss])} | "
                    f"{count_cell([float(s['unsettled_requests_warnings']) for s in ss])} | "
                    f"{count_cell([float(s['open_commitments_warnings']) for s in ss])} |"
                )
    w("")
    misreadings(w, g)
    pass_criteria(w, runs, g, manifest)
    what_agents_said(w, runs)
    interviews_next_to_slips(w, runs)
    return "\n".join(lines) + "\n"


def version_words(runs: list[dict]) -> str:
    """The protocol's version as `docs/protocol.md` states it, and the versions the runs were made
    on when any differs (each run records the one stated when it was made; older runs record none)."""
    spec = S.protocol_version()
    words = f"The protocol is version {spec or 'unknown'} (`docs/protocol.md`)"
    made: dict[str, int] = {}
    for r in runs:
        if r.get("protocol_version"):
            made[r["protocol_version"]] = made.get(r["protocol_version"], 0) + 1
    if made and set(made) != {spec}:
        words += "; the runs were made on " + ", ".join(f"{v} ({plural(n, 'run')})" for v, n in sorted(made.items()))
    return words


def misreadings(w, g: dict) -> None:
    w("## Misreadings the 1.4 texts address")
    w("")
    w(
        "The texts R1, R5, R6, R20, R21, and R24 were accepted on condition that this test confirms them "
        "(design D.13.1, 1.4): each is measured per model in the arm with guidance, which serves 1.4's texts "
        "(the other arm serves 1.2's instructions, R1's own subject), against the Wizard-of-Oz round. A text "
        "*falls* for a model when the upper end of the 95 % Wilson interval of its share of runs is below "
        "the round's share; otherwise it does not fall, and goes back for a decision before the release. A "
        "text counted by hand needs a reader of the transcripts. R21 cannot be confirmed by a run whose "
        "schemas the harness flattened."
    )
    w("")
    w(
        "| Text | Wizard-of-Oz round | How it is counted here | Model | Live, with guidance: runs with it / runs | Share [95 %] | Per run [95 %] | Verdict |"
    )
    w("|---|---|---|---|---|---|---|---|")
    for key, (what, woz_k, woz_n, only, how) in WOZ.items():
        woz = f"{woz_k} of {woz_n} runs" if woz_k is not None else "not seen"
        if only:
            woz += f" ({only} only)"
        if key not in AUTO:
            w(f"| {what} | {woz} | {how} | all | manual | | | manual: a reader counts it |")
            continue
        for model, scen in sorted(g.items()):
            rs = [
                r for sid, arms in scen.items() if (only is None or sid == only) for r in counted_of(arms.get("g", []))
            ]
            if not rs:
                w(f"| {what} | {woz} | {how} | {model} | no counted run | | | not measured |")
                continue
            with_it = sum(1 for r in rs if r["_slips"]["misreadings"].get(key))
            calls = [float(r["_slips"]["misreadings"].get(key, 0)) for r in rs]
            lo, hi = wilson(with_it, len(rs))
            share = f"{with_it / len(rs):.2f} [{lo:.2f}, {hi:.2f}]"
            if key == "R21" and all(r.get("schema_adapter") == "flatten" for r in rs):
                verdict = "cannot be confirmed: schemas flattened for this client"
            elif woz_k is None:
                verdict = "no round count to compare with"
            elif hi < woz_k / woz_n:
                verdict = "falls"
            else:
                verdict = "does not fall: goes back for a decision"
            w(
                f"| {what} | {woz} | {how} | {model} | {with_it} / {len(rs)} | {share} | {count_cell(calls)} | {verdict} |"
            )
    w("")


def pass_criteria(w, runs: list[dict], g: dict, manifest: dict | None) -> None:
    w("## Pass criteria")
    w("")
    models, scenarios, arms = expected(runs, manifest)
    src = "`manifest.json`" if manifest else "the models that ran, every campaign scenario, and both arms"
    w(f"The cells asked for come from {src}: {len(models)} models × {len(scenarios)} scenarios × {len(arms)} arms.")
    w("")
    missing, thin = [], []
    for model in models:
        for sid in scenarios:
            for arm in arms:
                n = len(counted_of(g.get(model, {}).get(sid, {}).get(arm, [])))
                cell = f"{model} {sid} {ARM_NAMES.get(arm, arm)}"
                if n == 0:
                    missing.append(cell)
                elif n < MIN_RUNS:
                    thin.append(f"{cell} (n={n})")
    for sid in scenarios:
        for extra, why in (SC.load(sid).get("extra_arms") or {}).items():
            missing.append(f"every model {sid} {extra} ({why})")
    ok = not missing and not thin
    w(f"- Every cell is reported, with at least {MIN_RUNS} counted runs: {'yes' if ok else 'no'}.")
    if missing:
        w(f"  - Missing ({len(missing)}): {'; '.join(missing)}.")
    if thin:
        w(f"  - Fewer than {MIN_RUNS} counted runs ({len(thin)}): {'; '.join(thin)}.")
    with_runs = sorted({model_name(r) for r in runs if r["_counted"] and r["scenario"] in scenarios})
    vendors = sorted({m.split(":", 1)[0] for m in with_runs})
    w(
        f"- At least {MIN_MODELS} models from at least {MIN_VENDORS} vendors: "
        f"{'yes' if len(with_runs) >= MIN_MODELS and len(vendors) >= MIN_VENDORS else 'no'} "
        f"({len(with_runs)} models with counted campaign runs: {', '.join(with_runs) or 'none'}; vendors: {', '.join(vendors) or 'none'})."
    )
    w(
        "- Fewer slips in total with guidance, per model: the sum over scenarios of the mean slips per run, "
        "in the scenarios both arms have counted runs of."
    )
    for model in models or sorted(g):
        tg, tn, both, left = compare(g.get(model, {}), lambda s: s["total"], scenarios)
        note = f"; left out: {', '.join(left)}" if left else ""
        if tg is None:
            verdict = "not comparable: no scenario has counted runs in both arms"
        elif tg < tn:
            verdict = f"yes ({tg:.2f} against {tn:.2f} over {len(both)} scenarios{note})"
        elif tg == tn:
            verdict = f"no, equal ({tg:.2f} over {len(both)} scenarios{note}): goes back for a decision"
        else:
            verdict = f"no ({tg:.2f} against {tn:.2f} over {len(both)} scenarios{note}): goes back for a decision"
        w(f"  - {model}: {verdict}.")
    # NC-10: a rule whose slip is not less frequent with guidance for any model goes back for a
    # decision, with the year simulation's result for it.
    no_gain, never = [], []
    for rule in RULES:
        if rule in NOT_MEASURED:
            continue
        gains, seen = [], False
        for model in models or sorted(g):
            scen = g.get(model, {})
            tg, tn, both, _ = compare(scen, lambda s, rule=rule: s["rules"].get(rule), scenarios)
            if tg is None:
                continue
            seen = seen or tg > 0 or tn > 0
            gains.append(tg < tn)
        if gains and not any(gains):
            (no_gain if seen else never).append(rule)
    w(
        f"- Rules whose slip occurred but is not less frequent with guidance for any model (NC-10, back for "
        f"a decision with the year simulation's result): {', '.join(no_gain) if no_gain else 'none'}."
    )
    w(
        f"- Rules whose slip occurred in neither arm for any model compared (NC-10 asks the same of them): "
        f"{', '.join(never) if never else 'none'}."
    )
    excluded = [r for r in runs if r.get("_slips") and r["_isolation"]]
    if excluded:
        w(f"- {len(excluded)} runs were left out of every count for isolation problems (see Runs).")
    flat = [r for r in runs if r["_counted"] and r.get("schema_adapter") == "flatten"]
    if flat:
        w(
            f"- {len(flat)} counted runs had §20.2's input schemas flattened by the harness for a client "
            "that refuses a top-level `oneOf`: they cannot confirm R21."
        )
    w("")


# -- the interviews --------------------------------------------------------------------------------

# How much of an answer the report quotes: in the summary per model and arm, and per session.
QUOTE_SUMMARY = 240
QUOTE_SESSION = 700
QUOTES_PER_QUESTION = 3


def plural(n: int, word: str) -> str:
    return f"{n} {word}{'' if n == 1 else 's'}"


def interviewed(runs: list[dict]) -> list[dict]:
    return [r for r in runs if r.get("_interview")]


def question_labels(arm: str) -> list[tuple[str, str]]:
    """(id, label) of the questions in the order asked; the session cut mid-task's last."""
    return [(q["id"], q["label"]) for q in IV.questions_for(arm, True)]


CURRENT_TEXT = {q["id"]: q["text"] for q in IV.QUESTIONS}


def asked_as_now(sess: dict, qid: str) -> bool:
    """Whether the session was asked question `qid` in the words asked now (an interview made
    with earlier words keeps them in `interview.json`, and is quoted only per session)."""
    asked = {q["id"]: q.get("text") for q in sess.get("questions") or []}
    return asked.get(qid) == CURRENT_TEXT.get(qid)


# How each client is resumed for the interview (interview.py, agents.*_interview_command).
RESUMED = {
    "claude": "Claude sessions were resumed (`claude -p --resume`) with no MCP server and no tool",
    "codex": (
        "Codex threads were forked (`codex exec fork --ephemeral`) with no MCP server and the tools as far "
        "off as the CLI allows: code mode fails closed, while `request_user_input` and, with the operator's own "
        "`CODEX_HOME`, the multi-agent tools stay offered; a tool the model calls is recorded"
    ),
}


def usage_words(u: dict) -> str:
    return (
        f"{int(u.get('input_all_tokens') or 0):,} input tokens ({int(u.get('cache_read_tokens') or 0):,} cached), "
        f"{int(u.get('output_tokens') or 0):,} output"
    )


def interview_cost_words(rs: list[dict]) -> str:
    """The interviews' cost: Claude's in USD, as the client reports it; Codex's tokens, its cost
    unknown (the client reports none)."""
    vendors = {r["vendor"] for r in rs}
    parts = []
    if vendors - {"codex"}:
        parts.append(f"interview cost {sum(float(r['_interview'].get('cost_usd') or 0) for r in rs):.4f} USD")
    if "codex" in vendors:
        total: dict = {}
        for r in rs:
            if r["vendor"] != "codex":
                continue
            for s in r["_interview"].get("sessions", []):
                for k, v in (s.get("usage") or {}).items():
                    if isinstance(v, (int, float)):
                        total[k] = total.get(k, 0) + v
        parts.append(f"interview tokens {usage_words(total)}; cost unknown (Codex reports none)")
    return "; ".join(parts)


def answers_of(sess: dict) -> dict[str, str]:
    """The answers by question id; an unnumbered reply as a whole, under `reply`."""
    if sess.get("answers"):
        return sess["answers"]
    if sess.get("status") == "answered" and sess.get("text"):
        return {"reply": sess["text"]}
    return {}


def where(r: dict, sess: dict) -> str:
    return f"{r['scenario']} {Path(r['_dir']).name} {sess['agent']}"


def unreliable(sess: dict, qid: str) -> str | None:
    """Why an answer about what the session did (IV.ABOUT_WHAT_IT_DID, or an unnumbered reply,
    which holds those answers too) cannot be taken at its word: the client's record of the session,
    all the resumed model knows of it, lost some of the forum calls it made. None otherwise."""
    if qid not in IV.ABOUT_WHAT_IT_DID and qid != "reply":
        return None
    kept, calls = sess.get("calls_kept"), int(sess.get("forum_calls") or 0)
    if isinstance(kept, int) and kept < calls:
        return f"unreliable: the client's record kept {kept} of its {plural(calls, 'forum call')}"
    return None


def what_agents_said(w, runs: list[dict]) -> None:
    w("## What agents said")
    w("")
    ivs = interviewed(runs)
    if not ivs:
        w("No run was interviewed (`--interview none`, or runs made before the interview existed).")
        w("")
        return
    sessions = [s for r in ivs for s in r["_interview"].get("sessions", [])]
    status: dict = {}
    for x in sessions:
        status[x["status"]] = status.get(x["status"], 0) + 1
    vendors = sorted({r["vendor"] for r in ivs})
    how = "; ".join(RESUMED.get(v, f"{v} sessions were resumed in their own client") for v in vendors)
    w(
        f"After a run was counted, each of its agent sessions was asked the questions below in one turn, "
        f"in its own client: {how}. `interview.json` in each run directory holds the questions as asked, "
        f"the whole answers, and the tokens. {plural(len(ivs), 'run')}, {plural(len(sessions), 'session')}: "
        f"{', '.join(f'{v} {k}' for k, v in sorted(status.items()))}; {interview_cost_words(ivs)}. The answers "
        f"are quoted as given, cut to {QUOTE_SUMMARY} characters here and {QUOTE_SESSION} in the next section; "
        "nothing summarises them. A run with isolation problems is marked, since an interview counts nothing."
    )
    w("")
    w(
        "The questions, the same in both arms, after a short preamble (the task is over, no tools now, at most "
        "three sentences each, say so where a question does not apply):"
    )
    w("")
    for q in IV.QUESTIONS:
        when = {"always": "", "cut": " (a session the harness ended mid-task)"}[q["when"]]
        w(f"- *{q['label']}*{when}: {q['text']}")
    w("")
    w(
        "A session the harness ended mid-task may have lost its last calls from the client's own record "
        "of it, which is all the resumed model knows: the next section sets the calls the client kept "
        "against those the run made. A session stopped right after a forum call (a fixed point, the call "
        "limit) is told before the questions, from the proxy's log, how many calls it made and which. Where "
        "the client kept fewer calls than the session made, its answers about what it did (what it was "
        "doing, how it decided what to do next) are marked unreliable."
    )
    w("")
    by: dict = {}
    for r in ivs:
        by.setdefault((model_name(r), r["arm"]), []).append(r)
    for (model, arm), rs in sorted(by.items(), key=lambda x: (x[0][0], x[0][1] != "g", x[0][1])):
        ss = [(r, s) for r in sorted(rs, key=lambda r: (r["scenario"], r["_dir"])) for s in r["_interview"]["sessions"]]
        answered = [(r, s) for r, s in ss if s["status"] == "answered"]
        unisolated = sum(1 for r in rs if r["_isolation"])
        w(f"### {model}, {ARM_NAMES.get(arm, arm)}")
        w("")
        note = f"; {unisolated} of the runs not isolated (left out of the counts)" if unisolated else ""
        earlier = sum(1 for _r, s in answered if not all(asked_as_now(s, q["id"]) for q in s.get("questions") or []))
        if earlier:
            note += f"; {plural(earlier, 'session')} asked in earlier words, quoted only in the next section"
        w(
            f"{len(answered)} of {plural(len(ss), 'session')} answered, in {plural(len(rs), 'run')}; "
            f"{interview_cost_words(rs)}{note}."
        )
        w("")
        labels = question_labels(arm) + [("reply", "Unnumbered reply")]
        for qid, label in labels:
            got = [(r, s, answers_of(s).get(qid, "")) for r, s in answered if qid == "reply" or asked_as_now(s, qid)]
            got = [(r, s, a) for r, s, a in got if a.strip()]
            if not got:
                continue
            quoted = "; ".join(
                f"“{IV.cut(a, QUOTE_SUMMARY)}” ({'; '.join(x for x in (where(r, s), unreliable(s, qid)) if x)})"
                for r, s, a in got[:QUOTES_PER_QUESTION]
            )
            more = f"; {len(got) - QUOTES_PER_QUESTION} more below" if len(got) > QUOTES_PER_QUESTION else ""
            w(f"- **{label}** ({plural(len(got), 'answer')}{more}): {quoted}")
        w("")


def describe(d: dict, limit: int = 200) -> str:
    """One slip's or misreading's detail on one line, without the agent it is charged to."""
    parts = [str(d["what"])] if d.get("what") else []
    for k, v in d.items():
        if k in ("agent", "what", "due"):
            continue
        if isinstance(v, list):
            v = ", ".join(str(x) for x in v[:5]) + (" …" if len(v) > 5 else "")
        parts.append(f"{k} {v}")
    return IV.cut("; ".join(parts), limit)


def session_facts(r: dict, aid: str) -> dict:
    """The slips, the items reported apart, the misreadings, and the refused calls of one agent."""
    slips = r.get("_slips") or {}
    out: dict = {"slips": [], "apart": [], "misreadings": [], "refused": []}
    for rule, ds in (slips.get("details") or {}).items():
        out["slips"] += [f"rule {rule}: {describe(d)}" for d in ds if d.get("agent") == aid]
    for kind, ds in (slips.get("apart") or {}).items():
        out["apart"] += [f"{kind.replace('_', ' ')}: {describe(d)}" for d in ds if d.get("agent") == aid]
    for key, ds in (slips.get("misreading_details") or {}).items():
        out["misreadings"] += [f"{key}: {describe(d)}" for d in ds if d.get("agent") == aid]
    tp = Path(r["_dir"]) / "agents" / f"{aid}.transcript.json"
    if tp.exists():
        tr = json.loads(tp.read_text())
        for t in tr.get("turns", []):
            for c in t.get("calls", []):
                if c.get("refused"):
                    err = next((x for x in (c.get("text") or "").splitlines() if x.startswith("error: ")), "error")
                    act = (c.get("args") or {}).get("action", "")
                    out["refused"].append(f"call {c.get('i')} {c.get('tool')} {act}: {err[len('error: ') :]}".strip())
    return out


def interviews_next_to_slips(w, runs: list[dict]) -> None:
    ivs = interviewed(runs)
    if not ivs:
        return
    w("## Interviews next to the slips")
    w("")
    w(
        "Per scenario and arm, each interviewed session: what the slip counter charged to it, what it reported "
        "apart (what the harness's fixed points caused), the misreadings its calls show, its refused calls, and "
        f"then its answers, each cut to {QUOTE_SESSION} characters."
    )
    w("")
    by: dict = {}
    for r in ivs:
        by.setdefault((r["scenario"], r["arm"], model_name(r)), []).append(r)
    for (sid, arm, model), rs in sorted(by.items(), key=lambda x: (x[0][0], x[0][1] != "g", x[0][1], x[0][2])):
        w(f"### {sid}, {ARM_NAMES.get(arm, arm)}, {model}")
        w("")
        for r in sorted(rs, key=lambda r: r["_dir"]):
            iv = r["_interview"]
            for sess in iv.get("sessions", []):
                aid = sess["agent"]
                w(f"#### {Path(r['_dir']).name}, agent {aid}")
                w("")
                f = session_facts(r, aid)
                if sess.get("ended_mid_task"):
                    ended = f"ended by the harness mid-task ({sess['ended_by']})"
                elif sess.get("stopped_by"):
                    ended = f"stopped by the harness between its turns ({sess['stopped_by']})"
                else:
                    ended = "ended on its own"
                calls = int(sess.get("forum_calls") or 0)
                kept = sess.get("calls_kept")
                if isinstance(kept, int):
                    ended += f"; the client kept {kept} of the {plural(calls, 'forum call')} in its record"
                    if kept < calls:
                        ended += ", and the interview's model knew only those"
                w(f"- Session: {plural(calls, 'forum call')}, {len(f['refused'])} refused; {ended}.")
                if sess.get("note"):
                    w(f"- Told before the questions: “{sess['note']}”")
                if r["_isolation"]:
                    w(f"- Not isolated, left out of the counts: {'; '.join(r['_isolation'])}.")
                if not r.get("_slips"):
                    w("- Slips: the run was not counted.")
                else:
                    w(f"- Slips: {'; '.join(f['slips']) if f['slips'] else 'none'}.")
                if f["apart"]:
                    w(f"- Reported apart: {'; '.join(f['apart'])}.")
                w(f"- Misreadings: {'; '.join(f['misreadings']) if f['misreadings'] else 'none'}.")
                if f["refused"]:
                    w(f"- Refused calls: {'; '.join(IV.cut(x, 160) for x in f['refused'])}.")
                u = sess.get("usage") or {}
                if isinstance(sess.get("cost_usd"), (int, float)):
                    cost = f", {float(sess['cost_usd']):.4f} USD"
                elif r["vendor"] == "codex":
                    cost = ", cost unknown"
                else:
                    cost = ""
                tokens = (
                    f"{int(u.get('input_all_tokens') or 0):,} input tokens, {int(u.get('output_tokens') or 0):,} output"
                    if u
                    else sess.get("usage_note") or "no tokens reported"
                )
                if sess["status"] == "answered":
                    w(f"- Interview: answered ({tokens}{cost}).")
                else:
                    w(f"- Interview: {sess['status'].replace('_', ' ')}: {sess.get('why') or 'no reason given'}.")
                if sess.get("over_budget"):
                    w(f"- Interview went over its budget of {float(sess.get('budget_usd') or 0):.2f} USD.")
                for p in sess.get("tools_problems") or []:
                    w(f"- Interview ran with a tool: {p}.")
                for p in sess.get("model_problems") or []:
                    w(f"- Interview ran with another model: {p}.")
                files = sess.get("client_files") or {}
                for p in ([files["refused"]] if files.get("refused") else []) + list(files.get("problems") or []):
                    w(f"- The client's files: {p}.")
                w("")
                answers = answers_of(sess)
                if not answers:
                    continue
                labels = {q["id"]: q["label"] for q in sess.get("questions", [])}
                labels["reply"] = "Unnumbered reply"
                shown = [(labels.get(k, k), unreliable(sess, k), a) for k, a in answers.items() if a.strip()]
                for i, (label, why, a) in enumerate(shown):
                    mark = f"({why[0].upper()}{why[1:]}.) " if why else ""
                    w(f"> **{label}.** {mark}{IV.cut(a, QUOTE_SESSION)}")
                    if i + 1 < len(shown):
                        w(">")
                w("")
