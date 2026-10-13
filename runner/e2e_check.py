"""Drive a runner the way VirtuAI Desktop and the server do, end to end.

    python desktop/runner/e2e_check.py desktop/runner/dist/virtuai/virtuai
    python desktop/runner/e2e_check.py "$(which virtuai)"      # the source CLI
    python desktop/runner/e2e_check.py <cli> --browser          # + a real browser action

Plays both sides around a real `virtuai run --desktop`: a minimal server on a
local WebSocket (hello -> ready -> execute frames) and the app on the runner's
stdin/stdout (answering approval requests). Checks that a read-only command
runs unasked, an allowed one runs, a denied one does not, and that closing
stdin stops the runner. With --browser it also opens a local page in a real
Chrome through browser-use (needs Chrome, and a display). Needs only
`websockets`, which the CLI depends on.

This is what proves a frozen build works: unit tests import the source, and a
frozen binary can start fine and still miss a module that is only imported
when the first command arrives.
"""

from __future__ import annotations

import asyncio
import json
import logging
import os
import subprocess
import sys
import tempfile
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

import websockets

PREFIX = "@@virtuai "
# The runner's skills sync makes plain HTTP requests to the same port, and the
# stdin-close check drops the socket: both are expected, neither is news.
logging.getLogger("websockets").setLevel(logging.CRITICAL + 1)
# What the "person" answers, by command.
ANSWERS = {"touch allowed.txt": True, "touch denied.txt": False}


PAGE_TITLE = "VirtuAI e2e page"


class _Page(BaseHTTPRequestHandler):
    def do_GET(self):
        body = f"<html><head><title>{PAGE_TITLE}</title></head><body><a href='#'>e2e link</a></body></html>"
        self.send_response(200)
        self.send_header("Content-Type", "text/html")
        self.end_headers()
        self.wfile.write(body.encode())

    def log_message(self, *args):
        pass


def main(cli: str, browser: bool = False) -> int:
    with tempfile.TemporaryDirectory() as tmp:
        # E2E_TMP_MODE=777 lets a runner started as another user (a wrapper
        # around runuser, for Chrome, which refuses root) write its HOME.
        os.chmod(tmp, int(os.environ.get("E2E_TMP_MODE", "700"), 8))
        home = Path(tmp) / "home"
        work = Path(tmp) / "work"
        (home / ".virtuai").mkdir(parents=True)
        (home / ".virtuai" / "credentials.json").write_text(json.dumps({"cli_token": "e2e"}))
        # Rules only: no Ollama here, so everything the rules do not clear is asked.
        (home / ".virtuai" / "config.json").write_text(json.dumps({"command_classifier": "off"}))
        if os.environ.get("E2E_TMP_MODE"):
            work.mkdir()
            for p in [home, home / ".virtuai", work, *(home / ".virtuai").iterdir()]:
                os.chmod(p, 0o777 if p.is_dir() else 0o666)
        return asyncio.run(_run(cli, home, work, browser))


