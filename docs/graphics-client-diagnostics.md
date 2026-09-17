# Graphics client diagnostics

These are producer-side diagnostics, not terminal acceptance or visual tests.
The [native graphics lane](gui-acceptance.md#standalone-wayland-graphics-clients)
must still pass with unmodified clients. A client emitting no bytes cannot test
the terminal's image decoder or renderer.

## img2sixel exits successfully but emits nothing

### Observed environment and result

On 2026-09-16, the local Arch packages were `libsixel 1.10.5-1`,
`gdk-pixbuf2 2.44.7-1`, and `glycin 2.1.5-2`. `img2sixel --version` reported
GdkPixbuf support enabled and native libpng support disabled.

The clean `3688c33` private-Wayland graphics run attempted all seven clients.
Six emitted data and had matching terminal acceptance logs; `img2sixel` exited
zero with zero stdout and stderr bytes for the generated 64×64 RGBA PNG.
The overall run correctly failed, before screenshot capture. Its local artifacts
are under `/tmp/termeric-native-graphics-3688c33`; these temporary files are not
durable release evidence. This failure is distinct from the earlier Kitty
placement/scrolling reports and does not explain or resolve those visual bugs.

### Cause

The optional [GdkPixbuf probe](../termeric/tests/gui/gdk_pixbuf_probe.c) found
that direct file decoding succeeds, but the streaming loader supplies its image
only after closing the input:

```text
formats=28
file: ok=1 image=yes prepared=0 size=64x64
write: ok=1 image=no prepared=0
close: ok=1 image=yes prepared=1 size=64x64
```

In [libsixel v1.10.5's loader](https://github.com/libsixel/libsixel/blob/v1.10.5/src/loader.c),
the GdkPixbuf path writes the input and requests the animation/pixbuf **before**
closing the loader. A null pixbuf jumps to cleanup, which closes the loader too
late to deliver a frame. The status still reflects successful frame allocation,
so this path can return success without output. The write and close errors are
also ignored. GdkPixbuf documents that
[closing the loader finishes parsing remaining data](https://docs.gtk.org/gdk-pixbuf/method.PixbufLoader.close.html).

A process-local differential experiment closed the loader before libsixel's
first image lookup and avoided a second close during cleanup. The same installed
client and PNG then emitted 8,388 bytes beginning with a Sixel DCS, with exit zero
and empty stderr. The unmodified control still emitted zero bytes. This is strong
evidence for a producer/library call-order incompatibility, not evidence that
Termeric rendered those bytes correctly.

The experiment used a temporary `LD_PRELOAD` shim, not an installed replacement.
It was not used by the acceptance runner and is **not a supported workaround**:
one static PNG does not validate animations, all error paths, or loader lifetime.
Do not add it to shell startup, the terminal, or release tests.

### Reproduce without changing packages

Run from the repository root in a normal local shell. The optional probe needs a
C compiler, `pkg-config`, and GdkPixbuf development headers/libraries; these were
already available on the diagnosed host. It is not part of the Rust build and
does not require any provider or AI service. Use a small, trusted fixture, not an
untrusted image: the probe reads the whole file and is not a resource-limit test.

```bash
pacman -Q libsixel gdk-pixbuf2 glycin
img2sixel --version
pkg-config --modversion gdk-pixbuf-2.0

# Substitute the PNG retained by your native graphics run.
fixture=/path/to/graphics-client-PID.png
probe_dir="$(mktemp -d /tmp/termeric-gdk-probe.XXXXXX)"
cc -Wall -Wextra -Werror termeric/tests/gui/gdk_pixbuf_probe.c \
  -o "$probe_dir/gdk-pixbuf-probe" $(pkg-config --cflags --libs gdk-pixbuf-2.0)
"$probe_dir/gdk-pixbuf-probe" "$fixture"

img2sixel "$fixture" > "$probe_dir/output.six" 2> "$probe_dir/stderr.log"
client_status=$?
printf 'img2sixel status=%s\n' "$client_status"
wc -c "$probe_dir/output.six" "$probe_dir/stderr.log"
```

The probe exits zero when direct and completed streaming decoding both succeed,
one on decoding/read failure, and two on incorrect usage. An absent pixbuf after
`write` alone is not a probe failure: the producer must finish the input first.
Keep the output files for diagnosis; the commands do not display image control
sequences or alter installed libraries.

A comments-only legacy `loaders.cache` is not proof that PNG decoding is missing:
this host exposes 28 formats and successfully decodes through Glycin. Restricted
agent/container execution instead produced a D-Bus Unix-socket `Operation not
permitted` error. That is a separate sandbox restriction. Glycin uses
[peer-to-peer D-Bus and sandboxed decoder processes](https://github.com/GNOME/glycin).
Run the probe in an appropriately permitted local environment; **do not disable
Glycin's own sandbox** or regenerate system caches on the assumption that this
is a missing-format problem.

### Resolution and acceptance boundary

The appropriate upstream/distro fix is in libsixel's GdkPixbuf loading path:
finish loading and handle write/close errors before inspecting the decoded
image, preserve animation behavior and ownership, and return failure when no
frame is delivered. A tested rebuild/update is preferable to a preload shim.
No newer fixed package has been verified here, and no package was installed,
downgraded, or replaced during this diagnosis.

After a supported unmodified client emits a complete Sixel frame, rerun the
full native graphics lane against a clean candidate. All seven clients must
satisfy the existing oracle; do not skip img2sixel, substitute shimmed output, or
promote the six successful clients to an overall pass. Initial visibility,
placement, scrolling, and clipping still need the deferred visual review.
The [Sixel/iTerm2 support status](feature-support.md) remains **Partial**.

For matching a later bug report to this diagnosis, the fixture SHA-256 was
`b816fdeb08edae3ec293de3a74cde3209961756ef5014f5cb409f6ba6b1ace73`;
`/usr/bin/img2sixel` was
`80cbb86c2781761d333189a9371f0cd92763c7f7745317b2220374b8206abded`.
