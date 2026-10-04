#!/bin/sh
# Install Termeric from this directory, or uninstall a previous installation.
#
# MANIFEST beside this script lists the files to install and their modes. An
# installation records the files it wrote in PREFIX/lib/termeric/installed-files
# for --uninstall. With --destdir the files are staged for a package manager,
# which tracks them itself, so nothing is recorded and no cache is refreshed.

set -eu

usage() {
    cat <<'EOF'
Usage: install.sh [--prefix DIR] [--destdir DIR] [--uninstall]

  --prefix DIR   Install under DIR (default: ~/.local)
  --destdir DIR  Stage the files under DIR/PREFIX for a package
  --uninstall    Remove the files a previous install under PREFIX recorded
  -h, --help     Show this help
EOF
}

fail() {
    printf 'install.sh: %s\n' "$1" >&2
    exit 1
}

prefix=""
destdir=""
uninstall=false
while [ "$#" -gt 0 ]; do
    case "$1" in
        --prefix)
            [ "$#" -ge 2 ] || fail "--prefix needs a directory"
            prefix=$2
            shift 2
            ;;
        --prefix=*)
            prefix=${1#--prefix=}
            shift
            ;;
        --destdir)
            [ "$#" -ge 2 ] || fail "--destdir needs a directory"
            destdir=$2
            shift 2
            ;;
        --destdir=*)
            destdir=${1#--destdir=}
            shift
            ;;
        --uninstall)
            uninstall=true
            shift
            ;;
        -h | --help)
            usage
            exit 0
            ;;
        *)
            usage >&2
            fail "unknown option: $1"
            ;;
    esac
done

if [ -z "$prefix" ]; then
    [ -n "${HOME:-}" ] || fail "HOME is not set; pass --prefix"
    prefix="$HOME/.local"
fi
# One spelling for every recorded path.
while [ "$prefix" != "${prefix%/}" ]; do
    prefix=${prefix%/}
done
case "$prefix" in
    /?*) ;;
    *) fail "--prefix must be an absolute directory other than /" ;;
esac
# The prefix goes into desktop entries and sed replacements; refuse the
# characters either would interpret.
case "$prefix" in
    *[\"\`\$\\%\|\&]* | *'
'*)
        fail "--prefix contains a character a desktop entry can't hold: $prefix"
        ;;
esac
if [ -n "$destdir" ]; then
    case "$destdir" in
        /*) ;;
        *) fail "--destdir must be an absolute directory" ;;
    esac
    while [ "$destdir" != "${destdir%/}" ]; do
        destdir=${destdir%/}
    done
fi

root="$destdir$prefix"
record="$prefix/lib/termeric/installed-files"
here=$(cd "$(dirname "$0")" && pwd)

refresh_caches() {
    if command -v update-desktop-database >/dev/null 2>&1 && [ -d "$prefix/share/applications" ]; then
        update-desktop-database -q "$prefix/share/applications" || true
    fi
    # Refresh only an icon cache that already exists: a cache created here
    # would hide icons that later installs add without refreshing it.
    if command -v gtk-update-icon-cache >/dev/null 2>&1 && [ -f "$prefix/share/icons/hicolor/icon-theme.cache" ]; then
        gtk-update-icon-cache -q -t -f "$prefix/share/icons/hicolor" || true
    fi
}

# Remove each recorded file, and each directory that leaves empty, up to the
# prefix's own top-level directories such as bin and share.
remove_recorded() {
    while IFS= read -r path; do
        case "$path" in
            "$prefix"/*) ;;
            *) fail "refusing to remove $path, which is outside $prefix" ;;
        esac
        rm -f "$path"
        directory=$(dirname "$path")
        while [ "$(dirname "$directory")" != "$prefix" ] && rmdir "$directory" 2>/dev/null; do
            directory=$(dirname "$directory")
        done
    done <"$record"
    rm -f "$record"
    rmdir "$prefix/lib/termeric" 2>/dev/null || true
}

if [ "$uninstall" = true ]; then
    [ -z "$destdir" ] || fail "--uninstall can't be combined with --destdir"
    [ -f "$record" ] || fail "no Termeric installation is recorded in $record"
    remove_recorded
    refresh_caches
    printf 'Uninstalled Termeric from %s\n' "$prefix"
    exit 0
fi

[ -f "$here/MANIFEST" ] || fail "MANIFEST is missing beside install.sh"

pending=""
if [ -z "$destdir" ]; then
    if [ -f "$record" ]; then
        # An upgrade: remove the previous files first, so any the new version
        # no longer ships don't linger.
        remove_recorded
    fi
    mkdir -p "$(dirname "$record")"
    pending="$record.new"
    : >"$pending"
fi

binary="$prefix/bin/termeric"
while read -r mode path; do
    source="$here/$path"
    target="$root/$path"
    [ -f "$source" ] || fail "MANIFEST lists $path, which is missing"
    mkdir -p "$(dirname "$target")"
    # Write beside the target and rename over it, so a running Termeric
    # keeps its executable and a failure never leaves half a file.
    temporary="$target.install-tmp"
    case "$path" in
        share/applications/*.desktop)
            if [ "$prefix" = /usr ]; then
                cp "$source" "$temporary"
            else
                # Launchers don't always search PREFIX/bin, so name the
                # binary by its full path.
                sed -e "s|^Exec=termeric\$|Exec=\"$binary\"|" \
                    -e "s|^Exec=termeric |Exec=\"$binary\" |" \
                    "$source" >"$temporary"
            fi
            ;;
        *)
            cp "$source" "$temporary"
            ;;
    esac
    chmod "$mode" "$temporary"
    mv -f "$temporary" "$target"
    if [ -n "$pending" ]; then
        printf '%s\n' "$prefix/$path" >>"$pending"
    fi
done <"$here/MANIFEST"

if [ -z "$pending" ]; then
    printf 'Staged Termeric for %s in %s\n' "$prefix" "$root"
    exit 0
fi

mv -f "$pending" "$record"
refresh_caches
printf 'Installed Termeric in %s\n' "$prefix"
case ":${PATH:-}:" in
    *":$prefix/bin:"*) ;;
    *) printf 'Add %s to PATH to run termeric from a shell.\n' "$prefix/bin" ;;
esac
case "$prefix" in
    /usr | /usr/local) ;;
    *)
        printf 'For Zsh completions, add this to ~/.zshrc before compinit:\n'
        printf "  fpath=('%s' \$fpath)\n" "$prefix/share/zsh/site-functions"
        ;;
esac
