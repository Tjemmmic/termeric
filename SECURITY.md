# Security policy

## Reporting a vulnerability

Please report security problems privately through GitHub's private vulnerability reporting: open the repository's [Security tab](https://github.com/Tjemmmic/termeric/security) and choose **Report a vulnerability**. Don't open a public issue for them.

Include what you found, how to reproduce it, the Termeric version (`termeric --version`), and your desktop (Wayland or X11, and which compositor or window manager). You'll get a reply in the report, and a fix is published as a new release, with credit if you want it.

## Supported versions

Termeric is a developer preview. Only the latest 0.x preview receives fixes.

## Scope

Examples of what counts: escape sequences or file transfers that let a program in the terminal read or write files, run commands, or reach the clipboard without your consent; another user on the machine reaching Termeric's control socket or live-domain daemon; plugin permissions that can be bypassed; secrets leaking into saved workspaces, transcripts, or exports.
