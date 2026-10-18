#!/usr/bin/env python3
"""Compile and exercise Termeric's version-controlled terminfo contract."""

from __future__ import annotations

import argparse
import fcntl
import os
from pathlib import Path
import pty
import select
import shutil
import struct
import subprocess
import sys
import tempfile
import termios
import time


ROOT = Path(__file__).resolve().parents[1]
SOURCE = ROOT / "assets" / "terminfo" / "xterm-termeric.terminfo"
CHECKED_DATABASE = ROOT / "assets" / "terminfo"
TERM = "xterm-termeric"


class CheckError(RuntimeError):
    pass


def require_tool(name: str) -> str:
    path = shutil.which(name)
    if path is None:
        raise CheckError(f"required tool is unavailable: {name}")
    return path


def run(command: list[str], env: dict[str, str]) -> subprocess.CompletedProcess[bytes]:
    result = subprocess.run(command, env=env, capture_output=True, timeout=15)
    if result.returncode != 0:
        raise CheckError(
            f"{' '.join(command)} failed ({result.returncode}): "
            f"{result.stderr.decode(errors='replace')}"
        )
    return result


def run_interactive(
    command: list[str], env: dict[str, str], input_bytes: bytes, label: str
) -> bytes:
    pid, descriptor = pty.fork()
    if pid == 0:
        os.execvpe(command[0], command, env)

    output = bytearray()
    fcntl.ioctl(descriptor, termios.TIOCSWINSZ, struct.pack("HHHH", 24, 80, 0, 0))
    deadline = time.monotonic() + 10
    sent = False
    status: int | None = None
    try:
        while time.monotonic() < deadline:
            if not sent:
                # Full-screen programs may reset terminal input during startup;
                # wait until their initial terminfo-driven screen setup settles.
                time.sleep(1.0)
                os.write(descriptor, input_bytes)
                sent = True
            readable, _, _ = select.select([descriptor], [], [], 0.1)
            if readable:
                try:
                    output.extend(os.read(descriptor, 65536))
                except OSError:
                    pass
            finished, status = os.waitpid(pid, os.WNOHANG)
            if finished:
                break
        else:
            os.kill(pid, 15)
            os.waitpid(pid, 0)
            tail = bytes(output[-500:]).decode(errors="replace")
            raise CheckError(f"{label} did not exit within 10 seconds; output tail: {tail!r}")
    finally:
        os.close(descriptor)

    assert status is not None
    if not os.WIFEXITED(status) or os.WEXITSTATUS(status) != 0:
        tail = bytes(output[-500:]).decode(errors="replace")
        raise CheckError(
            f"{label} exited abnormally with wait status {status}; output tail: {tail!r}"
        )
    return bytes(output)


