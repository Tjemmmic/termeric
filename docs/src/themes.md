# Themes and backgrounds

A theme sets the 16 ANSI colors programs use, the terminal's default text and background colors, and the colors of Termeric's own interface: tabs, menus, dialogs, selection, and search highlights. Termeric ships 26 themes, and you can add your own as small TOML files. You can also put a picture behind the terminal text.

## Built-in themes

The **root themes** are Termeric's own palettes, each named after a root or rhizome and drawn from its skin, flesh, and leaves. They keep the usual meaning of each ANSI color (red for errors, green for success), so programs look right. All twelve are dark.

| Name | Description |
| --- | --- |
| `turmeric` | The default. The skin, flesh, and leaves of the turmeric root. |
| `ginger` | Pale sandy tones, warm and understated. |
| `daikon` | Crisp whites and greens. |
| `malanga` | Grey-brown skin with purple splotches. |
| `beet` | Deep burgundy and vivid crimson. |
| `turnip` | Cream-to-purple, white flesh, sage leaves. |
| `yuca` | Bark-brown skin with bright white flesh. |
| `taro` | Dusty brown, pale lavender, purple veins. |
| `wasabi` | Bright green with chartreuse flesh. |
| `galangal` | Pinkish tan with hot pink shoots. |
| `horseradish` | Sandy tan with sharp white flesh. |
| `parsnip` | Cream skin and celery-green leaves. |

The **guest themes** are well-known community palettes, kept under their original names and authors.

| Name | Style |
| --- | --- |
| `catppuccin-latte` | Light |
| `catppuccin-mocha` | Dark |
| `dracula` | Dark |
| `everforest` | Dark |
| `gruvbox-dark` | Dark |
| `gruvbox-light` | Light |
| `kanagawa` | Dark |
| `nord` | Dark |
| `one-dark` | Dark |
| `one-light` | Light |
| `rose-pine` | Dark |
| `solarized-dark` | Dark |
| `solarized-light` | Light |
| `tokyo-night` | Dark |

## Choosing a theme

The quickest way is **Change Theme…** in the command palette (Ctrl+Shift+P). The list groups themes as Termeric, Guest, and Custom, and marks the one you are using. As you move through the list, the whole window previews each theme. Press Enter to keep the highlighted theme; Termeric saves the choice to `config.toml` right away. Press Escape to go back to the theme you had.

You can also pick a theme from the **Theme** dropdown on the Appearance page of Settings, which shows the theme's name, author, and description. Save Settings to keep it. Choosing a theme either way clears any `path` set in `[theme]`.

In `config.toml`, set the theme by name:

```toml
[theme]
name = "gruvbox-dark"
```

Or point at a theme file anywhere on disk:

```toml
[theme]
path = "/home/me/themes/my-theme.toml"
```

When `path` is set it wins over `name`. If a theme cannot be loaded at startup, Termeric uses `turmeric` and shows a notice.

A program running in the terminal can still change colors for its own terminal with OSC 4, 10, 11, and 12 (see [Terminal compatibility](compatibility.md)). Those changes last until the program resets them and do not affect other tabs.

## Custom themes

Put a theme file in `~/.config/termeric/themes/`. The file name without `.toml` becomes the theme's name, and it appears under Custom in **Change Theme…** and in the Settings dropdown. A custom theme with the same name as a built-in one replaces it.

