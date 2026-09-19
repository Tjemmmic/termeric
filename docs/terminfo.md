# Terminfo and terminal identity

Termeric has one native capability name: `xterm-termeric`. The source of truth
is [`assets/terminfo/xterm-termeric.terminfo`](../assets/terminfo/xterm-termeric.terminfo).
It describes the implemented 256-color/truecolor, cursor/editing, alternate
screen, application-key, mouse, and VT100 character-set surface without
claiming unsupported printer, synchronized-output terminfo extensions, or
advanced graphics capabilities. Classic `ech`, `rep`, `cbt`, and `tbc`
capabilities are advertised because their Unicode/wide-cell and tab-stop
behavior is covered by the core conformance suite. The extended `Enmg`, `Dsmg`,
`Clmg`, and `Cmg` capabilities expose DECLRMM/DECSLRM using the vocabulary tmux
recognizes; deterministic tests cover combined vertical/horizontal scrolling,
editing, wrapping, cursor motion, wide cells, BCE, and image placement. The
extended `Rect` boolean advertises the independently tested DECFRA rectangle
fill path to tmux; the other rectangular controls have no corresponding tmux
terminfo feature flag.
`bce` claim is backed by
ED/EL/ECH/ICH/DCH tests that preserve the cursor and populate erased/inserted
cells with the active background color, plus SU/SD/IL/DL and implicit
LF/IND/NEL/RI/wrap scrolling tests. Newly exposed cells carry only the active
background, not text attributes, protection, or hyperlink identity. Default,
indexed, and truecolor identities survive full/partial margins and both
screens; colored blanks retain their background in history, and synchronized
output keeps them hidden until release. This matches the
[ncurses BCE scrolling contract](https://invisible-island.net/ncurses/ncurses.faq.html).
The default-background path retains its no-extra-fill-pass behavior; colored
terminal scrolling on the alternate screen also has a zero-allocation rail.
ED 3 clears saved lines without destroying the viewport.
`is2` and `rs2` use the non-destructive DECSTR soft
reset, whose reset-versus-preserve contract is independently fixture-tested;
they do not clear the screen, scrollback, or custom tab stops.

XTGETTCAP (`DCS + q ... ST`) exposes the same bounded contract without reading
the host's terminal database. One request may contain at most 64 hexadecimal
names and receives one response; processing stops at the first malformed or
unknown name. `TN`/`name` reports the exact identity exported to that child,
including `xterm-256color` in compatibility mode. `Co`/`colors`, `RGB`, the Fish `indn` and
`query-os-name` extensions, and every special-key string listed in the source
entry are supported, including their xterm termcap aliases. Replies honor the
terminal-local S7C1T/S8C1T preference. XTSETTCAP, arbitrary host-database
queries, and capabilities absent from the native entry are deliberately not
claimed.

## Runtime negotiation

At every PTY spawn Termeric searches for the compiled entry in, in order:

1. `TERMERIC_TERMINFO_DIR`, then `TERMINFO`;
2. `~/.terminfo`, `$XDG_DATA_HOME/terminfo`, and `TERMINFO_DIRS`;
3. the executable prefix's `share/terminfo` directory;
4. conventional Unix system databases.

Both the character directory (`x/xterm-termeric`) and hexadecimal directory
(`78/xterm-termeric`) layouts are recognized. If found, the child receives
`TERM=xterm-termeric`, `COLORTERM=truecolor`, `TERMINFO` pinned to the exact
discovered root, and a `TERMINFO_DIRS` path that retains inherited or operating
system fallback databases. Pinning the root prevents a parent `TERMINFO` value
from outranking `TERMERIC_TERMINFO_DIR` after the custom identity has been
selected. If no entry is found, the child gets `TERM=xterm-256color`; Termeric
never exports an undiscoverable custom name.

The startup override `TERMERIC_TERM=xterm-256color` (or `compatibility`) forces
the rollback path. `TERMERIC_TERM=auto` is the default. Explicitly requesting
`xterm-termeric` still falls back safely and logs a warning when the entry is
missing. Other values are rejected rather than passed through.

```bash
TERMERIC_TERM=xterm-256color termeric
```

`xterm-256color` is a practical compatibility identity, not a statement that
Termeric implements every xterm extension. Use it only for a host or tool that
cannot install the native entry.

## Installation

The Debian package installs the compiled entry into
`/usr/share/terminfo/x/xterm-termeric`. A source install should compile it next
to the executable installation:

```bash
sudo tic -x -o /usr/local/share/terminfo \
  assets/terminfo/xterm-termeric.terminfo
```

A non-root per-user install is:

```bash
tic -x -o "$HOME/.terminfo" assets/terminfo/xterm-termeric.terminfo
infocmp -x xterm-termeric
```

For a relocatable bundle, compile under its `share/terminfo` directory or set
`TERMERIC_TERMINFO_DIR` to that directory before launch.

## SSH, sudo, and containers

SSH forwards `TERM` for interactive sessions but normally does not forward a
local terminfo database path. Install the same source on the remote host before
connecting:

```bash
scp assets/terminfo/xterm-termeric.terminfo host:/tmp/
ssh host 'mkdir -p "$HOME/.terminfo" && tic -x -o "$HOME/.terminfo" /tmp/xterm-termeric.terminfo'
ssh host 'infocmp -x xterm-termeric >/dev/null && printf "remote terminfo ok\n"'
```

Until then, override only the SSH process:

```bash
TERM=xterm-256color ssh host
```

`sudo` commonly preserves `TERM` while discarding user database variables. A
system package install needs no exception. For a user-only entry, either ask
the administrator to install it system-wide or use:

```bash
sudo env TERM=xterm-256color command
```

Container images need the compiled entry too. Copy the source into the image
and run `tic -x`, install the Termeric package's entry, or pass
`TERM=xterm-256color` as an explicit compatibility choice. Do not copy a Linux
compiled entry blindly to a platform with a different terminfo binary format;
compile the version-controlled source on the target.

## Verification and maintenance

Run the automated contract locally with:

```bash
python3 scripts/check_terminfo.py --require-apps
```

It requires `tic`, `infocmp`, `tput`, Vim, less, and tmux. The test compiles a
fresh database, asserts required and deliberately absent capabilities, expands
cursor addressing and horizontal-margin controls, and launches all three
full-screen tools on real PTYs. An isolated environment fixture also proves
that forwarding the custom name without its database fails, a per-user remote
installation restores discovery, and the documented `xterm-256color`
compatibility identity survives sudo/container-style removal of custom
database paths. It does not contact an SSH host or invoke sudo; real host and
administrator policy remain release-environment checks. The
Linux GUI acceptance lane separately verifies the negotiated environment while
driving those applications through Termeric. Debian CI extracts the package
and loads its installed entry with `infocmp`.

Pure environment-selection tests additionally prove that the custom identity
cannot be selected without a compiled entry and that its discovered database
root overrides a conflicting inherited `TERMINFO` while preserving ordered
fallback directories.

After any capability change, update the source, the DA contract if identity
changes, the support manifest, and these docs together. Recompile the checked
Linux package asset with:

```bash
tic -x -o assets/terminfo assets/terminfo/xterm-termeric.terminfo
```