def check_database(compiled: Path, env: dict[str, str]) -> None:
    infocmp = require_tool("infocmp")
    listing = run([infocmp, "-x", "-A", str(compiled), TERM], env).stdout.decode()
    checked_listing = run(
        [infocmp, "-x", "-A", str(CHECKED_DATABASE), TERM], env
    ).stdout.decode()
    normalized = lambda text: "\n".join(
        line for line in text.splitlines() if not line.startswith("#")
    )
    if normalized(listing) != normalized(checked_listing):
        raise CheckError(
            "checked compiled entry is stale; run "
            "tic -x -o assets/terminfo assets/terminfo/xterm-termeric.terminfo"
        )
    required = [
        "colors#0x100",
        "pairs#0x10000",
        "Rect,",
        "Tc",
        "setrgbf=",
        "setrgbb=",
        # Showing the cursor must leave its blinking to the configuration.
        "cnorm=\\E[?25h,",
        "smcup=\\E[?1049h",
        "rmcup=\\E[?1049l",
        "cbt=\\E[Z",
        "Clmg=\\E[s",
        "Cmg=\\E[%i%p1%d;%p2%ds",
        "Dsmg=\\E[?69l",
        "Enmg=\\E[?69h",
        "ech=\\E[%p1%dX",
        "rep=%p1%c\\E[%p2%{1}%-%db",
        "tbc=\\E[3g",
        "is2=\\E[!p",
        "rs2=\\E[!p",
        "u8=\\E[?1;2;4;8c",
        "Smulx=\\E[4:%p1%dm",
        "Setulc=\\E[58:2::%p1%{65536}%/%d:%p1%{256}%/%{255}%&%d:%p1%{255}%&%dm",
        "Ss=\\E[%p1%d q",
        "Se=\\E[ q",
        "Ms=\\E]52;%p1%s;%p2%s\\E\\\\",
        "Sync=\\E[?2026%?%p1%{1}%-%tl%eh%;",
        "BE=\\E[?2004h",
        "BD=\\E[?2004l",
        "PS=\\E[200~",
        "PE=\\E[201~",
        "fe=\\E[?1004h",
        "fd=\\E[?1004l",
        "kxIN=\\E[I",
        "kxOUT=\\E[O",
    ]
    forbidden = ["mc0=", "mc4=", "mc5="]
    for capability in required:
        if capability not in listing:
            raise CheckError(f"compiled entry is missing {capability}")
    for capability in forbidden:
        if capability in listing:
            raise CheckError(f"compiled entry overclaims {capability}")

    colors = run([require_tool("tput"), "colors"], env).stdout.strip()
    if colors != b"256":
        raise CheckError(f"tput reported {colors!r} colors instead of 256")
    cup = run([require_tool("tput"), "cup", "2", "3"], env).stdout
    if cup != b"\x1b[3;4H":
        raise CheckError(f"unexpected cup expansion: {cup!r}")
    expansions = {
        ("Rect",): b"",
        ("cbt",): b"\x1b[Z",
        ("Clmg",): b"\x1b[s",
        ("Cmg", "2", "7"): b"\x1b[3;8s",
        ("Dsmg",): b"\x1b[?69l",
        ("Enmg",): b"\x1b[?69h",
        ("ech", "3"): b"\x1b[3X",
        ("rep", "88", "4"): b"X\x1b[3b",
        ("is2",): b"\x1b[!p",
        ("rs2",): b"\x1b[!p",
        ("tbc",): b"\x1b[3g",
        ("Smulx", "3"): b"\x1b[4:3m",
        ("Setulc", str(0x123456)): b"\x1b[58:2::18:52:86m",
        ("Ss", "5"): b"\x1b[5 q",
        ("Se",): b"\x1b[ q",
        ("Ms", "c", "aGk="): b"\x1b]52;c;aGk=\x1b\\",
        ("Sync", "1"): b"\x1b[?2026h",
        ("Sync", "2"): b"\x1b[?2026l",
        ("BE",): b"\x1b[?2004h",
        ("BD",): b"\x1b[?2004l",
        ("PS",): b"\x1b[200~",
        ("PE",): b"\x1b[201~",
        ("fe",): b"\x1b[?1004h",
        ("fd",): b"\x1b[?1004l",
        ("kxIN",): b"\x1b[I",
        ("kxOUT",): b"\x1b[O",
    }
    for arguments, expected in expansions.items():
        actual = run([require_tool("tput"), *arguments], env).stdout
        if actual != expected:
            raise CheckError(
                f"unexpected {' '.join(arguments)} expansion: {actual!r}"
            )


def check_optional_apps(env: dict[str, str], require_apps: bool, temp: Path) -> None:
    env = dict(env)
    env.pop("TMUX", None)
    tmux_tmp = temp / "tmux"
    tmux_tmp.mkdir(mode=0o700)
    env["TMUX_TMPDIR"] = str(tmux_tmp)
    probes = [
        ("vim", ["vim", "-Nu", "NONE", "-i", "NONE", "-n"], b"\x1b:qa!\r"),
        ("less", ["less", "--no-init", str(temp / "less.txt")], b"q"),
        ("tmux", ["tmux", "-L", f"termeric-terminfo-{os.getpid()}", "-f", "/dev/null", "new-session", "/bin/sh"], b"exit\r"),
    ]
    (temp / "less.txt").write_text("terminfo acceptance\n" * 200, encoding="utf-8")

    for name, command, input_bytes in probes:
        path = shutil.which(name)
        if path is None:
            if require_apps:
                raise CheckError(f"required acceptance application is unavailable: {name}")
            print(f"skip {name}: executable unavailable")
            continue
        command[0] = path
        try:
            run_interactive(command, env, input_bytes, name)
        except CheckError as error:
            if not require_apps and "Operation not permitted" in str(error):
                print(f"skip {name}: local sandbox denies Unix sockets")
                continue
            raise
        print(f"ok {name}")