Start from the [example theme](https://github.com/Tjemmmic/termeric/blob/main/assets/themes/custom-example.toml), or copy one of the built-in files from `termeric/themes/` in the source tree.

While a custom theme is in use, Termeric watches its file. Save an edit and the window updates within a second. If the edited file has an error, a notice explains it and the current colors stay.

### Theme file format

A theme has four tables. Every key is required unless marked optional.

```toml
[meta]
name = "My Theme"
author = "Your Name"
description = "Optional one-line description"

[primary]            # ANSI colors 0-7
black = "#1A1A1A"
red = "#FF5555"
green = "#50FA7B"
yellow = "#F1FA8C"
blue = "#BD93F9"
magenta = "#FF79C6"
cyan = "#8BE9FD"
white = "#F8F8F2"

[bright]             # ANSI colors 8-15
black = "#6272A4"
red = "#FF6E6E"
green = "#69FF94"
yellow = "#FFFFA5"
blue = "#D6ACFF"
magenta = "#FF92DF"
cyan = "#A4FFFF"
white = "#FFFFFF"

[ui]
background = "#282A36"
foreground = "#F8F8F2"
cursor = "#F8F8F2"
cursor_text = "#282A36"
selection = "#44475A"
# selection_text = "#FFFFFF"
border = "#44475A"
tab_active = "#BD93F9"
tab_inactive = "#44475A"
search_match = "#F1FA8C"
url = "#8BE9FD"
```

Write colors as `"#RRGGBB"` or `"#RRGGBBAA"` (with alpha), or as an inline table such as `{ r = 40, g = 42, b = 54 }` or `{ r = 40, g = 42, b = 54, a = 0.5 }`. Use the full six or eight hex digits: a shorter or misspelled value is read as black rather than reported as an error.

`[meta]` holds `name`, `author`, and an optional `description`, which Settings shows beside the theme.

The `[ui]` keys:

| Key | Used for |
| --- | --- |
| `background` | Terminal background, and the base for dialogs, the command palette, and Settings. |
| `foreground` | Default text color, and the text color of Termeric's interface. |
| `cursor` | The cursor, and the accent in the command palette. |
| `cursor_text` | Text drawn under a block cursor. |
| `selection` | Background of selected text. An alpha value makes it translucent. |
| `selection_text` | Optional. Color of selected text. Leave it out to keep each character's own color. |
| `border` | Tab separators, dividers, and dialog borders. |
| `tab_active` | The thin accent line on the active tab, and the accent color in Settings. |
| `tab_inactive` | The tab bar background. Raised tabs and menus are shaded from it toward `foreground`, so pick a color close to `background`. |
| `search_match` | Search highlights. Termeric applies its own transparency, lighter for other matches and stronger for the current one, so any alpha you give is ignored. |
| `url` | Hyperlinks and detected links. |

Termeric also uses the theme's yellow, red, and green for activity dots and agent status, so keep those recognizable.

## Background images

You can show a static PNG or JPEG behind the terminal text. The image covers the terminal area of the window (below the top bar and beside a left tab rail) and spans all panes. Text, cell colors, selections, and inline images are drawn on top of it. It is off by default.

Set it in `config.toml`:

```toml
[window.background_image]
path = "/home/me/Pictures/leaves.jpg"
opacity = 0.2
fit = "cover"
```

| Setting | Default | Description |
| --- | --- | --- |
| `path` | unset | PNG or JPEG file. A relative path is resolved beside `config.toml`. Remove it to turn the image off. |
| `opacity` | `0.2` | How strongly the image shows over the theme background, 0.0 to 1.0. Independent of `window.opacity`. |
| `fit` | `"cover"` | How the image fills the area (see below). |

The fit modes:

- `cover` fills the whole area, keeping the image's proportions and cropping the edges.
- `contain` shows the whole image, keeping its proportions, with the theme background around it.
- `stretch` fills the area exactly, distorting the image if needed.
- `center` shows the image at its own pixel size, centered and cropped.

The path can only be set in the file; Settings shows it on the Appearance page, where you can adjust **Background Image Opacity** and **Background Image Fit** live.

Termeric watches the image file and reloads it when it changes. If a new version cannot be used, a notice says why and the previous image stays. An image is refused if:

- it is not PNG or JPEG, or is an animated PNG,
- the file is larger than 16 MiB,
- either side is larger than 8192 pixels, or the decoded image needs more than 64 MiB.

A low opacity keeps text readable. Programs in the terminal cannot set or change the background image.
