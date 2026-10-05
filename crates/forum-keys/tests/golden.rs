//! The golden files: `golden.txt` pins version 1 of the key normaliser, and `golden_principal.txt`
//! pins the principal-key normaliser (design D.11.14, D.12.1).

use forum_keys::{KeyError, NormKey, is_normal, normalise, normalise_principal, normalise_v};

/// One row of a golden file.
struct Row {
    line: usize,
    input: String,
    expected: Result<String, String>,
}

/// Parses a golden file; panics with the line number on a malformed row.
fn rows(file: &str, text: &str) -> Vec<Row> {
    let mut rows = Vec::new();
    for (index, line) in text.lines().enumerate() {
        let line_no = index + 1;
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let columns: Vec<&str> = line.split('\t').collect();
        let [input, outcome, output] = columns.as_slice() else {
            panic!("{file}:{line_no}: expected three tab-separated columns, got {columns:?}");
        };
        let input = unescape(input).unwrap_or_else(|e| panic!("{file}:{line_no}: {e}"));
        let output = unescape(output).unwrap_or_else(|e| panic!("{file}:{line_no}: {e}"));
        let expected = match *outcome {
            "ok" => Ok(output),
            "err" => Err(output),
            other => panic!("{file}:{line_no}: the second column is {other:?}, not ok or err"),
        };
        rows.push(Row {
            line: line_no,
            input,
            expected,
        });
    }
    assert!(!rows.is_empty(), "{file} has no rows");
    rows
}

/// Resolves the escapes `\\`, `\t`, `\n`, `\r`, and `\u{HEX}`.
fn unescape(s: &str) -> Result<String, String> {
    let mut out = String::new();
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('\\') => out.push('\\'),
            Some('t') => out.push('\t'),
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some('u') => {
                if chars.next() != Some('{') {
                    return Err(format!("malformed \\u escape in {s:?}"));
                }
                let hex: String = chars.by_ref().take_while(|&c| c != '}').collect();
                let code = u32::from_str_radix(&hex, 16)
                    .map_err(|_| format!("bad hexadecimal {hex:?} in {s:?}"))?;
                out.push(char::from_u32(code).ok_or(format!("bad code point {hex} in {s:?}"))?);
            }
            other => return Err(format!("unknown escape \\{other:?} in {s:?}")),
        }
    }
    Ok(out)
}

/// The variant name of an error, without the message of `BadValue`: the golden files pin which
/// rule refused a key, not the wording of the hint.
fn variant(e: KeyError) -> String {
    match e {
        KeyError::Empty => "Empty".into(),
        KeyError::NoScheme => "NoScheme".into(),
        KeyError::BadScheme => "BadScheme".into(),
        KeyError::BadValue(_) => "BadValue".into(),
        KeyError::TooLong => "TooLong".into(),
        KeyError::UnknownVersion(v) => format!("UnknownVersion({v})"),
    }
}

/// Runs every row through `f` and reports every mismatch at once.
fn check(file: &str, text: &str, f: impl Fn(&str) -> Result<String, KeyError>) -> Vec<Row> {
    let rows = rows(file, text);
    let mismatches: Vec<String> = rows
        .iter()
        .filter_map(|row| {
            let got = f(&row.input).map_err(variant);
            (got != row.expected).then(|| {
                format!(
                    "{file}:{}: {:?} gave {got:?}, expected {:?}",
                    row.line, row.input, row.expected
                )
            })
        })
        .collect();
    assert!(mismatches.is_empty(), "\n{}", mismatches.join("\n"));
    rows
}

#[test]
fn golden_version_1() {
    let rows = check("golden.txt", include_str!("golden.txt"), |raw| {
        normalise_v(1, raw).map(|NormKey { key, version }| {
            assert_eq!(version, 1, "{raw:?}");
            key
        })
    });
    // Every normal form is normal: K2's oracle accepts it, and normalising it again changes nothing.
    for row in &rows {
        if let Ok(key) = &row.expected {
            assert!(
                is_normal(key, 1),
                "golden.txt:{}: {key:?} is not normal",
                row.line
            );
            assert_eq!(
                normalise_v(1, key),
                Ok(NormKey {
                    key: key.clone(),
                    version: 1
                }),
                "golden.txt:{}",
                row.line
            );
        }
    }
}

#[test]
fn golden_principal() {
    let rows = check(
        "golden_principal.txt",
        include_str!("golden_principal.txt"),
        normalise_principal,
    );
    for row in &rows {
        if let Ok(key) = &row.expected {
            assert_eq!(
                normalise_principal(key).as_ref(),
                Ok(key),
                "golden_principal.txt:{}",
                row.line
            );
        }
        // A person's alias is the principal key itself (D-3): `person:<p>` normalises to
        // `person:` and p's normal form, and refuses what the principal normaliser refuses.
        let as_person = normalise(&format!("person:{}", row.input)).map(|n| n.key);
        match &row.expected {
            Ok(key) => assert_eq!(
                as_person,
                Ok(format!("person:{key}")),
                "golden_principal.txt:{}",
                row.line
            ),
            Err(_) => assert!(as_person.is_err(), "golden_principal.txt:{}", row.line),
        }
    }
}
