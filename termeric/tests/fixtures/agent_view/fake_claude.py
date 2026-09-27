#!/usr/bin/env python3
"""A scripted stand-in for Claude Code's stream-json protocol.

It speaks the same protocol as `claude -p --input-format stream-json
--output-format stream-json --permission-prompt-tool stdio`, so the agent view
can be developed and tested without an account or any usage. What it does is
chosen by words in each prompt:

  edit      read a file, then ask to edit it (approval with a diff)
  run       ask to run a command
  ask       ask a multiple-choice question
  plan      present a plan for approval
  todo      update a to-do list
  agent     run a sub-agent
  slow      stream slowly, so the turn can be interrupted
  fail      end the turn with an error
  crash     exit with an error message
  (other)   answer with a short Markdown reply

Pass `--version` to print a version line.
"""

import json
import os
import select
import sys
import time
import uuid

VERSION = "2.1.999"
SESSION = os.environ.get("FAKE_CLAUDE_SESSION", str(uuid.uuid4()))
DELAY = float(os.environ.get("FAKE_CLAUDE_DELAY", "0.02"))

mode = "default"
model = "claude-sonnet-5"
for index, argument in enumerate(sys.argv):
    if argument == "--permission-mode" and index + 1 < len(sys.argv):
        mode = sys.argv[index + 1]
    if argument == "--model" and index + 1 < len(sys.argv):
        model = sys.argv[index + 1]
    if argument == "--resume" and index + 1 < len(sys.argv):
        SESSION = sys.argv[index + 1]

# Claude Code switches to bypassPermissions only when launched allowing it.
ALLOW_BYPASS = "--allow-dangerously-skip-permissions" in sys.argv

pending_input = []
interrupted = False


def send(message):
    sys.stdout.write(json.dumps(message) + "\n")
    sys.stdout.flush()


_buffer = b""


def read_line(block):
    """One input line; "" when none is ready without blocking; None at EOF.

    Reads the descriptor directly: mixing a buffered reader with select()
    would miss lines already sitting in the buffer.
    """
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


def read_message():
    if pending_input:
        return pending_input.pop(0)
    while True:
        line = read_line(True)
        if line is None:
            return None
        if line.strip():
            return json.loads(line)


def handle_control(message):
    """Answers a control request; returns False if it was not one."""
    global mode, model, interrupted
    if message.get("type") != "control_request":
        return False
    request = message["request"]
    subtype = request.get("subtype")
    body = {}
    if subtype == "initialize":
        body = {
            "models": [
                {"value": "default", "displayName": "Default (recommended)",
                 "description": "Opus 5.5 · Best for everyday, complex tasks"},
                {"value": "sonnet", "displayName": "Sonnet 5",
                 "description": "Most efficient for everyday tasks"},
                {"value": "haiku", "displayName": "Haiku 4.5",
                 "description": "Fastest for quick answers"},
            ],
            "commands": [
                {"name": "compact", "description": "Summarize the conversation", "argumentHint": ""},
                {"name": "context", "description": "Show current context usage", "argumentHint": ""},
                {"name": "review", "description": "Review a pull request", "argumentHint": "<pr>"},
            ],
            "current_permission_mode": mode,
        }
    elif subtype == "set_permission_mode":
        if request["mode"] == "bypassPermissions" and not ALLOW_BYPASS:
            send({"type": "control_response", "response": {
                "subtype": "error", "request_id": message["request_id"],
                "error": "Cannot set permission mode to bypassPermissions because the session "
                         "was not launched with --dangerously-skip-permissions",
                "error_code": "bypass_not_launched"}})
            return True
        mode = request["mode"]
        send({"type": "system", "subtype": "status", "status": None,
              "permissionMode": mode, "session_id": SESSION, "uuid": str(uuid.uuid4())})
    elif subtype == "set_model":
        model = request.get("model") or model
    elif subtype == "get_context_usage":
        body = {"totalTokens": 48000, "maxTokens": 200000, "categories": []}
    elif subtype == "interrupt":
        interrupted = True
    send({"type": "control_response", "response": {
        "subtype": "success", "request_id": message["request_id"], "response": body}})
    return True


def pump_controls():
    """Handles control requests that arrived while the turn was running."""
    while True:
        line = read_line(False)
        if not line:
            return
        message = json.loads(line)
        if not handle_control(message):
            pending_input.append(message)


