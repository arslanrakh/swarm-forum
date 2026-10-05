//! `swarm-sim`: the load driver of the scale gates (design D.12.8, D.15; WP-F4).
//!
//! ```text
//! swarm-sim gate <g1|g2|g4|g5|g6|g7|g8|g9|g10|g12> [--scale smoke|full] --bin <forum-server>
//!           [--work <dir>] [--results <dir>] [--stubs count|skip|require] [--seed <n>]
//!           [--agents <n>] [--days <d>] [--seconds <s>] [--docs <n>] [--reuse <result.json>]
//!           [--keep 1]
//! swarm-sim verdict --gate <G-n> --title <text> --scale smoke|full --status pass|fail|not-yet
//!           --note <text> [--results <dir>] [--work <dir>] [--bin <forum-server>] [--log <file>]
//! swarm-sim machine [--dir <dir>] [--bin <forum-server>]
//! swarm-sim run --url <mcp url> --token <token>... [--agents <n>] [--profile low|base|high]
//!           [--mix base|heavy] [--seconds <s>] [--clock-scale <k>] [--backlog <n>]
//!           [--stubs count|skip|require] [--out <file>]
//! ```
//!
//! `gate` starts the `forum-server` binary (built with its `bench` feature) on a database of its
//! own, runs the gate at its scale, prints the numbers against D.12.8's limits, and writes each
//! verdict as `<results>/<gate>-<scale>.json` with the machine it ran on; it exits 0 on pass, 1 on
//! fail, 3 when nothing could be checked yet, and 2 on an error. `g1` and `g2` run the same
//! scenario and write both verdicts; `--reuse` prints a verdict written before instead of running
//! again; the gate's databases are removed afterwards unless `--keep 1`. `verdict` records the
//! result of a gate that a script checked itself (G-3, G-11), with the per-tool numbers of its
//! test's output (`--log`). `machine` prints the machine, its durable fsync, and, given the server
//! binary, the commit the server's SQLite really pays. `run` drives a load against a server
//! already running and writes the recorded calls as JSON.

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use swarm_sim::gates::{self, Options, Scale, Verdict};
use swarm_sim::mix;
use swarm_sim::record::StubPolicy;
use swarm_sim::run::{Phase, Spec, run};

const USAGE: &str = "usage:
  swarm-sim gate <g1|g2|g4|g5|g6|g7|g8|g9|g10|g12> [--scale smoke|full] --bin <forum-server>
            [--work <dir>] [--results <dir>] [--stubs count|skip|require] [--seed <n>]
            [--agents <n>] [--days <d>] [--seconds <s>] [--docs <n>] [--reuse <result.json>]
            [--keep 1]
  swarm-sim verdict --gate <G-n> --title <text> --scale smoke|full --status pass|fail|not-yet
            --note <text> [--results <dir>] [--work <dir>] [--bin <forum-server>] [--log <file>]
  swarm-sim machine [--dir <dir>] [--bin <forum-server>]
  swarm-sim run --url <mcp url> --token <token>... [--agents <n>] [--profile low|base|high]
            [--mix base|heavy] [--seconds <s>] [--clock-scale <k>] [--backlog <n>]
            [--stubs count|skip|require] [--out <file>]";

/// Flags with values; a flag may repeat (`--token`).
struct Args(Vec<(String, String)>);

impl Args {
    fn parse(args: &[String]) -> Result<Args, String> {
        let mut out = Vec::new();
        let mut it = args.iter();
        while let Some(a) = it.next() {
            let name = a
                .strip_prefix("--")
                .ok_or_else(|| format!("unexpected argument {a:?}"))?;
            let v = it.next().ok_or_else(|| format!("--{name} needs a value"))?;
            out.push((name.to_string(), v.clone()));
        }
        Ok(Args(out))
    }

    fn get(&self, name: &str) -> Option<&str> {
        self.0
            .iter()
            .rev()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }

