//! `control export {after?, limit?}` (design D.11.15, §20.8, A-78): a page of the history after
//! `after`, as JSON lines.
//!
//! Each event is one line, `{"seq": <seq>, "event": <its canonical JSON>}`, decompressed from the
//! history; the first time a transaction appears in the page, a line with its `txs` row precedes
//! its first event: `{"tx": <id>, "kind": …, "at": <ms>, "class": …, "session": …, "op_id": …}`.
//! A page holds at most `limit` events (1,000 by default, the core's `EXPORT_DEFAULT`) and at most
//! [`EXPORT_BYTES_MAX`] bytes of lines, but always at least one event when there is one, so that
//! paging with the returned `next_after` always moves on. Hidden content is included: hiding
//! affects rendering only (§16), and masked secrets never reached the history (D.11.13). An export
//! is a copy of the history, not a backup of the state (D.11.6).

use rusqlite::Connection;
use serde_json::{Map, Value, json};

use crate::render::json::Json;
use crate::store::history::{StoredEvent, TxKind, TxRow, events_after, txs_after};
use crate::store::{Result, StoreError};

/// The most bytes of lines one page carries (D.11.15).
pub const EXPORT_BYTES_MAX: usize = 1 << 20;

/// Events read from the history per query while a page fills.
const BATCH: u32 = 256;

/// One page of the history.
#[derive(Clone, Debug, PartialEq)]
pub struct ExportPage {
    /// The `after` asked for.
    pub after: Option<u64>,
    /// The lines, in order: transaction rows and events.
    pub lines: Vec<Value>,
    /// How many events the page holds.
    pub events: u32,
    /// The `after` of the next page: the last event's `seq`, or `after` for an empty page.
    pub next_after: u64,
}

/// The name of a transaction kind in an export.
#[must_use]
pub fn kind_name(k: TxKind) -> &'static str {
    match k {
        TxKind::Genesis => "genesis",
        TxKind::Settle => "settle",
        TxKind::Cmd => "cmd",
        TxKind::Admin => "admin",
        TxKind::Migrate => "migrate",
        TxKind::Restore => "restore",
    }
}

fn tx_line(t: &TxRow) -> Value {
    let mut m = Map::new();
    m.insert("tx".into(), json!(t.id));
    m.insert("kind".into(), json!(kind_name(t.meta.kind)));
    m.insert("at".into(), json!(t.meta.at.0));
    m.insert(
        "class".into(),
        t.meta
            .class
            .as_ref()
            .map_or(Value::Null, |c| json!(c.as_str())),
    );
    m.insert(
        "session".into(),
        t.meta.session.map_or(Value::Null, |s| json!(s.render())),
    );
    m.insert(
        "op_id".into(),
        t.meta
            .op_id
            .as_ref()
            .map_or(Value::Null, |o| json!(o.as_str())),
    );
    if let Some(d) = &t.meta.detail {
        m.insert("detail".into(), json!(d));
    }
    Value::Object(m)
}

fn event_line(e: &StoredEvent) -> Result<Value> {
    Ok(json!({"seq": e.seq, "event": serde_json::to_value(&e.event)?}))
}

fn size(v: &Value) -> usize {
    serde_json::to_string(v).map_or(0, |s| s.len().saturating_add(1))
}

/// The transaction row `id`.
fn tx_row(conn: &Connection, id: i64) -> Result<TxRow> {
    txs_after(conn, id.saturating_sub(1), 1)?
        .into_iter()
        .find(|t| t.id == id)
        .ok_or_else(|| StoreError::Invalid(format!("the event's transaction {id} has no row")))
}

/// The page of the history after `after` (every event when None), at most `limit` events.
///
/// # Errors
///
/// A failed query, or a stored event or transaction that does not decode.
pub fn page(conn: &Connection, after: Option<u64>, limit: u32) -> Result<ExportPage> {
    let mut out = ExportPage {
        after,
        lines: Vec::new(),
        events: 0,
        next_after: after.unwrap_or(0),
    };
    let mut bytes = 0_usize;
    let mut last_tx: Option<i64> = None;
    'fill: while out.events < limit {
        let want = BATCH.min(limit - out.events);
        let batch = events_after(conn, out.next_after, want)?;
        if batch.is_empty() {
            break;
        }
        for e in &batch {
            let mut add = Vec::with_capacity(2);
            if last_tx != Some(e.tx) {
                add.push(tx_line(&tx_row(conn, e.tx)?));
            }
            add.push(event_line(e)?);
            let n: usize = add.iter().map(size).sum();
            if out.events > 0 && bytes.saturating_add(n) > EXPORT_BYTES_MAX {
                break 'fill;
            }
            bytes = bytes.saturating_add(n);
            out.lines.extend(add);
            out.events += 1;
            out.next_after = e.seq;
            last_tx = Some(e.tx);
            if out.events >= limit {
                break 'fill;
            }
        }
    }
    Ok(out)
}

/// A `serde_json` value as the renderer's ordered JSON, its object keys sorted by bytes, the
/// canonical order of D.11.5, whatever order `serde_json` holds them in (it keeps insertion order,
/// `preserve_order`, for the structured content's sake, D.11.7).
#[must_use]
pub fn to_json(v: &Value) -> Json {
    match v {
        Value::Null => Json::Null,
        Value::Bool(b) => Json::Bool(*b),
        Value::Number(n) => n
            .as_u64()
            .map(Json::U)
            .or_else(|| n.as_i64().map(Json::I))
            .unwrap_or_else(|| Json::S(n.to_string())),
        Value::String(s) => Json::S(s.clone()),
        Value::Array(a) => Json::A(a.iter().map(to_json).collect()),
        Value::Object(m) => {
            let mut fields: Vec<(String, Json)> =
                m.iter().map(|(k, x)| (k.clone(), to_json(x))).collect();
            fields.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
            Json::O(fields)
        }
    }
}

