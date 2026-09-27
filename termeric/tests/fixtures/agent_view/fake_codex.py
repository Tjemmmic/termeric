#!/usr/bin/env python3
"""A scripted stand-in for Codex's app server.

It speaks the same protocol as `codex app-server` (JSON-RPC over stdio, one
message per line, no `jsonrpc` field), so the agent view can be developed and
tested without an account or any usage. What a turn does is chosen by words
in the message:

  edit      propose a patch and ask to apply it
  run       ask to run a command
  ask       ask a question
  todo      update the plan
  slow      stream slowly, so the turn can be stopped
  fail      end the turn with an error
  crash     exit with an error message
  settings  answer with the mode and model the turn ran with
  overrides answer with the `-c` configuration keys it was started with
  (other)   answer with a short Markdown reply

`thread/list` offers two earlier threads in any folder, `earlier-named` and
`earlier-preview`, which can be continued. `/compact` and `/review` arrive as
their own requests. `-c key=value`
overrides may come before `app-server`. Setting FAKE_CODEX_SIGNED_OUT reports
no account.
"""

import json
import os
import select
import sys
import time
import uuid

DELAY = float(os.environ.get("FAKE_CODEX_DELAY", "0.02"))
DEFAULT_MODEL = "gpt-fake-5"

threads = {}
overrides = {}
pending_input = []
state = {"thread": None, "turn": None, "interrupted": False, "notes": [], "next_id": 100}


def send(message):
    sys.stdout.write(json.dumps(message) + "\n")
    sys.stdout.flush()


def notify(method, params):
    send({"method": method, "params": params})


_buffer = b""


def read_line(block):
    """One input line; "" when none is ready without blocking; None at EOF."""
    global _buffer
    while b"\n" not in _buffer:
        if not block and not select.select([0], [], [], 0)[0]:
            return ""
        chunk = os.read(0, 65536)
        if not chunk:
            line, _buffer = _buffer, b""
            return line.decode() if line else None
        _buffer += chunk
    line, _, _buffer = _buffer.partition(b"\n")
    return line.decode()


def read_message(block=True):
    if pending_input:
        return pending_input.pop(0)
    while True:
        line = read_line(block)
        if line is None:
            sys.exit(0)
        if line == "":
            return None
        if line.strip():
            return json.loads(line)


def handle_during_turn(message):
    """Handles what may arrive while a turn runs; returns False otherwise."""
    method = message.get("method")
    if method == "turn/interrupt":
        state["interrupted"] = True
        send({"id": message["id"], "result": {}})
        return True
    if method == "turn/steer":
        if message["params"].get("expectedTurnId") != state["turn"]:
            send({"id": message["id"], "error": {"code": -32600, "message": "not the active turn"}})
            return True
        state["notes"].extend(part["text"] for part in message["params"]["input"])
        send({"id": message["id"], "result": {"turnId": state["turn"]}})
        return True
    return False


def pump():
    while True:
        message = read_message(block=False)
        if message is None:
            return
        if not handle_during_turn(message):
            pending_input.append(message)


def ask(method, params):
    """Sends a request to the client and waits for its answer."""
    state["next_id"] += 1
    request_id = state["next_id"]
    send({"id": request_id, "method": method, "params": params})
    while True:
        message = read_message()
        if message.get("id") == request_id and "method" not in message:
            notify("serverRequest/resolved", {"threadId": state["thread"], "requestId": request_id})
            return message.get("result", {})
        if not handle_during_turn(message):
            pending_input.append(message)
        if state["interrupted"]:
            notify("serverRequest/resolved", {"threadId": state["thread"], "requestId": request_id})
            return {"decision": "cancel"}


def base(item_id=None):
    params = {"threadId": state["thread"], "turnId": state["turn"]}
    if item_id:
        params["itemId"] = item_id
    return params


def item(kind, **fields):
    return dict({"type": kind, "id": "item_" + uuid.uuid4().hex[:8]}, **fields)


def started(entry):
    notify("item/started", dict(base(), item=entry, startedAtMs=0))


def completed(entry):
    notify("item/completed", dict(base(), item=entry, completedAtMs=0))


