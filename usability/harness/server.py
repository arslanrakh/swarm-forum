"""Building, starting, stopping, and exporting `forum-server` for one run.

Each run gets a fresh directory, database, and port. The bearer tokens are made per run and reach
the server only through its environment (`FORUM_TOKEN_<NAME>`, D.11.3), never a file: the server's
working directory is the run directory, which has no `.env`. After the run the history is read with
`forum-server usability-export`, a read-only subcommand of the `usability` feature.
"""

from __future__ import annotations

import base64
import hashlib
import json
import os
import re
import secrets
import signal
import socket
import subprocess
import time
import urllib.error
import urllib.request
from pathlib import Path

from . import params as P

ARMS = {"g": "with_guidance", "nog": "without_guidance"}


def repo_root() -> Path:
    return Path(__file__).resolve().parents[2]


VERSION_LINE = re.compile(r"^Version (\d+(?:\.\d+)+)\b")


def protocol_version(spec: Path | None = None) -> str | None:
    """The protocol's version as `docs/protocol.md` states it ("Version 1.5, ..." near its top), or
    None when it states none."""
    try:
        head = (spec or repo_root() / "docs" / "protocol.md").read_text().splitlines()[:20]
    except OSError:
        return None
    for line in head:
        m = VERSION_LINE.match(line.strip())
        if m:
            return m.group(1)
    return None


def build(release_check: bool = True) -> Path:
    """Builds the debug binary with the `usability` feature (it does not compile in release)."""
    root = repo_root()
    subprocess.run(
        ["cargo", "build", "-q", "-p", "forum-server", "--features", "usability", "--bin", "forum-server"],
        cwd=root,
        check=True,
    )
    return root / "target" / "debug" / "forum-server"


def new_token() -> str:
    """A token as `forum-server token new` makes it: `fsk_` and 43 base64url characters."""
    return "fsk_" + base64.urlsafe_b64encode(secrets.token_bytes(32)).decode().rstrip("=")


def token_value(token: str, cls: str, caps: list[str] | None) -> str:
    v = f"{hashlib.sha256(token.encode()).hexdigest()};class={cls}"
    if caps:
        v += ";caps=" + ",".join(caps)
    return v


def free_port() -> int:
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def minimal_env() -> dict:
    """The environment of child processes: enough to run, nothing of the calling session."""
    keep = ["PATH", "HOME", "USER", "LOGNAME", "LANG", "LC_ALL", "TMPDIR", "TERM", "SHELL"]
    return {k: os.environ[k] for k in keep if k in os.environ}


class Server:
    """One `forum-server serve` process for one run."""

    def __init__(
        self, binary: Path, run_dir: Path, parameters: dict, arm: str, tokens: dict, clock_lag_ms: int = 0
    ):
        """`tokens` maps a name to `{"class": ..., "caps": [...]}`; tokens are made here.
        `clock_lag_ms` starts the forum's clock that far behind the system's (the lab's seeds,
        `catch_up`)."""
        self.clock_lag_ms = clock_lag_ms
        self.binary = binary
        self.dir = run_dir
        self.params = parameters
        self.arm = arm
        self.port = free_port()
        self.url = f"http://127.0.0.1:{self.port}/mcp"
        self.tokens = {name: new_token() for name in tokens}
        self.token_specs = tokens
        self.proc: subprocess.Popen | None = None
        self.log_path = run_dir / "server.log"

    def config_text(self) -> str:
        head = [
            f'listen = "127.0.0.1:{self.port}"',
            'database = "forum.db"',
            'search_database = "forum-search.db"',
            f'usability_arm = "{ARMS[self.arm]}"',
        ]
        if self.clock_lag_ms:
            head += [f'usability_clock_lag = "{-(-self.clock_lag_ms // 1000)}s"', 'usability_clock_jump_file = "clock.jump"']
        return "\n".join(head) + "\n" + P.toml(self.params)

    def start(self, wait: float = 30.0) -> None:
        bad = P.problems(self.params)
        if bad:
            raise RuntimeError(f"the parameters break §22: {bad}")
        self.dir.mkdir(parents=True, exist_ok=True)
        if (self.dir / "forum.db").exists():
            raise RuntimeError(f"{self.dir / 'forum.db'} exists: every run starts on a fresh database")
        (self.dir / "forum.toml").write_text(self.config_text())
        env = minimal_env()
        env["RUST_LOG"] = "info"
        for name, spec in self.token_specs.items():
            var = "FORUM_TOKEN_" + name.upper().replace("-", "_")
            env[var] = token_value(self.tokens[name], spec["class"], spec.get("caps"))
        log = self._log = open(self.log_path, "w")
        self.proc = subprocess.Popen(
            [str(self.binary), "serve", "--config", "forum.toml"],
            cwd=self.dir,
            env=env,
            stdout=log,
            stderr=subprocess.STDOUT,
            start_new_session=True,
        )
        deadline = time.monotonic() + wait
        while time.monotonic() < deadline:
            if self.proc.poll() is not None:
                raise RuntimeError(f"forum-server exited: {self.log_path.read_text()[-2000:]}")
            if self._answers():
                return
            time.sleep(0.2)
        raise RuntimeError("forum-server did not start")

    def catch_up(self, ms: int, wait: float = 10.0) -> None:
        """Moves the forum's lagged clock forward by `ms` (at most to the system's), and waits until
        the server has read it."""
        if not self.clock_lag_ms or ms <= 0:
            return
        f = self.dir / "clock.jump"
        tmp = self.dir / "clock.jump.tmp"
        tmp.write_text(str(int(ms)))
        tmp.rename(f)
        deadline = time.monotonic() + wait
        while f.exists():
            if time.monotonic() > deadline:
                raise RuntimeError("the server did not read the clock's catch-up")
            time.sleep(0.02)
        self.clock_lag_ms = max(0, self.clock_lag_ms - int(ms))

    def _answers(self) -> bool:
        req = urllib.request.Request(self.url, data=b"{}", method="POST")
        try:
            urllib.request.urlopen(req, timeout=2).close()
            return True
        except urllib.error.HTTPError as e:
            e.close()
            return True  # 401 without a token: it is up
        except (urllib.error.URLError, OSError):
            return False

    def stop(self) -> None:
        if self.proc and self.proc.poll() is None:
            self.proc.send_signal(signal.SIGTERM)
            try:
                self.proc.wait(timeout=30)
            except subprocess.TimeoutExpired:
                self.proc.kill()
                self.proc.wait()
        if getattr(self, "_log", None):
            self._log.close()

    def export(self) -> list[dict]:
        """The whole history as `control export`'s JSON lines, saved as `export.jsonl` too."""
        out = subprocess.run(
            [str(self.binary), "usability-export", "--db", str(self.dir / "forum.db")],
            env=minimal_env(),
            capture_output=True,
            text=True,
            check=True,
        ).stdout
        (self.dir / "export.jsonl").write_text(out)
        return [json.loads(line) for line in out.splitlines() if line.strip()]