/// One line of an export as canonical JSON (D.11.5, D.11.15): keys sorted by bytes, no
/// insignificant whitespace. A line holds no float (the core has none), so `canonical_json`
/// always writes it; `serde_json`'s own writing is the fallback.
#[must_use]
pub fn line_text(l: &Value) -> String {
    crate::digest::canonical_json(l).unwrap_or_else(|| serde_json::to_string(l).unwrap_or_default())
}

/// The body of an export response: `{after, limit, events, next_after}`, and the lines, each the
/// canonical JSON of one line (D.11.15).
#[must_use]
pub fn body(page: &ExportPage, limit: u32) -> (Json, Vec<String>) {
    let head = Json::obj()
        .with_opt("after", page.after.map(Json::U))
        .with("limit", Json::from(limit))
        .with("events", Json::from(page.events))
        .with("next_after", Json::U(page.next_after));
    let lines = page.lines.iter().map(line_text).collect();
    (head, lines)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::TxKind;
    use crate::store::testkit::{appended, create_node, init, with_session};
    use forum_core::{Eid, EntryBody, Ev, HideTarget, ROOT, Text};

    #[test]
    fn a_page_carries_each_transaction_once_and_the_next_after() {
        let (mut w, s) = with_session();
        let n = create_node(&mut w, ROOT, "a", s, init(1, &[]));
        let ev = appended(&w, n, s, EntryBody::Object, "the secret plan");
        w.tx(TxKind::Cmd, vec![ev]);
        let eid = Eid(w.s.counters.next_eid.0 - 1);
        w.tx(
            TxKind::Cmd,
            vec![Ev::Hidden {
                target: HideTarget::Entry(eid),
                reason: Text("tidy".into()),
            }],
        );
        let total = crate::store::history::last_seq(&w.conn).unwrap();
        assert!(total >= 4);
        let all = page(&w.conn, None, 1_000).unwrap();
        assert_eq!(u64::from(all.events), total);
        assert_eq!(all.next_after, total);
        // Every event line is preceded by its transaction's line the first time it appears.
        let mut seen_tx = std::collections::BTreeSet::new();
        let mut last = None;
        for l in &all.lines {
            if let Some(id) = l.get("tx") {
                assert!(
                    seen_tx.insert(id.as_i64().unwrap()),
                    "a transaction appears once"
                );
                assert!(l.get("kind").is_some() && l.get("at").is_some());
                last = id.as_i64();
            } else {
                assert!(l.get("seq").is_some() && l.get("event").is_some());
                assert!(
                    last.is_some(),
                    "an event comes after its transaction's line"
                );
            }
        }
        assert_eq!(all.lines[0]["kind"], "genesis");
        // Each line is canonical JSON, keys sorted at every depth, in the text and in the
        // structured content alike, though `serde_json` keeps insertion order (D.11.15).
        let (_, texts) = body(&all, 1_000);
        for (text, line) in texts.iter().zip(&all.lines) {
            assert_eq!(Some(text.clone()), crate::digest::canonical_json(line));
            assert_eq!(to_json(line).text(), *text);
        }
        assert!(texts[0].starts_with("{\"at\":"), "{}", texts[0]);
        assert!(
            texts.iter().any(|t| t.starts_with("{\"event\":")),
            "an event line starts with event, before seq"
        );
        // Hidden content is in the export (§16).
        let text = serde_json::to_string(&all.lines).unwrap();
        assert!(text.contains("the secret plan"));
        // Paging one event at a time reaches the same events, each page naming its transaction.
        let mut after = None;
        let mut seqs = Vec::new();
        loop {
            let p = page(&w.conn, after, 1).unwrap();
            if p.events == 0 {
                assert_eq!(p.next_after, after.unwrap_or(0));
                break;
            }
            assert_eq!(p.events, 1);
            assert!(
                p.lines[0].get("tx").is_some(),
                "each page names the transaction"
            );
            seqs.push(p.next_after);
            after = Some(p.next_after);
        }
        assert_eq!(seqs, (1..=total).collect::<Vec<_>>());
    }

    #[test]
    fn a_page_stops_at_a_mebibyte_but_never_empty() {
        let (mut w, s) = with_session();
        let n = create_node(&mut w, ROOT, "big", s, init(1, &[]));
        let text = "x".repeat(60_000);
        for _ in 0..40 {
            let ev = appended(&w, n, s, EntryBody::Object, &text);
            w.tx(TxKind::Cmd, vec![ev]);
        }
        let p = page(&w.conn, None, 1_000).unwrap();
        let bytes: usize = p.lines.iter().map(size).sum();
        assert!(bytes <= EXPORT_BYTES_MAX, "{bytes}");
        let total = crate::store::history::last_seq(&w.conn).unwrap();
        assert!(u64::from(p.events) < total, "the page stopped early");
        // The rest follows from next_after, page by page.
        let mut events = u64::from(p.events);
        let mut q = page(&w.conn, Some(p.next_after), 1_000).unwrap();
        while q.next_after < total {
            events += u64::from(q.events);
            q = page(&w.conn, Some(q.next_after), 1_000).unwrap();
        }
        events += u64::from(q.events);
        assert_eq!(events, total);
        let (head, lines) = body(&q, 1_000);
        assert_eq!(lines.len(), q.lines.len());
        assert_eq!(head.get("next_after"), Some(&Json::U(total)));
        // The last event alone still makes a page.
        let one = page(&w.conn, Some(total - 1), 1_000).unwrap();
        assert_eq!(one.events, 1);
    }
}
