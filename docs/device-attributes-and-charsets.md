# Device attributes and DEC character sets

Termeric implements a deliberately conservative VT100 identity and the three resident VT100 94-character sets. The identity is a compatibility response, not a claim that every historical VT100 electrical, printer, or display option exists.

## Device Attributes contract

| Request | Reply | Meaning |
|---|---|---|
| `CSI c` or `CSI 0 c` | `CSI ? 1 ; 2 ; 4 ; 8 c` | Primary DA: VT100 with advanced video, Sixel, and UDKs |
| `ESC Z` | `CSI ? 1 ; 2 ; 4 ; 8 c` | Obsolete DECID alias for Primary DA |
| `CSI > c` or `CSI > 0 c` | `CSI > 0 ; 1 ; 0 c` | Secondary DA: VT100 class, Termeric protocol revision 1, no ROM cartridge |

Requests with a nonzero parameter or an unrelated private/intermediate prefix are ignored. Replies are emitted as `TerminalEvent::PtyWrite` and consumed by the originating tab or pane before application-facing events are collected. Focus is never used to select a reply destination.

The Primary DA response uses xterm's VT100-with-advanced-video identity and adds parameter `4` for Termeric's documented static Sixel profile and parameter `8` for bounded DEC user-defined keys. It does not advertise 132 columns, printers, ReGIS, national replacement sets, locator hardware, or rectangular editing. The Secondary DA type `0` is xterm's VT100 identifier; the middle field is Termeric's protocol revision, not the package semantic version. See the [xterm control-sequence reference](https://invisible-island.net/xterm/ctlseqs/ctlseqs.html) and DEC's [VT102 Special Graphics table](https://manx-docs.org/mirror/vt100.net/docs/vt102-ug/table5-15.html).

## G0-G3 designation and invocation

Termeric starts with ASCII designated into G0 through G3, with G0 invoked into GL and G2 selected for the stored GR state. These resident 94-character-set designations are supported:

| Final | G0 `ESC (` | G1 `ESC )` | G2 `ESC *` | G3 `ESC +` |
|---|---|---|---|---|
| `B` | US ASCII | US ASCII | US ASCII | US ASCII |
| `A` | United Kingdom | United Kingdom | United Kingdom | United Kingdom |
| `0` | DEC Special Graphics | DEC Special Graphics | DEC Special Graphics | DEC Special Graphics |

SI (`0x0f`, LS0) invokes G0 into GL and SO (`0x0e`, LS1) invokes G1. `ESC n` (LS2) and `ESC o` (LS3) persistently invoke G2 or G3 into GL. `ESC N` (SS2) and `ESC O` (SS3) invoke G2 or G3 for exactly the next printable character; parser controls and status queries do not consume a pending single shift. `ESC ~`, `ESC }`, and `ESC |` store LS1R, LS2R, and LS3R invocation state for GR.

GL mapping occurs on printable ASCII bytes before grapheme storage, so the resulting Unicode glyphs participate in the normal width, edit, reflow, search, copy, and shaping paths. UTF-8 input is decoded as Unicode rather than as an 8-bit ISO-2022 byte stream, so non-ASCII scalars are not remapped through the stored GR register. GR is preserved and reportable for DEC state compatibility, not a claim of complete 8-bit character-set decoding.

The complete DEC Special Graphics mapping for bytes `_` through `~` is:

```text
_`abcdefghijklmnopqrstuvwxyz{|}~
 ◆▒␉␌␍␊°±␤␋┘┐┌└┼⎺⎻─⎼⎽├┤┴┬│≤≥π≠£·
```

Optional alternate-ROM designators (`1` and `2`), 96-character designations, later national replacement sets, and ISO-2022/UTF-8 mode-selection controls are not claimed. Unsupported designators leave the prior register unchanged.

## Save, reset, and alternate-screen behavior

DECSC/DECRC save and repeatably restore all four designations, GL/GR invocation, and a pending SS2/SS3 alongside cursor position, rendition, DECOM, delayed wrap, and DECSCA. CSI s/u and mode 1048 use the same state. Mode 1049 therefore isolates charset and other saved-state changes made on the alternate-screen path: leaving it restores the primary saved state. Plain 47/1047 transitions do not promise a save/restore; their terminal-global charset state continues, matching their no-cursor-save contract. RIS restores ASCII in G0 through G3, invokes G0 into GL and G2 into the stored GR state, clears pending single shift, and clears saved states; a subsequent unsaved DECRC uses home defaults.

## C1 control framing

Termeric accepts the supported DEC controls in both their 7-bit `ESC` form and their raw 8-bit C1 form: IND, NEL, HTS, RI, SS2, SS3, DCS, DECID, CSI, ST, OSC, and APC. A raw C1 byte is recognized only when it is not a continuation of an in-progress UTF-8 scalar. For example, the `0x9b` in UTF-8 `c4 9b` (`ě`) remains text, while a standalone `0x9b` introduces CSI.

`ESC SP F` (S7C1T) selects the default 7-bit form for terminal-generated replies. `ESC SP G` (S8C1T) selects single-byte CSI/DCS/OSC/APC introducers and ST terminators. The choice is terminal-local, affects core replies and origin-addressed clipboard/color replies, survives DECSTR, and returns to 7-bit on RIS. App-mediated queries capture the mode when parsed, so a later mode switch in the same PTY batch cannot change an earlier response.

Within ESC/CSI bodies and supported DCS/OSC/APC protocol fields, bytes in the GR range (`0xa0`-`0xff`) are treated as their 7-bit equivalents. This includes parameters, intermediates, final bytes, string data, and a high-bit `\\` in the two-byte `ESC \\` terminator. The exception is supported user-facing OSC text: window/icon titles, hyperlink URIs, and notification text retain their UTF-8 bytes. This mirrors the practical Unicode exception needed by a modern terminal while keeping ASCII-oriented control protocols ECMA-48-compatible. Unsupported C1 functions remain outside this claim.

## DEC conformance-level selection

`CSI 61-65 ; Pc \" p` (DECSCL) stores DEC operating level 1 through 5 and performs the same non-destructive soft reset as DECSTR. The screen, scrollback, tab stops, and C1 input acceptance survive. At levels 2 through 5, omitted `Pc`, `0`, and `2` select 8-bit replies while `1` selects 7-bit replies. Level 1 always uses 7-bit replies because its `Pc` is ignored. Invalid levels, subparameters, plural parameters, and invalid level-2-or-newer `Pc` values are rejected without changing state. RIS restores the startup level-4, 7-bit profile.

`DCS $ q \" p ST` (DECRQSS for DECSCL) reports the current level and C1 width, for example `DCS 1 $ r 64 ; 1 \" p ST` at startup. The reply itself uses the selected 7-bit or 8-bit framing.

This is a bounded negotiation profile, not strict historical emulation. Selecting a lower level does not disable newer controls Termeric implements and does not rewrite DA or terminfo identity. Applications that use DECSCL for its soft reset, C1 selection, and status report are covered; applications that expect a VT100/VT200 feature firewall are not.

## Presentation-state reports and restore

`CSI 1 $ w` (DECRQPSR) returns a DECCIR response in the selected C1 framing, containing the bounded cursor coordinates, sole page, DEC write rendition and protection, delayed-wrap and origin state, GL/GR registers, 94/96-set flags, and all four designations. `CSI 2 $ w` returns the current tab stops as a 1-based DECTABSR list. Report 0, unknown reports, plural parameters, subparameters, and unrelated intermediates are ignored.

The corresponding `DCS 1 $ t ... ST` and `DCS 2 $ t ... ST` DECRSPS forms restore cursor information or tab stops. Termeric validates an entire bounded payload before changing state: malformed fields, out-of-grid positions, unsupported designators, unsupported 96-character state, and malformed tab lists leave the terminal unchanged. Cursor restore changes only the historical DEC rendition subset, preserving unrelated modern attributes such as hidden and strikethrough. Multiple pages and extended reports are not implemented.

## Terminfo boundary

Termeric advertises `TERM=xterm-termeric` only when the compiled entry is discoverable. Its `smacs=ESC(0`, `rmacs=ESC(B`, `acsc` line-drawing pairs, `u9=CSI c`, and exact `u8=CSI ? 1 ; 2 ; 4 ; 8 c` response agree with this byte contract. If the entry is absent, or `TERMERIC_TERM=xterm-256color` requests rollback, the PTY uses the documented compatibility identity instead. DA1/DA2 remain a bounded VT identity and do not independently imply either terminfo's full capability surface. See [the terminfo contract](terminfo.md) and [inline-image contract](inline-images.md).

## Verification

Deterministic conformance fixtures cover byte-split DA/DECID requests, the complete graphics repertoire, designation and invocation across G0-G3, and a DECRQPSR/DECRSPS cursor-and-tab round trip. Unit tests cover invalid DA parameters, UK mapping, single and locking shifts, strict atomic state restore, RIS, DECSC/DECRC, and 1049 restoration. A real-PTY integration test runs simultaneous DSR, DA1, and DA2 probes from two panes in an inactive tab and verifies that both complete without leaking a reply to the focused tab. The headless GPU Unicode lane queues and submits every mapped DEC glyph through the real font atlas and text pipeline.
