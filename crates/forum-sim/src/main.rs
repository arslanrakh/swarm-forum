//! `forum-sim`: the year simulation (design D.15.4, WP-Y), light form.
//!
//! ```text
//! forum-sim --profile company|no-user [--days <d>] [--seed <n>] [--warmup-days <d>]
//!           [--cooldown-days <d>] [--principals <n>] [--p-follow <0..1>] [--check-hours <h>]
//!           [--out <file>]
//! ```
//!
//! Runs the profile and prints its report against D.15.4 in Markdown (or writes it to `--out`).
//! See the library's documentation for what the run simulates and what its light form leaves out.

use std::process::ExitCode;

use forum_sim::profile::{Kind, Profile};
use forum_sim::rng::PPM;

const USAGE: &str = "usage: forum-sim --profile company|no-user [--days <d>] [--seed <n>] \
                     [--warmup-days <d>] [--cooldown-days <d>] [--principals <n>] \
                     [--p-follow <0..1>] [--check-hours <h>] [--out <file>]";

/// A decimal probability (`0.5`, `0.9`, `1`) as parts per million, without floats.
fn parse_ppm(s: &str) -> Option<u32> {
    let (int, frac) = s.split_once('.').unwrap_or((s, ""));
    let int: u32 = if int.is_empty() { 0 } else { int.parse().ok()? };
    if frac.len() > 6 || !frac.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let mut f: u32 = if frac.is_empty() {
        0
    } else {
        frac.parse().ok()?
    };
    for _ in frac.len()..6 {
        f *= 10;
    }
    let v = int.checked_mul(PPM)?.checked_add(f)?;
    (v <= PPM).then_some(v)
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut kind = None;
    let mut days = 30_u64;
    let mut seed = 1_u64;
    let mut warmup: Option<u64> = None;
    let mut cooldown: Option<u64> = None;
    let mut principals: Option<usize> = None;
    let mut p_follow = 0_u32;
    let mut check_hours: Option<u64> = None;
    let mut out: Option<String> = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let Some(v) = it.next() else {
            eprintln!("{a} needs a value\n{USAGE}");
            return ExitCode::from(2);
        };
        let ok = match a.as_str() {
            "--profile" => {
                kind = Kind::parse(v);
                kind.is_some()
            }
            "--days" => v.parse().map(|x| days = x).is_ok(),
            "--seed" => v.parse().map(|x| seed = x).is_ok(),
            "--warmup-days" => v.parse().map(|x| warmup = Some(x)).is_ok(),
            "--cooldown-days" => v.parse().map(|x| cooldown = Some(x)).is_ok(),
            "--principals" => v.parse().map(|x| principals = Some(x)).is_ok(),
            "--p-follow" => parse_ppm(v).map(|x| p_follow = x).is_some(),
            "--check-hours" => v.parse().map(|x| check_hours = Some(x)).is_ok(),
            "--out" => {
                out = Some(v.clone());
                true
            }
            _ => false,
        };
        if !ok {
            eprintln!("bad argument {a} {v}\n{USAGE}");
            return ExitCode::from(2);
        }
    }
    let Some(kind) = kind else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };
    let mut p = Profile::of(kind, days, seed);
    if let Some(w) = warmup {
        p.warmup_days = w;
    }
    if let Some(c) = cooldown {
        p.cooldown_days = c;
    }
    if let Some(n) = principals {
        p.principals = n;
    }
    if let Some(h) = check_hours {
        p.check_every_hours = h;
    }
    p.p_follow = p_follow;
    let sim = forum_sim::run(p);
    let report = forum_sim::report::render(&sim);
    match out {
        Some(path) => {
            if let Err(e) = std::fs::write(&path, report) {
                eprintln!("cannot write {path}: {e}");
                return ExitCode::from(1);
            }
        }
        None => print!("{report}"),
    }
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::parse_ppm;

    #[test]
    fn probabilities_parse_without_floats() {
        assert_eq!(parse_ppm("0"), Some(0));
        assert_eq!(parse_ppm("0.5"), Some(500_000));
        assert_eq!(parse_ppm("0.9"), Some(900_000));
        assert_eq!(parse_ppm("1"), Some(1_000_000));
        assert_eq!(parse_ppm(".25"), Some(250_000));
        assert_eq!(parse_ppm("0.000001"), Some(1));
        assert_eq!(parse_ppm("1.5"), None);
        assert_eq!(parse_ppm("0.0000001"), None);
        assert_eq!(parse_ppm("x"), None);
    }
}