def say(text):
    """Streams an agent message; returns False when interrupted."""
    entry = item("agentMessage", text="", phase=None, memoryCitation=None, delivery=None,
                 questions=None)
    started(entry)
    for position, word in enumerate(text.split(" ")):
        notify("item/agentMessage/delta", dict(base(entry["id"]),
                                               delta=word if position == 0 else " " + word))
        time.sleep(DELAY)
        pump()
        if state["interrupted"]:
            return False
    entry["text"] = text
    entry["phase"] = "final_answer"
    completed(entry)
    return True


def think(text):
    entry = item("reasoning", summary=[], content=[])
    started(entry)
    notify("item/reasoning/summaryTextDelta", dict(base(entry["id"]), delta=text, summaryIndex=0))
    entry["summary"] = [text]
    completed(entry)


def command(text, output, approve=True):
    entry = item("commandExecution", command=f"/bin/bash -lc '{text}'", cwd=os.getcwd(),
                 processId=None, source="agent", status="inProgress",
                 commandActions=[{"type": "unknown", "command": text}], aggregatedOutput=None,
                 exitCode=None, durationMs=None)
    started(entry)
    decision = "accept"
    if approve:
        answer = ask("item/commandExecution/requestApproval", dict(
            base(entry["id"]), startedAtMs=0, kind="command", environmentId=None,
            reason="It runs the test suite", command=entry["command"], cwd=os.getcwd(),
            proposedExecpolicyAmendment=text.split(" ")[:2]))
        decision = answer.get("decision")
    if decision == "cancel":
        entry["status"] = "declined"
        completed(entry)
        return None
    if decision == "decline":
        entry["status"] = "declined"
        completed(entry)
        return False
    entry.update(status="completed", aggregatedOutput=output, exitCode=0, durationMs=5)
    completed(entry)
    return decision


def token_usage():
    notify("thread/tokenUsage/updated", dict(base(), tokenUsage={
        "total": {"totalTokens": 9100},
        "last": {"totalTokens": 64000, "inputTokens": 60000, "cachedInputTokens": 0,
                 "cacheWriteInputTokens": 0, "outputTokens": 4000, "reasoningOutputTokens": 0},
        "modelContextWindow": 256000}))


def finish(status="completed", error=None):
    turn = {"id": state["turn"], "items": [], "itemsView": "notLoaded", "status": status,
            "error": error, "startedAt": 0, "completedAt": 0, "durationMs": 1200}
    notify("turn/completed", {"threadId": state["thread"], "turn": turn})
    state["turn"] = None


def begin_turn(request_id):
    state["turn"] = "turn_" + uuid.uuid4().hex[:8]
    state["interrupted"] = False
    state["notes"] = []
    turn = {"id": state["turn"], "items": [], "itemsView": "notLoaded", "status": "inProgress",
            "error": None, "startedAt": 0, "completedAt": None, "durationMs": None}
    send({"id": request_id, "result": {"turn": turn}})
    notify("turn/started", {"threadId": state["thread"], "turn": turn})