    fn all(&self, name: &str) -> Vec<String> {
        self.0
            .iter()
            .filter(|(k, _)| k == name)
            .map(|(_, v)| v.clone())
            .collect()
    }

    fn num<T: std::str::FromStr>(&self, name: &str) -> Result<Option<T>, String> {
        self.get(name)
            .map(|v| {
                v.parse::<T>()
                    .map_err(|_| format!("--{name} {v:?} is not a number"))
            })
            .transpose()
    }

    fn only(&self, allowed: &[&str]) -> Result<(), String> {
        match self.0.iter().find(|(k, _)| !allowed.contains(&k.as_str())) {
            Some((k, _)) => Err(format!("unknown option --{k}")),
            None => Ok(()),
        }
    }
}

fn scale(a: &Args) -> Result<Scale, String> {
    match a.get("scale").unwrap_or("smoke") {
        "smoke" => Ok(Scale::Smoke),
        "full" => Ok(Scale::Full),
        s => Err(format!("--scale {s:?}: smoke or full")),
    }
}

fn stubs(a: &Args) -> Result<StubPolicy, String> {
    let s = a.get("stubs").unwrap_or("count");
    StubPolicy::parse(s).ok_or_else(|| format!("--stubs {s:?}: count, skip, or require"))
}

fn exit_of(v: &Verdict) -> ExitCode {
    match v.pass {
        Some(true) => ExitCode::SUCCESS,
        Some(false) => ExitCode::from(1),
        None => ExitCode::from(3),
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let rt = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("swarm-sim: {e}");
            return ExitCode::from(2);
        }
    };
    let result = match args.first().map(String::as_str) {
        Some("gate") => rt.block_on(gate(&args[1..])),
        Some("verdict") => verdict(&args[1..]),
        Some("machine") => machine(&args[1..]).map(|()| ExitCode::SUCCESS),
        Some("run") => rt.block_on(load(&args[1..])).map(|()| ExitCode::SUCCESS),
        Some("-h" | "--help" | "help") => {
            println!("{USAGE}");
            Ok(ExitCode::SUCCESS)
        }
        _ => Err(USAGE.to_string()),
    };
    match result {
        Ok(code) => code,
        Err(e) => {
            eprintln!("swarm-sim: {e}");
            ExitCode::from(2)
        }
    }
}

async fn gate(args: &[String]) -> Result<ExitCode, String> {
    let which = args.first().ok_or(USAGE)?.clone();
    let a = Args::parse(&args[1..])?;
    a.only(&[
        "scale", "bin", "work", "results", "stubs", "seed", "agents", "days", "seconds", "docs",
        "reuse", "keep",
    ])?;
    let scale = scale(&a)?;
    let results = PathBuf::from(a.get("results").unwrap_or("target/bench/results"));
    if let Some(p) = a.get("reuse") {
        let text = std::fs::read_to_string(p).map_err(|e| format!("{p}: {e}"))?;
        let v: Verdict = serde_json::from_str(&text).map_err(|e| format!("{p}: {e}"))?;
        print!("{}", v.render());
        println!("(reused {p})");
        return Ok(exit_of(&v));
    }
    let o = Options {
        gate: if which == "g2" {
            "g1".into()
        } else {
            which.clone()
        },
        scale,
        bin: PathBuf::from(a.get("bin").ok_or("--bin <forum-server> is required")?),
        work: PathBuf::from(a.get("work").unwrap_or("target/bench/work")),
        out: None,
        stubs: stubs(&a)?,
        seed: a.num("seed")?.unwrap_or(1),
        agents: a.num("agents")?,
        days: a.num("days")?,
        seconds: a.num("seconds")?,
        docs: a.num("docs")?,
        keep: a.get("keep").is_some_and(|k| k != "0"),
    };
    let verdicts = gates::gate(&o).await?;
    let mut code = ExitCode::from(2);
    for v in &verdicts {
        print!("{}", v.render());
        let file = results.join(format!(
            "{}-{}.json",
            v.gate.to_lowercase().replace('-', ""),
            scale.name()
        ));
        gates::write(v, &file)?;
        println!("result: {}", file.display());
        if v.gate.to_lowercase().replace('-', "") == which {
            code = exit_of(v);
        }
    }
    Ok(code)
}

