#!/usr/bin/env python3
"""A scripted stand-in for OpenCode's ACP server.

It speaks the Agent Client Protocol as `opencode acp` does (JSON-RPC 2.0 over
stdio, one message per line), so the agent view can be developed and tested
without an account or any usage. What a turn does is chosen by words in the
message:

  edit      edit a file, asking first; allowed, it also tries to write the
            file through the client, as OpenCode does
  run       run a command, asking first
  todo      update its to-do list
  slow      stream slowly, so the turn can be stopped
  signin    fail as a provider without credentials does
  settings  answer with the mode and model the turn ran with
  (other)   think, then answer with a short Markdown reply

`session/list` offers two earlier sessions in any folder, `ses_earlier_one`
and `ses_earlier_two`, which can be continued; any other id cannot. The first
argument must be `acp`.
"""

import json
import os
import select
import sys
import time

DELAY = float(os.environ.get("FAKE_OPENCODE_DELAY", "0.02"))
EARLIER = {
    "ses_earlier_one": ("Fix the tab bar", "2026-09-26T10:00:00.000Z"),
    "ses_earlier_two": ("Add the drag picture", "2026-09-20T09:30:00.000Z"),
}

state = {
    "session": None,
    "mode": "build",
    "model": "opencode/big-pickle",
    "next_id": 0,
    "cancelled": False,
}
set_aside = []
_buffer = b""


def send(message):
    message["jsonrpc"] = "2.0"
    sys.stdout.write(json.dumps(message) + "\n")
    sys.stdout.flush()


def update(session_update, **fields):
    fields["sessionUpdate"] = session_update
    send({"method": "session/update",
          "params": {"sessionId": state["session"], "update": fields}})


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
    while True:
        line = read_line(block)
        if line is None:
            sys.exit(0)
        if line == "":
            return None
        if line.strip():
            return json.loads(line)


def watch(message):
    """Handles what may arrive while a turn runs; sets anything else aside."""
    if message.get("method") == "session/cancel":
        state["cancelled"] = True
    else:
        set_aside.append(message)


def pause():
    time.sleep(DELAY)
    while True:
        message = read_message(block=False)
        if message is None:
            return
        watch(message)


def ask(method, params):
    """Sends a request to the client and waits for its reply."""
    state["next_id"] += 1
    request_id = state["next_id"]
    send({"id": request_id, "method": method, "params": params})
    while True:
        message = read_message()
        if message.get("id") == request_id and "method" not in message:
            return message
        watch(message)


def options():
    return [
        {"id": "model", "name": "Model", "category": "model", "type": "select",
         "currentValue": state["model"],
         "options": [
             {"value": "opencode/big-pickle", "name": "OpenCode Zen/Big Pickle"},
             {"value": "zai/glm-4.7", "name": "Z.AI/GLM-4.7"},
         ]},
        {"id": "mode", "name": "Session Mode", "category": "mode", "type": "select",
         "currentValue": state["mode"],
         "options": [
             {"value": "build", "name": "build",
              "description": "The default agent. Executes tools based on configured permissions."},
             {"value": "plan", "name": "plan", "description": "Plan mode. Disallows all edit tools."},
         ]},
    ]


def commands():
    update("available_commands_update", availableCommands=[
        {"name": "init", "description": "Create or update AGENTS.md"},
        {"name": "review", "description": "Review changes"},
    ])


def say(text, message_id="msg_answer"):
    for piece in [text[i:i + 12] for i in range(0, len(text), 12)]:
        update("agent_message_chunk", messageId=message_id,
               content={"type": "text", "text": piece})
        pause()


def permission(call_id, kind, title, raw_input, content=None):
    tool_call = {"toolCallId": call_id, "title": title, "kind": kind, "rawInput": raw_input}
    if content:
        tool_call["content"] = content
    reply = ask("session/request_permission", {
        "sessionId": state["session"],
        "toolCall": tool_call,
        "options": [
            {"optionId": "once", "kind": "allow_once", "name": "Allow once"},
            {"optionId": "always", "kind": "allow_always", "name": "Always allow"},
            {"optionId": "reject", "kind": "reject_once", "name": "Reject"},
        ],
    })
    outcome = reply.get("result", {}).get("outcome", {})
    if outcome.get("outcome") != "selected":
        return None
    return outcome.get("optionId")


