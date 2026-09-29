# Images and file transfer

Termeric can show pictures inside the terminal, move files between a remote shell and your computer through the terminal itself, and accept files you drag onto the window. This page covers what works and how to try each one.

## Inline images

Programs can draw images in the terminal using any of three common protocols. Termeric supports the parts of each that image viewers and command-line tools use in practice.

| Protocol | What works |
| --- | --- |
| Kitty graphics | PNG and raw RGB/RGBA images, compressed or not, sent through the terminal, from a temporary file, or from shared memory; placing, cropping, resizing, layering above or below text, and deleting images; Kitty's frame animation; Unicode placeholders |
| Sixel | Standard Sixel images with their color palettes, transparent backgrounds, and pixel aspect ratios |
| iTerm2 (OSC 1337) | Inline PNG images, sized automatically or in cells, pixels, or percent |

Images scroll with the text, stay inside their pane, and are cleared along with the text when a program clears the screen. Images drawn by a full-screen program disappear when it exits. Each terminal keeps up to 256 images (64 MiB of image data), dropping the least recently used first.

Images aren't saved in [workspaces](workspaces-and-sessions.md) or tab snapshots. The iTerm2 protocol accepts only still PNG images; other formats, including GIF and animated PNG, show nothing, and its file-download mode (`inline=0`) isn't supported.

### Try it

With [kitty](https://sw.kovidgoyal.net/kitty/)'s `kitten` tool installed:

```sh
kitten icat photo.png
kitten icat animation.gif
```

`kitten icat` handles JPEG, GIF, and other formats by converting them before sending. Over `ssh` it sends image data through the terminal, which Termeric supports.

With [libsixel](https://github.com/saitoha/libsixel):

```sh
img2sixel photo.png
```

With [timg](https://timg.sh), choose the protocol explicitly:

```sh
timg -pk photo.jpg   # Kitty graphics
timg -ps photo.jpg   # Sixel
timg -pi photo.jpg   # iTerm2
```

iTerm2's `imgcat` script also works, but only for PNG files, because it sends the file unchanged.

### Images from local files

A Kitty graphics program can ask the terminal to read an image straight from a file path on your computer (`kitten icat --transfer-mode=file photo.png`). Because any program writing to the terminal could use this to read your files, Termeric refuses it unless you allow it:

```toml
[security]
allow_kitty_file_transfers = true
```

Leave it off unless you trust every program you run in Termeric. Normal image display, including temporary-file and shared-memory transfers, works without it. The setting has nothing to do with the file transfers below.

## File transfer with kitten transfer

Kitty's file-transfer protocol lets a shell on another machine send files to your computer, or fetch files from it, through the terminal connection. You don't need `scp` or a second login. Termeric asks you before every transfer.

Run these in a shell on the remote machine, with `kitten` installed there:

```sh
# Copy a file from the remote machine to your computer
kitten transfer report.pdf Downloads/report.pdf

# Copy a file from your computer to the remote machine
kitten transfer --direction=upload Documents/notes.txt notes.txt
```

Paths on your computer that don't start with `/` are relative to your home directory there, so `Downloads/report.pdf` lands in `~/Downloads` on your computer. If you write a leading `~`, quote it (`'~/Downloads/report.pdf'`); unquoted, the remote shell expands it to the remote home directory before `kitten` sees it.

When a transfer starts, Termeric shows a dialog naming the terminal that asked:

- **Allow remote file transfer?** appears for files coming to your computer. Choose **Allow transfer** (A) or **Deny transfer** (D). Files, directories, and links can be received, and existing files are never replaced.
- **Allow terminal to read host files?** appears for files leaving your computer and lists every requested path. Choose **Allow these reads once** (A) or **Deny** (D). Only regular files can be sent; directories and links are refused.

Only allow a transfer you started yourself. A toast shows progress, and the transfer stops if you close the tab or pane that started it.

Limits:

- up to 64 MiB per transfer;
- files coming to your computer: up to 1,024 files and directories per transfer;
- files leaving your computer: up to 16 files per transfer, without recursing into directories;
- `kitten transfer`'s delta mode (`--transmit-deltas`) and its permission-bypass password are not supported.

File transfer works in local tabs and in tabs attached to a [live domain](live-domains.md).

## Drag and drop files

Drag a file or folder from your file manager onto the Termeric window to type its path at the cursor of the focused pane. Termeric quotes the path so spaces and special characters are safe in the shell, adds a space after it, and does not press Enter. Drop several files to insert several paths.

Nothing is read or uploaded; only the path is typed. The pointer shows whether the focused pane accepts the drop, and Termeric shows a toast when it doesn't. A drop is refused:

- in a tab Termeric opened for a coding agent or plugin, so a local path isn't sent to it by accident;
- in a [serial](profiles.md#serial-profiles) tab;
- in a read-only pane, or while broadcast input is on;
- while a panel, search, copy mode, or another input mode is open.

### Uploading to a live domain

Dropping a file or folder on a pane attached to a [live domain](live-domains.md) uploads it instead. Termeric asks you to confirm, showing the source, the destination pane, and the name the file will get in that pane's current directory. Press **U** to upload or **C** to cancel.

The upload runs in the background with a progress toast; run **Cancel Upload** from the command palette to stop it. Uploads are limited to 64 MiB, an existing file or folder of the same name is never replaced, and when the upload finishes the remote path is typed into the pane without pressing Enter. See [Live domains](live-domains.md) for scripted uploads with `termeric domain upload`.