def stream_text(text, message_id):
    send({"type": "stream_event", "parent_tool_use_id": None, "session_id": SESSION,
          "event": {"type": "message_start", "message": {"id": message_id}}})
    send({"type": "stream_event", "parent_tool_use_id": None, "session_id": SESSION,
          "event": {"type": "content_block_start", "index": 0,
                    "content_block": {"type": "text", "text": ""}}})
    words = text.split(" ")
    for position, word in enumerate(words):
        piece = word if position == 0 else " " + word
        send({"type": "stream_event", "parent_tool_use_id": None, "session_id": SESSION,
              "event": {"type": "content_block_delta", "index": 0,
                        "delta": {"type": "text_delta", "text": piece}}})
        time.sleep(DELAY)
        pump_controls()
        if interrupted:
            return False
    send({"type": "stream_event", "parent_tool_use_id": None, "session_id": SESSION,
          "event": {"type": "content_block_stop", "index": 0}})
    send({"type": "assistant", "parent_tool_use_id": None, "session_id": SESSION,
          "message": {"id": message_id, "role": "assistant",
                      "content": [{"type": "text", "text": text}]}})
    return True


def tool_use(name, tool_input, parent=None):
    tool_id = "toolu_" + uuid.uuid4().hex[:12]
    send({"type": "assistant", "parent_tool_use_id": parent, "session_id": SESSION,
          "message": {"id": "msg_" + uuid.uuid4().hex[:8], "role": "assistant",
                      "content": [{"type": "tool_use", "id": tool_id, "name": name,
                                   "input": tool_input}]}})
    return tool_id


def tool_result(tool_id, content, is_error=False, detail=None, parent=None):
    message = {"type": "user", "parent_tool_use_id": parent, "session_id": SESSION,
               "message": {"role": "user", "content": [
                   {"type": "tool_result", "tool_use_id": tool_id, "content": content,
                    "is_error": is_error}]}}
    if detail is not None:
        message["tool_use_result"] = detail
    send(message)


def ask(tool_id, name, tool_input, suggestions=None, description=None):
    """Asks the host for permission and waits for its answer."""
    request_id = "req_" + uuid.uuid4().hex[:8]
    request = {"subtype": "can_use_tool", "tool_name": name, "input": tool_input,
               "tool_use_id": tool_id}
    if suggestions:
        request["permission_suggestions"] = suggestions
    if description:
        request["description"] = description
    send({"type": "control_request", "request_id": request_id, "request": request})
    while True:
        message = read_message()
        if message is None:
            sys.exit(0)
        if message.get("type") == "control_response" and \
                message["response"].get("request_id") == request_id:
            return message["response"].get("response", {})
        if not handle_control(message):
            pending_input.append(message)
        if interrupted:
            send({"type": "control_cancel_request", "request_id": request_id})
            return {"behavior": "deny", "message": "interrupted"}


def finish(started, success=True, error=None):
    result = {"type": "result", "subtype": "success" if success else "error_during_execution",
              "is_error": not success, "duration_ms": int((time.time() - started) * 1000),
              "num_turns": 1, "total_cost_usd": 0.0123, "session_id": SESSION,
              "permission_denials": [], "result": "" if success else (error or "failed"),
              "uuid": str(uuid.uuid4())}
    if error:
        result["errors"] = [error]
    send(result)