def run_turn(text):
    words = text.lower()
    if "signin" in words:
        return None, {"code": -32000,
                      "message": "Authentication required: provider authentication required",
                      "data": {"providerId": "zai"}}
    if "settings" in words:
        say(f"Mode {state['mode']}, model {state['model']}.")
        return "end_turn", None
    if "edit" in words:
        path = os.path.join(os.getcwd(), "notes.txt")
        raw = {"filePath": path, "oldString": "old line", "newString": "new line"}
        update("tool_call", toolCallId="call_edit", title="edit", kind="edit",
               status="pending", locations=[{"path": path}], rawInput=raw)
        chosen = permission("call_edit", "edit", "notes.txt",
                            {"filepath": path, "diff": "@@ -1 +1 @@\n-old line\n+new line\n"},
                            [{"type": "diff", "path": path,
                              "oldText": "old line\n", "newText": "new line\n"}])
        if chosen is None or chosen == "reject":
            update("tool_call_update", toolCallId="call_edit", status="failed",
                   content=[{"type": "content", "content": {
                       "type": "text", "text": "The user rejected this edit"}}])
            say("I left the file alone.")
            return "end_turn", None
        written = ask("fs/write_text_file", {"sessionId": state["session"], "path": path,
                                             "content": "new line\n"})
        refused = "error" in written
        update("tool_call_update", toolCallId="call_edit", status="completed",
               title="notes.txt",
               content=[{"type": "content", "content": {"type": "text", "text": "Edit applied."}},
                        {"type": "diff", "path": path, "oldText": "old line",
                         "newText": "new line"}],
               rawOutput={"output": "Edit applied."})
        say(f"Edited notes.txt ({chosen}; client write {'refused' if refused else 'accepted'}).")
        return "end_turn", None
    if "run" in words:
        raw = {"command": "cargo test", "description": "Runs the tests"}
        update("tool_call", toolCallId="call_run", title="cargo test", kind="execute",
               status="pending", rawInput=raw)
        chosen = permission("call_run", "execute", "cargo test", raw)
        if state["cancelled"]:
            return "cancelled", None
        if chosen in ("once", "always"):
            update("tool_call_update", toolCallId="call_run", status="completed",
                   content=[{"type": "content", "content": {
                       "type": "text", "text": "test result: ok. 3 passed"}}],
                   rawOutput={"output": "test result: ok. 3 passed"})
            say("The tests pass.")
        else:
            update("tool_call_update", toolCallId="call_run", status="failed",
                   rawOutput={"error": "The user rejected permission to use this tool"})
            say("I did not run them.")
        return "end_turn", None
    if "todo" in words:
        update("tool_call", toolCallId="call_todo", title="todowrite", kind="other",
               status="pending", rawInput={})
        update("tool_call_update", toolCallId="call_todo", status="completed",
               title="2 todos",
               rawInput={"todos": [
                   {"content": "Read the parser", "status": "completed", "priority": "high", "id": "1"},
                   {"content": "Fix the bug", "status": "in_progress", "priority": "high", "id": "2"},
               ]},
               content=[{"type": "content", "content": {"type": "text", "text": "2 todos"}}])
        say("Planned it.")
        return "end_turn", None
    if "slow" in words:
        for _ in range(400):
            if state["cancelled"]:
                return "cancelled", None
            update("agent_message_chunk", messageId="msg_slow",
                   content={"type": "text", "text": "."})
            time.sleep(0.02)
            pause()
        return "end_turn", None
    update("agent_thought_chunk", messageId="msg_answer",
           content={"type": "text", "text": "The person said hello."})
    pause()
    update("tool_call", toolCallId="call_read", title="read", kind="read", status="pending",
           rawInput={"filePath": os.path.join(os.getcwd(), "README.md")})
    update("tool_call_update", toolCallId="call_read", status="completed", title="README.md",
           content=[{"type": "content", "content": {"type": "text", "text": "# Readme"}}])
    say("Hello! This is **bold** from the fake OpenCode.")
    update("usage_update", used=64000, size=256000, cost={"amount": 0.02, "currency": "USD"})
    return "end_turn", None


def open_session(session_id):
    state["session"] = session_id
    return {"configOptions": options()}


def handle(message):
    method = message.get("method")
    request_id = message.get("id")
    params = message.get("params") or {}
    if request_id is None:
        return
    result, error = None, None
    if method == "initialize":
        result = {
            "protocolVersion": 1,
            "agentCapabilities": {
                "loadSession": True,
                "promptCapabilities": {"embeddedContext": True, "image": True},
                "sessionCapabilities": {"close": {}, "list": {}, "resume": {}},
            },
            "authMethods": [{"id": "opencode-login", "name": "Login with opencode",
                             "description": "Run `opencode auth login` in the terminal"}],
            "agentInfo": {"name": "OpenCode", "version": "0.0.0-fake"},
        }
    elif method == "session/list":
        result = {"sessions": [
            {"sessionId": sid, "cwd": params.get("cwd"), "title": title, "updatedAt": updated}
            for sid, (title, updated) in EARLIER.items()
        ]}
    elif method == "session/new":
        result = open_session("ses_fake_new")
        result["sessionId"] = "ses_fake_new"
    elif method == "session/resume":
        if params.get("sessionId") in EARLIER:
            result = open_session(params["sessionId"])
        else:
            error = {"code": -32602, "message": "Session not found"}
    elif method == "session/set_config_option":
        key = {"mode": "mode", "model": "model"}.get(params.get("configId"))
        if key:
            state[key] = params.get("value")
        result = {"configOptions": options()}
    elif method == "session/prompt":
        state["cancelled"] = False
        text = "".join(block.get("text", "") for block in params.get("prompt", []))
        stop, error = run_turn(text)
        if error is None:
            result = {"stopReason": stop}
    else:
        error = {"code": -32601, "message": f"Method not found: {method}"}
    reply = {"id": request_id}
    if error is not None:
        reply["error"] = error
    else:
        reply["result"] = result
    send(reply)
    # OpenCode lists its commands just after a session opens.
    if method in ("session/new", "session/resume") and error is None:
        commands()


def main():
    if sys.argv[1:2] != ["acp"]:
        sys.stderr.write("usage: fake_opencode.py acp\n")
        sys.exit(2)
    while True:
        message = set_aside.pop(0) if set_aside else read_message()
        handle(message)


if __name__ == "__main__":
    main()