fn verdict(args: &[String]) -> Result<ExitCode, String> {
    let a = Args::parse(args)?;
    a.only(&[
        "gate", "title", "scale", "status", "note", "results", "work", "bin", "log",
    ])?;
    let gate = a.get("gate").ok_or("--gate is required")?;
    let scale = scale(&a)?;
    let work = PathBuf::from(a.get("work").unwrap_or("target/bench/work"));
    std::fs::create_dir_all(&work).map_err(|e| e.to_string())?;
    let log = a
        .get("log")
        .map(|p| std::fs::read_to_string(p).map_err(|e| format!("{p}: {e}")))
        .transpose()?;
    let bin = a.get("bin").map(PathBuf::from).filter(|b| b.exists());
    let v = gates::scripted(&gates::Scripted {
        gate,
        title: a.get("title").unwrap_or(""),
        scale,
        status: a.get("status").unwrap_or("not-yet"),
        note: a.get("note").unwrap_or(""),
        dir: &work,
        bin: bin.as_deref(),
        log: log.as_deref(),
    });
    print!("{}", v.render());
    let results = PathBuf::from(a.get("results").unwrap_or("target/bench/results"));
    let file = results.join(format!(
        "{}-{}.json",
        gate.to_lowercase().replace('-', ""),
        scale.name()
    ));
    gates::write(&v, &file)?;
    println!("result: {}", file.display());
    Ok(exit_of(&v))
}

fn machine(args: &[String]) -> Result<(), String> {
    let a = Args::parse(args)?;
    a.only(&["dir", "bin"])?;
    let dir = PathBuf::from(a.get("dir").unwrap_or("target/bench/work"));
    let bin = a.get("bin").map(PathBuf::from);
    let m = swarm_sim::machine::machine(&dir, bin.as_deref());
    println!(
        "{}",
        serde_json::to_string_pretty(&m).map_err(|e| e.to_string())?
    );
    eprintln!("{}", swarm_sim::machine::describe(&m));
    Ok(())
}

async fn load(args: &[String]) -> Result<(), String> {
    let a = Args::parse(args)?;
    a.only(&[
        "url",
        "token",
        "agents",
        "profile",
        "mix",
        "seconds",
        "clock-scale",
        "backlog",
        "stubs",
        "out",
        "seed",
    ])?;
    let profile =
        mix::profile(a.get("profile").unwrap_or("base")).ok_or("--profile: low, base, or high")?;
    let tokens = a.all("token");
    if tokens.is_empty() {
        return Err("--token is required".into());
    }
    let mut s = Spec::new(
        a.get("url").ok_or("--url is required")?,
        tokens,
        a.num("agents")?.unwrap_or(100),
        profile,
    );
    s.scale = a.num("clock-scale")?.unwrap_or(1.0);
    s.backlog = a.num("backlog")?.unwrap_or(1_000);
    s.stubs = stubs(&a)?;
    s.seed = a.num("seed")?.unwrap_or(1);
    let mut p = Phase::base("load", Duration::from_secs(a.num("seconds")?.unwrap_or(60)));
    if a.get("mix") == Some("heavy") {
        p.mix = mix::HEAVY;
    }
    s.phases = vec![p];
    let out = run(&s, None).await;
    let report = out.report(&s);
    let text = serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?;
    match a.get("out") {
        Some(f) => std::fs::write(f, text).map_err(|e| format!("{f}: {e}"))?,
        None => println!("{text}"),
    }
    Ok(())
}