def run_turn(params):
    thread = threads[state["thread"]]
    if "approvalPolicy" in params:
        thread["approval"] = params["approvalPolicy"]
    if "sandboxPolicy" in params:
        thread["sandbox"] = params["sandboxPolicy"]["type"]
    if params.get("model"):
        thread["model"] = params["model"]
    text = " ".join(part.get("text", "") for part in params["input"] if part["type"] == "text")
    skills = [part["name"] for part in params["input"] if part["type"] == "skill"]
    words = text.lower()

    if "crash" in words:
        sys.stderr.write("Error: the fake Codex crashed on purpose\n")
        sys.stderr.flush()
        sys.exit(2)

    if "fail" in words:
        say("Trying that now.")
        finish("failed", {"message": "The fake Codex failed on purpose.",
                          "codexErrorInfo": "usageLimitExceeded", "additionalDetails": None,
                          "misalignment": None})
        return

    if "slow" in words:
        if not say(" ".join(["This answer streams slowly so it can be stopped."] * 40)):
            finish("interrupted")
            return
        finish()
        return

    if "overrides" in words:
        say("overrides " + ", ".join(sorted(overrides)))
        finish()
        return

    if "settings" in words:
        say(f"sandbox {thread['sandbox']}, approval {thread['approval']}, "
            f"model {thread['model']}, skills {','.join(skills) or 'none'}")
        finish()
        return

    if "edit" in words:
        think("The answer is off by one.")
        path = os.path.join(os.getcwd(), "src/lib.rs")
        changes = [
            {"path": path, "kind": {"type": "update", "move_path": None},
             "diff": "@@ -1,3 +1,3 @@\n pub fn answer() -> u32 {\n-    41\n+    42\n }\n"},
            {"path": os.path.join(os.getcwd(), "NOTES.md"), "kind": {"type": "add"},
             "diff": "# Notes\nThe answer is 42.\n"},
        ]
        entry = item("fileChange", changes=changes, status="inProgress")
        started(entry)
        answer = ask("item/fileChange/requestApproval", dict(base(entry["id"]), startedAtMs=0,
                                                              reason=None, grantRoot=None))
        if answer.get("decision") in ("accept", "acceptForSession"):
            entry["status"] = "completed"
            completed(entry)
            say("Fixed: `answer()` now returns **42**.")
        else:
            entry["status"] = "declined"
            completed(entry)
            time.sleep(DELAY * 5)
            pump()
            if state["interrupted"] or answer.get("decision") == "cancel":
                finish("interrupted")
                return
            note = f" You said: {state['notes'][0]}" if state["notes"] else ""
            say("Understood, I left the files alone." + note)
        finish()
        return

    if "run" in words:
        outcome = command("cargo test -p termeric scrollbar",
                          "running 3 tests\ntest result: ok. 3 passed; 0 failed")
        if outcome is None:
            finish("interrupted")
            return
        if outcome:
            say(f"All three scrollbar tests pass ({outcome}).")
        else:
            say("I won't run it.")
        finish()
        return

    if "ask" in words:
        answer = ask("item/tool/requestUserInput", dict(base("item_q"), isBlocking=True,
                                                         autoResolutionMs=None, questions=[{
            "id": "approvals", "header": "Approvals",
            "question": "Which approval style should new sessions use?",
            "isOther": False, "isSecret": False, "options": [
                {"label": "Ask before changes", "description": "Safest"},
                {"label": "Auto", "description": "Faster"}]}]))
        chosen = answer.get("answers", {}).get("approvals", {}).get("answers", ["nothing"])
        say(f"Noted: **{chosen[0]}**.")
        finish()
        return

    if "todo" in words:
        notify("turn/plan/updated", dict(base(), explanation=None, plan=[
            {"step": "Read the code", "status": "completed"},
            {"step": "Write the fix", "status": "inProgress"},
            {"step": "Run the tests", "status": "pending"}]))
        say("Tracking three steps.")
        finish()
        return

    say("Here is a short answer with **bold**, `code`, and a list:\n\n- one\n- two")
    token_usage()
    finish()


def thread_result(thread_id):
    thread = threads[thread_id]
    return {"thread": {"id": thread_id, "sessionId": thread_id, "preview": "", "name": None,
                       "ephemeral": False, "status": {"type": "idle"}, "turns": []},
            "model": thread["model"], "modelProvider": "openai", "cwd": thread["cwd"],
            "approvalPolicy": thread["approval"], "sandbox": {"type": thread["sandbox"]},
            "reasoningEffort": "medium", "instructionSources": [], "disabledPluginIds": [],
            "serviceTier": None, "approvalsReviewer": "user"}


SANDBOX = {"read-only": "readOnly", "workspace-write": "workspaceWrite",
           "danger-full-access": "dangerFullAccess"}