def turn(prompt):
    global interrupted
    interrupted = False
    started = time.time()
    send({"type": "system", "subtype": "init", "session_id": SESSION, "model": model,
          "cwd": os.getcwd(), "permissionMode": mode, "claude_code_version": VERSION,
          "tools": ["Read", "Edit", "Bash"], "uuid": str(uuid.uuid4())})
    words = prompt.lower()
    message_id = "msg_" + uuid.uuid4().hex[:8]

    if "crash" in words:
        sys.stderr.write("Error: the fake agent crashed on purpose\n")
        sys.stderr.flush()
        sys.exit(2)

    if "fail" in words:
        stream_text("Trying that now.", message_id)
        finish(started, success=False, error="The fake agent failed on purpose")
        return

    if "slow" in words:
        text = " ".join(["This answer streams slowly so it can be stopped."] * 40)
        if not stream_text(text, message_id):
            finish(started, success=False, error="Interrupted")
            return
        finish(started)
        return

    if "edit" in words:
        stream_text("I'll check the file first.", message_id)
        read = tool_use("Read", {"file_path": os.path.join(os.getcwd(), "src/lib.rs")})
        tool_result(read, "1\tpub fn answer() -> u32 {\n2\t    41\n3\t}")
        path = os.path.join(os.getcwd(), "src/lib.rs")
        edit_input = {"file_path": path, "old_string": "    41", "new_string": "    42"}
        edit = tool_use("Edit", edit_input)
        answer = ask(edit, "Edit", edit_input, suggestions=[
            {"type": "setMode", "mode": "acceptEdits", "destination": "session"}])
        if answer.get("behavior") == "allow":
            tool_result(edit, "The file has been updated.", detail={
                "filePath": path, "oldString": "    41", "newString": "    42",
                "structuredPatch": [{"oldStart": 1, "oldLines": 3, "newStart": 1, "newLines": 3,
                                     "lines": [" pub fn answer() -> u32 {", "-    41", "+    42", " }"]}]})
            stream_text("Fixed: `answer()` now returns **42**.", "msg_" + uuid.uuid4().hex[:8])
        else:
            tool_result(edit, answer.get("message", "denied"), is_error=True)
            stream_text("Understood, I left the file alone.", "msg_" + uuid.uuid4().hex[:8])
        finish(started)
        return

    if "run" in words:
        command = {"command": "cargo test -p termeric scrollbar",
                   "description": "Run the scrollbar tests"}
        bash = tool_use("Bash", command)
        answer = ask(bash, "Bash", command, description="Run the scrollbar tests", suggestions=[
            {"type": "addRules", "behavior": "allow", "destination": "localSettings",
             "rules": [{"toolName": "Bash", "ruleContent": "cargo test:*"}]}])
        if answer.get("behavior") == "allow":
            for step in range(3):
                time.sleep(DELAY * 5)
                pump_controls()
            tool_result(bash, "running 3 tests\ntest a ... ok\ntest b ... ok\ntest c ... ok\n\n"
                              "test result: ok. 3 passed; 0 failed")
            stream_text("All three scrollbar tests pass.", "msg_" + uuid.uuid4().hex[:8])
        else:
            tool_result(bash, answer.get("message", "denied"), is_error=True)
            stream_text("I won't run it.", "msg_" + uuid.uuid4().hex[:8])
        finish(started)
        return

    if "ask" in words:
        question = {"questions": [{
            "question": "Which approval style should new sessions use?", "header": "Approvals",
            "multiSelect": False, "options": [
                {"label": "Ask before edits", "description": "Safest; you approve each change"},
                {"label": "Edit without asking", "description": "Faster; commands still ask"}]}]}
        question_id = tool_use("AskUserQuestion", question)
        answer = ask(question_id, "AskUserQuestion", question)
        chosen = answer.get("updatedInput", {}).get("answers", {})
        tool_result(question_id, json.dumps(chosen))
        choice = next(iter(chosen.values()), "nothing")
        stream_text(f"Noted: **{choice}**.", "msg_" + uuid.uuid4().hex[:8])
        finish(started)
        return

    if "plan" in words:
        plan = ("## Codex driver\n\n1. **Driver trait.** One interface for both agents.\n"
                "2. **App server.** Map thread and turn events to the same cards.\n"
                "3. **Version check.** Refuse unknown versions.")
        plan_id = tool_use("ExitPlanMode", {"plan": plan})
        answer = ask(plan_id, "ExitPlanMode", {"plan": plan})
        if answer.get("behavior") == "allow":
            tool_result(plan_id, "User has approved your plan.")
            stream_text("Starting on step 1.", "msg_" + uuid.uuid4().hex[:8])
        else:
            tool_result(plan_id, answer.get("message", "Keep planning"), is_error=True)
            stream_text("I'll revise the plan.", "msg_" + uuid.uuid4().hex[:8])
        finish(started)
        return

    if "todo" in words:
        todos = {"todos": [
            {"content": "Read the code", "status": "completed", "activeForm": "Reading the code"},
            {"content": "Write the fix", "status": "in_progress", "activeForm": "Writing the fix"},
            {"content": "Run the tests", "status": "pending", "activeForm": "Running the tests"}]}
        todo_id = tool_use("TodoWrite", todos)
        tool_result(todo_id, "Todos have been modified successfully.")
        stream_text("Tracking three steps.", message_id)
        finish(started)
        return

    if "agent" in words:
        task_id = tool_use("Agent", {"description": "Find callers", "prompt": "Find callers",
                                     "subagent_type": "Explore"})
        for name, tool_input in [("Grep", {"pattern": "refresh_focus"}),
                                 ("Read", {"file_path": os.path.join(os.getcwd(), "src/focus.rs")})]:
            child = tool_use(name, tool_input, parent=task_id)
            time.sleep(DELAY * 5)
            tool_result(child, "ok", parent=task_id)
        tool_result(task_id, "Found 2 callers.")
        stream_text("The sub-agent found two callers.", message_id)
        finish(started)
        return

    reply = ("Here is a short answer with **bold**, `code`, and a list:\n\n"
             "- one\n- two\n\n```rust\nfn main() {}\n```")
    if not stream_text(reply, message_id):
        finish(started, success=False, error="Interrupted")
        return
    finish(started)


def main():
    if "--version" in sys.argv:
        print(f"{VERSION} (Claude Code)")
        return
    while True:
        message = read_message()
        if message is None:
            return
        if handle_control(message):
            continue
        if message.get("type") == "user":
            content = message["message"]["content"]
            text = content if isinstance(content, str) else " ".join(
                part.get("text", "") for part in content if part.get("type") == "text")
            turn(text)


if __name__ == "__main__":
    main()
