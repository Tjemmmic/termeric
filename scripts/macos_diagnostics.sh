#!/bin/bash
# Gather what is needed to diagnose Termeric on a Mac into one text file.
#
# Usage: scripts/macos_diagnostics.sh [/path/to/Termeric.app] [output file]
#
# It records the macOS and hardware versions, the app's signature and
# quarantine state, what a Termeric shell's environment and terminfo lookup
# look like, and a debug log of one short launch (the window opens for about
# fifteen seconds and closes itself, with throwaway settings). Read the file
# before sharing it: the log can contain text from your terminal, and the
# environment section shows PATH, the locale, and your variables' names.

set -u
app=${1:-/Applications/Termeric.app}
out=${2:-$HOME/Desktop/termeric-diagnostics-$(date +%Y%m%d-%H%M%S).txt}
binary="$app/Contents/MacOS/termeric"

section() { printf '\n===== %s =====\n' "$1"; }

{
  section "System"
  sw_vers
  uname -a
  sysctl -n machdep.cpu.brand_string 2>/dev/null
  echo "Rosetta: $(/usr/bin/pgrep -q oahd && echo present || echo absent)"
  system_profiler SPDisplaysDataType 2>/dev/null | sed -n '1,40p'

  section "App"
  echo "App: $app"
  if [ ! -x "$binary" ]; then
    echo "No executable at $binary"
  else
    "$binary" --version
    file "$binary"
    /usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$app/Contents/Info.plist" 2>&1
    /usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$app/Contents/Info.plist" 2>&1
    codesign --display --verbose=2 "$app" 2>&1
    codesign --verify --deep --strict "$app" 2>&1 && echo "signature verifies"
    echo "Quarantine: $(xattr -p com.apple.quarantine "$app" 2>/dev/null || echo none)"
    case "$(cd "$app" && pwd -P)" in
      *AppTranslocation*) echo "Translocated: macOS runs a temporary copy; move the app to Applications" ;;
      *) echo "Not translocated" ;;
    esac
  fi

  section "Shell and environment"
  echo "Login shell: $(dscl . -read "/Users/$USER" UserShell 2>/dev/null | awk '{print $2}')"
  echo "SHELL=$SHELL"
  echo "LANG=${LANG-unset} LC_ALL=${LC_ALL-unset} LC_CTYPE=${LC_CTYPE-unset}"
  echo "TERM=${TERM-unset} TERM_PROGRAM=${TERM_PROGRAM-unset}"
  echo "PATH=$PATH"
  echo "Variable names (not values):"
  env | cut -d= -f1 | sort | tr '\n' ' '
  echo
  for tool in git brew python3 node claude codex opencode; do
    printf '%-9s %s\n' "$tool" "$(command -v "$tool" || echo 'not found')"
  done
  xcode-select -p 2>&1

  section "Terminfo"
  for directory in "$app/Contents/Resources/terminfo" "$HOME/.terminfo" /usr/share/terminfo; do
    ls -l "$directory/78/xterm-termeric" "$directory/x/xterm-termeric" 2>/dev/null
  done
  TERMINFO="$app/Contents/Resources/terminfo" /usr/bin/infocmp -x xterm-termeric 2>&1 | head -5
  TERMINFO="$app/Contents/Resources/terminfo" TERM=xterm-termeric /usr/bin/tput colors 2>&1

  section "Configuration and runtime directories"
  ls -la "${XDG_CONFIG_HOME:-$HOME/.config}/termeric" 2>&1
  ls -la "${XDG_DATA_HOME:-$HOME/.local/share}/termeric" 2>&1
  ls -la "$(getconf DARWIN_USER_DIR)termeric" 2>&1

  section "Running Termeric processes"
  pgrep -lf 'Termeric.app/Contents/MacOS/termeric' || echo none
  if [ -x "$binary" ]; then "$binary" control list 2>&1 | head -40; fi

  section "Debug log of one launch"
  # With throwaway configuration, data, and runtime directories, so your own
  # settings and saved windows are neither used nor changed.
  if [ -x "$binary" ]; then
    scratch=$(mktemp -d /tmp/termeric-diagnostics.XXXXXX)
    mkdir -m 700 "$scratch/run"
    XDG_CONFIG_HOME="$scratch/config" XDG_DATA_HOME="$scratch/data" XDG_RUNTIME_DIR="$scratch/run" \
      RUST_LOG=warn,termeric=debug "$binary" --separate-process 2>&1 &
    pid=$!
    sleep 15
    kill "$pid" 2>/dev/null
    wait "$pid" 2>/dev/null
    rm -rf "$scratch"
  fi
} > "$out" 2>&1

echo "Wrote $out"