def handle(message):
    method = message.get("method")
    params = message.get("params") or {}
    request_id = message.get("id")
    if method == "initialize":
        send({"id": request_id, "result": {"userAgent": "termeric/0.999.0 (fake)",
                                            "codexHome": "/nonexistent", "platformFamily": "unix",
                                            "platformOs": "linux"}})
    elif method == "initialized":
        pass
    elif method == "account/read":
        account = None if os.environ.get("FAKE_CODEX_SIGNED_OUT") else {
            "type": "chatgpt", "email": None, "planType": "pro"}
        send({"id": request_id, "result": {"account": account, "requiresOpenaiAuth": True}})
    elif method == "model/list":
        send({"id": request_id, "result": {"nextCursor": None, "data": [
            {"id": DEFAULT_MODEL, "model": DEFAULT_MODEL, "displayName": "GPT-Fake-5",
             "description": "The default fake model", "hidden": False, "isDefault": True},
            {"id": "gpt-fake-mini", "model": "gpt-fake-mini", "displayName": "GPT-Fake-Mini",
             "description": "A smaller fake model", "hidden": False, "isDefault": False},
            {"id": "gpt-hidden", "model": "gpt-hidden", "displayName": "Hidden",
             "description": "", "hidden": True, "isDefault": False}]}})
    elif method == "skills/list":
        send({"id": request_id, "result": {"data": [{"cwd": os.getcwd(), "errors": [], "skills": [
            {"name": "deploy", "description": "Deploy the app", "path": "/skills/deploy/SKILL.md",
             "scope": "repo", "enabled": True},
            {"name": "off", "description": "Disabled", "path": "/skills/off/SKILL.md",
             "scope": "repo", "enabled": False}]}]}})
    elif method == "thread/list":
        kinds = params.get("sourceKinds") or []
        threads_listed = [
            {"id": "earlier-named", "name": "Fix the flaky test", "preview": "the test fails",
             "cwd": params.get("cwd"), "updatedAt": 1790000000, "source": "cli"},
            {"id": "earlier-preview", "name": None, "preview": "\n  Tidy the README\nmore",
             "cwd": params.get("cwd"), "updatedAt": 1789000000, "source": "vscode"},
            {"id": "scripted", "name": "Nightly", "preview": "", "cwd": params.get("cwd"),
             "updatedAt": 1791000000, "source": "exec"},
        ]
        send({"id": request_id, "result": {"nextCursor": None, "data": [
            thread for thread in threads_listed if thread["source"] in kinds]}})
    elif method in ("thread/start", "thread/resume"):
        thread_id = params.get("threadId") if method == "thread/resume" else str(uuid.uuid4())
        if method == "thread/resume" and thread_id == "missing":
            send({"id": request_id, "error": {"code": -32600, "message": "no rollout found"}})
            return
        threads.setdefault(thread_id, {"model": DEFAULT_MODEL, "cwd": params.get("cwd", ""),
                                       "approval": params.get("approvalPolicy"),
                                       "sandbox": SANDBOX.get(params.get("sandbox"), "readOnly")})
        state["thread"] = thread_id
        send({"id": request_id, "result": thread_result(thread_id)})
        notify("thread/started", {"thread": thread_result(thread_id)["thread"]})
    elif method == "turn/start":
        if params.get("threadId") not in threads:
            send({"id": request_id, "error": {"code": -32600, "message": "thread not found"}})
            return
        begin_turn(request_id)
        run_turn(params)
    elif method == "thread/compact/start":
        send({"id": request_id, "result": {}})
        state["turn"] = "turn_" + uuid.uuid4().hex[:8]
        notify("turn/started", {"threadId": state["thread"], "turn": {"id": state["turn"]}})
        entry = item("contextCompaction")
        started(entry)
        completed(entry)
        finish()
    elif method == "review/start":
        begin_turn(request_id)
        target = params["target"]
        review = item("exitedReviewMode",
                      review=f"Reviewed {target.get('instructions', target['type'])}: looks good.")
        completed(review)
        finish()
    elif method in ("turn/interrupt", "turn/steer"):
        send({"id": request_id, "error": {"code": -32600, "message": "no active turn"}})
    elif request_id is not None and method is not None:
        send({"id": request_id, "error": {"code": -32601, "message": f"unknown {method}"}})


def main():
    args = sys.argv[1:]
    while len(args) >= 2 and args[0] == "-c":
        key, _, value = args[1].partition("=")
        overrides[key] = value
        args = args[2:]
    if args != ["app-server"]:
        sys.stderr.write("usage: fake_codex.py [-c key=value]... app-server\n")
        sys.exit(2)
    while True:
        message = read_message()
        handle(message)


if __name__ == "__main__":
    main()