def check_environment_boundaries(temp: Path) -> None:
    """Exercise the database propagation boundaries documented for SSH/sudo."""
    infocmp = require_tool("infocmp")
    tic = require_tool("tic")
    tput = require_tool("tput")
    home = temp / "remote-home"
    empty_database = temp / "empty-database"
    home.mkdir()
    empty_database.mkdir()

    missing_env = dict(os.environ)
    missing_env.update(
        {
            "HOME": str(home),
            "TERM": TERM,
            "TERMINFO": str(empty_database),
            "TERMINFO_DIRS": str(empty_database),
        }
    )
    missing = subprocess.run(
        [infocmp, "-x", TERM],
        env=missing_env,
        capture_output=True,
        timeout=15,
    )
    if missing.returncode == 0:
        raise CheckError(
            "an isolated remote environment unexpectedly discovered xterm-termeric"
        )

    user_database = home / ".terminfo"
    run([tic, "-x", "-o", str(user_database), str(SOURCE)], dict(os.environ))
    installed_env = dict(os.environ)
    installed_env.update(
        {
            "HOME": str(home),
            "TERM": TERM,
            # An empty component retains system defaults after ~/.terminfo.
            "TERMINFO_DIRS": "",
        }
    )
    installed_env.pop("TERMINFO", None)
    run([infocmp, "-x", TERM], installed_env)
    if run([tput, "colors"], installed_env).stdout.strip() != b"256":
        raise CheckError("the per-user remote database did not expose 256 colors")

    compatibility_env = dict(os.environ)
    compatibility_env.update(
        {
            "HOME": str(temp / "sanitized-home"),
            "TERM": "xterm-256color",
            "TERMINFO_DIRS": "",
        }
    )
    compatibility_env.pop("TERMINFO", None)
    if run([tput, "colors"], compatibility_env).stdout.strip() != b"256":
        raise CheckError("the sudo/container compatibility identity is not discoverable")
    print("ok isolated SSH/sudo database boundaries")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--require-apps",
        action="store_true",
        help="fail instead of skipping when vim, less, or tmux is unavailable",
    )
    args = parser.parse_args()

    if not SOURCE.is_file():
        raise CheckError(f"terminfo source is missing: {SOURCE}")

    tic = require_tool("tic")
    # tic compiles an entry it only warns about, such as a %; without a %?.
    warnings = run([tic, "-c", "-x", str(SOURCE)], dict(os.environ)).stderr
    if warnings.strip():
        raise CheckError(f"tic -c warns about the entry: {warnings.decode(errors='replace')}")

    with tempfile.TemporaryDirectory(prefix="termeric-terminfo-") as directory:
        compiled = Path(directory) / "database"
        compiled.mkdir()
        run([tic, "-x", "-o", str(compiled), str(SOURCE)], dict(os.environ))
        env = dict(os.environ)
        env.update(
            {
                "TERM": TERM,
                "COLORTERM": "truecolor",
                "TERMINFO": str(compiled),
                "TERMINFO_DIRS": str(compiled),
            }
        )
        check_database(compiled, env)
        check_environment_boundaries(Path(directory))
        check_optional_apps(env, args.require_apps, Path(directory))

    print("terminfo acceptance passed")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (CheckError, subprocess.TimeoutExpired) as error:
        print(f"terminfo acceptance failed: {error}", file=sys.stderr)
        raise SystemExit(1) from error
