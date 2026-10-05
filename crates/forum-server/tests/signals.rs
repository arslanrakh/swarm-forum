//! The binary's signals (design D.11.8, D.11.3): `forum-server serve` run as a process on a scratch
//! database. SIGTERM stops it while a client holds a standalone SSE stream open (review
//! E4-spec-2), and a SIGHUP while it starts reloads the tokens once it runs rather than end the
//! process (review E4-robust-11). And its settings from the environment, under its flags, with
//! `/healthz` (D.11.9, D.11.2).

#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use forum_server::auth::{hex, sha256};
use forum_server::config::ENV_SETTINGS;

const TOKEN: &str = "fsk_signals-test-token-0123456789abcdefghijklmnop";
const VERSION: &str = "2025-06-18";

/// A running `forum-server serve`, its stderr lines, and its directory.
struct Server {
    child: Child,
    lines: mpsc::Receiver<String>,
    seen: Vec<String>,
    dir: PathBuf,
}

/// The text of a log line without its ANSI colours.
fn plain(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            for d in chars.by_ref() {
                if d.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

impl Server {
    fn start(tag: &str) -> Server {
        Server::start_with(
            tag,
            &["serve", "--db", "forum.db", "--listen", "127.0.0.1:0"],
            &[],
        )
    }

    /// `forum-server` with `args`, and the settings' variables `vars` alone.
    fn start_with(tag: &str, args: &[&str], vars: &[(&str, &str)]) -> Server {
        let dir = std::env::temp_dir().join(format!("forum-signals-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let line = format!("{};class=coding-agent", hex(&sha256(TOKEN.as_bytes())));
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_forum-server"));
        // The settings' variables (D.11.9) are the test's own, never inherited.
        for v in ENV_SETTINGS {
            cmd.env_remove(v);
        }
        let mut child = cmd
            .args(args)
            .envs(vars.iter().copied())
            .current_dir(&dir)
            .env("FORUM_TOKEN_SIGNALS", line)
            .env("NO_COLOR", "1")
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let stderr = child.stderr.take().unwrap();
        let (tx, lines) = mpsc::channel();
        std::thread::spawn(move || {
            for l in BufReader::new(stderr).lines() {
                let Ok(l) = l else { return };
                if tx.send(plain(&l)).is_err() {
                    return;
                }
            }
        });
        Server {
            child,
            lines,
            seen: Vec::new(),
            dir,
        }
    }

    /// The first line containing `what`, waiting at most `timeout`.
    fn line(&mut self, what: &str, timeout: Duration) -> Option<String> {
        if let Some(l) = self.seen.iter().find(|l| l.contains(what)) {
            return Some(l.clone());
        }
        let mut left = timeout;
        loop {
            let step = Duration::from_millis(20).min(left);
            match self.lines.recv_timeout(step) {
                Ok(l) => {
                    self.seen.push(l.clone());
                    if l.contains(what) {
                        return Some(l);
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => left = left.saturating_sub(step),
                Err(mpsc::RecvTimeoutError::Disconnected) => return None,
            }
            if left.is_zero() {
                return None;
            }
        }
    }

    /// The address it listens on, from its log.
    fn addr(&mut self) -> String {
        let l = self
            .line("serving MCP at /mcp", Duration::from_secs(60))
            .unwrap_or_else(|| panic!("it serves: {:?}", self.seen));
        let at = l.split("listen=").nth(1).unwrap();
        at.split_whitespace().next().unwrap().to_string()
    }

    fn signal(&self, sig: &str) {
        let ok = Command::new("kill")
            .args([format!("-{sig}"), self.child.id().to_string()])
            .status()
            .unwrap()
            .success();
        assert!(ok, "kill -{sig}");
    }

    /// How it exited, waiting at most `timeout`.
    fn exit(&mut self, timeout: Duration) -> Option<ExitStatus> {
        let mut left = timeout;
        while !left.is_zero() {
            if let Some(s) = self.child.try_wait().unwrap() {
                return Some(s);
            }
            std::thread::sleep(Duration::from_millis(20));
            left = left.saturating_sub(Duration::from_millis(20));
        }
        None
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Sends one HTTP/1.1 request; with `Connection: close`, reads the whole response.
fn send(addr: &str, method: &str, headers: &[(&str, &str)], body: &str) -> TcpStream {
    let mut s = TcpStream::connect(addr).unwrap();
    let mut head = format!(
        "{method} /mcp HTTP/1.1\r\nHost: {addr}\r\nAuthorization: Bearer {TOKEN}\r\n\
         Content-Length: {}\r\n",
        body.len()
    );
    for (k, v) in headers {
        head.push_str(&format!("{k}: {v}\r\n"));
    }
    head.push_str("\r\n");
    s.write_all(head.as_bytes()).unwrap();
    s.write_all(body.as_bytes()).unwrap();
    s
}

/// One HTTP/1.1 request to `path`, with `Host: <addr>` unless `host` names
/// another, with the token if `auth`; the status code and the body.
fn ask(addr: &str, method: &str, path: &str, host: Option<&str>, auth: bool) -> (u16, String) {
    let mut s = TcpStream::connect(addr).unwrap();
    let mut head = format!(
        "{method} {path} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\nContent-Length: 0\r\n",
        host.unwrap_or(addr)
    );
    if auth {
        head.push_str(&format!("Authorization: Bearer {TOKEN}\r\n"));
    }
    head.push_str("\r\n");
    s.write_all(head.as_bytes()).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
    let mut out = String::new();
    s.read_to_string(&mut out).unwrap();
    let code = out
        .split(' ')
        .nth(1)
        .and_then(|c| c.parse().ok())
        .unwrap_or(0);
    let body = out
        .split_once("\r\n\r\n")
        .map(|(_, b)| b.to_string())
        .unwrap_or_default();
    (code, body)
}

fn post(addr: &str, session: Option<&str>, body: &str) -> String {
    let mut headers = vec![
        ("Content-Type", "application/json"),
        ("Accept", "application/json, text/event-stream"),
        ("Connection", "close"),
        ("MCP-Protocol-Version", VERSION),
    ];
    if let Some(s) = session {
        headers.push(("Mcp-Session-Id", s));
    }
    let mut s = send(addr, "POST", &headers, body);
    s.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
    let mut out = String::new();
    s.read_to_string(&mut out).unwrap();
    out
}

/// The `mcp-session-id` header of a response.
fn session_id(response: &str) -> String {
    response
        .lines()
        .find_map(|l| {
            let (k, v) = l.split_once(':')?;
            k.eq_ignore_ascii_case("mcp-session-id")
                .then(|| v.trim().to_string())
        })
        .unwrap_or_else(|| panic!("no session id: {response}"))
}

/// SIGTERM stops the server while a client holds a standalone `GET /mcp` stream open: the stream
/// ends and the process exits within seconds, where it used to wait on the stream for ever
/// (review E4-spec-2).
#[test]
fn sigterm_stops_the_server_with_an_sse_stream_open() {
    let mut srv = Server::start("sigterm");
    let addr = srv.addr();
    let init = post(
        &addr,
        None,
        &format!(
            r#"{{"jsonrpc":"2.0","id":1,"method":"initialize","params":{{"protocolVersion":"{VERSION}","capabilities":{{}},"clientInfo":{{"name":"signals","version":"1"}}}}}}"#
        ),
    );
    assert!(init.starts_with("HTTP/1.1 200"), "{init}");
    let sid = session_id(&init);
    let r = post(
        &addr,
        Some(&sid),
        r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
    );
    assert!(r.starts_with("HTTP/1.1 202"), "{r}");
    // One MCP call.
    let r = post(
        &addr,
        Some(&sid),
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"session","arguments":{"action":"hello","intent":"stop with a stream open"}}}"#,
    );
    assert!(r.contains("[swarm] running"), "{r}");
    // The standalone SSE stream, held open.
    let mut sse = send(
        &addr,
        "GET",
        &[
            ("Accept", "text/event-stream"),
            ("Mcp-Session-Id", &sid),
            ("MCP-Protocol-Version", VERSION),
        ],
        "",
    );
    sse.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
    let mut head = [0_u8; 512];
    let n = sse.read(&mut head).unwrap();
    let head = String::from_utf8_lossy(&head[..n]).to_string();
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    srv.signal("TERM");
    let status = srv.exit(Duration::from_secs(15));
    assert!(
        status.is_some_and(|s| s.success()),
        "it stopped, cleanly: {status:?} {:?}",
        srv.seen
    );
    assert!(
        srv.line(
            "stopping: finishing the current group",
            Duration::from_secs(1)
        )
        .is_some()
    );
    assert!(srv.line("stopped", Duration::from_secs(1)).is_some());
    drop(sse);
}

/// A SIGHUP while the server starts does not end it: the signals are handled before the engine
/// starts, and the tokens are reloaded once it runs (review E4-robust-11).
#[test]
fn sighup_during_startup_reloads_once_the_server_runs() {
    let mut srv = Server::start("sighup");
    assert!(
        srv.line("starting the engine", Duration::from_secs(60))
            .is_some(),
        "{:?}",
        srv.seen
    );
    // SIGHUP after SIGHUP, while it starts, until it serves.
    let mut sent = 0;
    while srv
        .line("serving MCP at /mcp", Duration::from_millis(1))
        .is_none()
    {
        assert!(
            srv.child.try_wait().unwrap().is_none(),
            "a SIGHUP ended the process: {:?}",
            srv.seen
        );
        srv.signal("HUP");
        sent += 1;
        assert!(sent < 20_000, "it never served: {:?}", srv.seen);
    }
    assert!(sent > 0);
    assert!(
        srv.line("reloading the tokens", Duration::from_secs(10))
            .is_some(),
        "{:?}",
        srv.seen
    );
    srv.signal("TERM");
    let status = srv.exit(Duration::from_secs(15));
    assert!(status.is_some_and(|s| s.success()), "{status:?}");
}

/// D.11.9: `serve` takes its settings from the environment, a flag winning over its variable, and
/// reduces `FORUM_ALLOWED_HOSTS` to host names; `/healthz` answers without a token (D.11.2), while
/// `/mcp` still wants one and refuses a `Host` the variable does not name.
#[test]
fn the_environment_configures_serve_under_its_flags() {
    let mut srv = Server::start_with(
        "environment",
        &["serve", "--db", "flag.db"],
        &[
            ("FORUM_LISTEN", "127.0.0.1:0"),
            ("FORUM_DATABASE", "env.db"),
            ("FORUM_SEARCH_DATABASE", ""),
            (
                "FORUM_ALLOWED_HOSTS",
                "https://forum.example.com:8640/, 127.0.0.1",
            ),
        ],
    );
    let addr = srv.addr();
    assert!(srv.dir.join("flag.db").exists(), "the flag wins");
    assert!(!srv.dir.join("env.db").exists());
    assert_eq!(
        ask(&addr, "GET", "/healthz", None, false),
        (200, "ok".into())
    );
    assert_eq!(
        ask(
            &addr,
            "GET",
            "/healthz",
            Some("elsewhere.example.org"),
            false
        )
        .0,
        200
    );
    assert_eq!(ask(&addr, "POST", "/mcp", None, false).0, 401);
    // With the token, the domain passes rmcp's Host check (a PUT then gets 405), and another
    // does not.
    assert_eq!(
        ask(&addr, "PUT", "/mcp", Some("forum.example.com"), true).0,
        405
    );
    assert_eq!(ask(&addr, "PUT", "/mcp", None, true).0, 405);
    assert_eq!(
        ask(&addr, "PUT", "/mcp", Some("elsewhere.example.org"), true).0,
        403
    );
    srv.signal("TERM");
    let status = srv.exit(Duration::from_secs(15));
    assert!(status.is_some_and(|s| s.success()), "{status:?}");
    assert!(srv.line("stopped", Duration::from_secs(1)).is_some());
    assert!(
        srv.seen.iter().all(|l| !l.contains("allowed hosts")),
        "nothing to warn of on loopback: {:?}",
        srv.seen
    );
    assert!(
        !srv.dir.join("flag-search.db").exists(),
        "FORUM_SEARCH_DATABASE empty: no search"
    );
    drop(srv);
    // `*` turns rmcp's Host check off, and the log says so; the token is still required.
    let mut srv = Server::start_with(
        "any-host",
        &["serve", "--listen", "127.0.0.1:0"],
        &[("FORUM_ALLOWED_HOSTS", "*")],
    );
    let addr = srv.addr();
    assert!(
        srv.line("every Host header is accepted", Duration::from_secs(10))
            .is_some(),
        "{:?}",
        srv.seen
    );
    assert_eq!(
        ask(&addr, "PUT", "/mcp", Some("elsewhere.example.org"), true).0,
        405
    );
    assert_eq!(
        ask(&addr, "POST", "/mcp", Some("elsewhere.example.org"), false).0,
        401
    );
    assert!(srv.dir.join("forum.db").exists(), "the default database");
}