async def _run(cli: str, home: Path, work: Path, browser: bool) -> int:
    results: dict[str, dict] = {}
    browsed: dict[str, dict] = {}
    asked: list[str] = []
    ready: dict = {}
    done = asyncio.Event()
    page_server = None
    if browser:
        page_server = ThreadingHTTPServer(("127.0.0.1", 0), _Page)
        threading.Thread(target=page_server.serve_forever, daemon=True).start()

    async def server(ws):
        await ws.send(json.dumps({"v": 1, "id": "h", "type": "hello"}))
        frames = [("f1", "ls -la"), ("f2", "touch allowed.txt"), ("f3", "touch denied.txt")]
        for fid, command in frames:
            await ws.send(json.dumps({
                "v": 1, "id": fid, "type": "execute", "command": command, "timeout_secs": 60,
            }))
        async for raw in ws:
            msg = json.loads(raw)
            if msg.get("type") == "ready":
                ready.update(msg)
            elif msg.get("type") == "execute_result":
                results[msg["id"]] = msg
                if len(results) == len(frames) and browser:
                    # One at a time, as the agent does: get_state after navigate.
                    url = f"http://127.0.0.1:{page_server.server_address[1]}/"
                    await ws.send(json.dumps({"v": 1, "id": "b1", "type": "browser_action",
                                              "action": "navigate", "params": {"url": url}}))
                elif len(results) == len(frames):
                    done.set()
            elif msg.get("type") == "browser_action_result":
                browsed[msg["id"]] = msg
                if msg["id"] == "b1":
                    await ws.send(json.dumps({"v": 1, "id": "b2", "type": "browser_action",
                                              "action": "get_state", "params": {}}))
                else:
                    done.set()

    async with websockets.serve(server, "127.0.0.1", 0) as srv:
        port = srv.sockets[0].getsockname()[1]
        env = {
            **os.environ,
            "HOME": str(home),
            "VIRTUAI_TRANSPORT": "socket",
            "PYTHON_KEYRING_BACKEND": "keyring.backends.fail.Keyring",
        }
        proc = subprocess.Popen(
            [cli, "run", "--desktop", "--allow-multiple", "--server", f"http://127.0.0.1:{port}",
             "--workdir", str(work), "--serve-models", "none"],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
            text=True, env=env,
        )
        log: list[str] = []

        def app_side():
            for line in proc.stdout:
                log.append(line.rstrip())
                if not line.startswith(PREFIX):
                    continue
                event = json.loads(line[len(PREFIX):])
                if event.get("type") == "approval_request":
                    asked.append(event["command"])
                    approved = ANSWERS.get(event["command"], False)
                    proc.stdin.write(json.dumps(
                        {"type": "approval_response", "id": event["id"], "approved": approved}) + "\n")
                    proc.stdin.flush()

        threading.Thread(target=app_side, daemon=True).start()
        try:
            await asyncio.wait_for(done.wait(), 240 if browser else 120)
        except asyncio.TimeoutError:
            print("e2e: timed out; runner output:\n" + "\n".join(log), file=sys.stderr)
            proc.kill()
            return 1

        # The app quitting: stdin closes, the runner must stop by itself.
        proc.stdin.close()
        try:
            exit_code = await asyncio.to_thread(proc.wait, 30)
        except subprocess.TimeoutExpired:
            proc.kill()
            exit_code = None

    failures = []
    if results["f1"].get("exit_code") != 0:
        failures.append(f"ls should run: {results['f1']}")
    if "ls -la" in asked:
        failures.append("ls should not be asked about")
    if results["f2"].get("exit_code") != 0 or not (work / "allowed.txt").exists():
        failures.append(f"an allowed command should run: {results['f2']}")
    if results["f3"].get("exit_code") != 126 or (work / "denied.txt").exists():
        failures.append(f"a denied command must not run: {results['f3']}")
    if "did not authorize" not in results["f3"].get("output", ""):
        failures.append("the agent should be told the person said no")
    if sorted(asked) != sorted(ANSWERS):
        failures.append(f"asked about {asked}, expected {list(ANSWERS)}")
    if exit_code is None:
        failures.append("the runner kept running after stdin closed")
    if browser:
        # `ready.browser.available` is not asserted: it only looks for Chrome
        # by name, and a test machine may point VIRTUAI_BROWSER_EXECUTABLE at
        # a Chromium elsewhere. What matters is that a page opens and is read.
        nav, state = browsed.get("b1", {}), browsed.get("b2", {})
        if nav.get("error"):
            failures.append(f"navigate failed: {nav['error']}")
        seen = " ".join(str(state.get(k) or "") for k in ("result", "title", "dom_state"))
        if state.get("error") or PAGE_TITLE not in seen or "e2e link" not in seen:
            failures.append(f"get_state did not see the page: {state}")
    if failures:
        print("e2e: FAILED\n  " + "\n  ".join(failures), file=sys.stderr)
        print("runner output:\n" + "\n".join(log), file=sys.stderr)
        return 1
    extra = f", browser read {browsed['b2'].get('url')} (reported available: {(ready.get('browser') or {}).get('available')})" if browser else ""
    print(f"e2e: ok (asked {len(asked)}, runner exited {exit_code} on stdin close{extra})")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1], browser="--browser" in sys.argv[2:]))
