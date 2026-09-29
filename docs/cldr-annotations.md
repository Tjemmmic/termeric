# CLDR annotation index

Termeric's Unicode picker bundles a generated, English-only character and emoji annotation index. CLDR supplies human short names and search keywords; Unicode Emoji data supplies the exact fully-qualified presentation sequence inserted into the terminal. The application does not read locale files, contact a network service, or build a CLDR heap index at runtime.

## Pinned inputs

The generator accepts only these exact upstream files:

| Purpose | Versioned source | SHA-256 |
|---|---|---|
| English base annotations | [CLDR JSON 48.0.0 `annotations/en/annotations.json`](https://raw.githubusercontent.com/unicode-org/cldr-json/48.0.0/cldr-json/cldr-annotations-full/annotations/en/annotations.json) | `f22083cb86dffb63a643d5bacb5d9899f82d2fa5d388ad4f3aed72184acef505` |
| English derived annotations | [CLDR JSON 48.0.0 `annotationsDerived/en/annotations.json`](https://raw.githubusercontent.com/unicode-org/cldr-json/48.0.0/cldr-json/cldr-annotations-derived-full/annotationsDerived/en/annotations.json) | `15c9457afa703a32dcc78d54626418a91e820fe0ddd67b7d491c4c4d9f5846ab` |
| Fully-qualified presentation sequences | [Unicode Emoji 17.0 `emoji-test.txt`](https://www.unicode.org/Public/17.0.0/emoji/emoji-test.txt) | `1d8a944f88d7952f7ef7c5167fef3c67995bcae24543949710231b03a201acda` |

The CLDR Git blob identities are respectively `d27057edd3207303fa451efc15ffbbdf633e6a74` and `188fa714011f7fbc54ed1e618316904754fd9ff5`. Both CLDR inputs are needed: the base file covers ordinary characters and explicitly named sequences, while the derived file supplies flags, modifiers, and most composed sequences.

CLDR intentionally removes emoji/text variation selectors from annotation keys. The generator strips selectors when joining against Emoji 17.0, then stores the matching `fully-qualified` or `component` sequence. This keeps, for example, `❤️`, `👩🏾‍⚖️`, and `🏳️‍⚧️` as exact insertion bytes rather than silently changing them to text-presentation forms.

## Regeneration

Download into disposable paths, then run the checked generator:

```bash
curl -fL \
  https://raw.githubusercontent.com/unicode-org/cldr-json/48.0.0/cldr-json/cldr-annotations-full/annotations/en/annotations.json \
  -o /tmp/termeric-cldr-48-base.json
curl -fL \
  https://raw.githubusercontent.com/unicode-org/cldr-json/48.0.0/cldr-json/cldr-annotations-derived-full/annotationsDerived/en/annotations.json \
  -o /tmp/termeric-cldr-48-derived.json
curl -fL \
  https://www.unicode.org/Public/17.0.0/emoji/emoji-test.txt \
  -o /tmp/termeric-emoji-17-test.txt

python3 scripts/generate_cldr_annotations.py \
  --base /tmp/termeric-cldr-48-base.json \
  --derived /tmp/termeric-cldr-48-derived.json \
  --emoji /tmp/termeric-emoji-17-test.txt
python3 scripts/generate_cldr_annotations.py \
  --base /tmp/termeric-cldr-48-base.json \
  --derived /tmp/termeric-cldr-48-derived.json \
  --emoji /tmp/termeric-emoji-17-test.txt \
  --check
```

Generation rejects an input hash mismatch before parsing. It also rejects duplicate JSON keys, base/derived sequence overlap, unknown record fields, missing or repeated short names, duplicate keywords, controls, surrogates, oversized fields, conflicting presentation mappings, and every record/pool/ artifact cap violation. Output order is the exact Unicode-code-point order of the inserted grapheme and is deterministic for the pinned inputs.

The generated `cldr_annotations.bin` format is deliberately small and private:

- a 16-byte little-endian header contains `TRMCLD01`, record count, and pool byte count;
- each fixed 16-byte record stores four offsets for text, short name, normalized search terms, and end;
- one trailing UTF-8 pool owns all variable data.

Runtime loading verifies the header, exact 4,342-record count, total length, offset contiguity/order/UTF-8 boundaries, sorted uniqueness, absence of control characters, and exactly one Unicode 17 extended grapheme per insertion. The complete immutable artifact is 475,510 bytes against a 768 KiB ceiling. It is referenced with `include_bytes!`, so the process does not duplicate it into a heap catalog.

## Search and resource policy

Short names and keywords are normalized into ordered, unique search terms. All query terms must match by whole term, token prefix, or token substring. Exact scalar syntax remains a separate one-result path. Curated exact names, Unicode canonical names/aliases, and exact CLDR short names retain explicit ranking above partial annotation matches. Merging canonical, annotation, and curated candidates deduplicates exact UTF-8 graphemes before retaining at most 256 executable rows.

The final result vector is shrunk and measured across candidate/action/command storage before entering the palette; its hard owned-memory ceiling is 1 MiB. Release benchmarks record five fresh structural validations, five warm end-to-end keyword queries, the complete static artifact size, and each query's owned result bytes. The initial broad rails are 25 ms p95, 75 ms p95, 768 KiB, and 1 MiB respectively.

Only English (`en`) annotations are bundled. This is natural-language emoji discovery, not general UI localization, locale-sensitive collation, fuzzy stemming, or a promise that the configured fonts contain every returned grapheme. Additional locales require a separate selection, fallback, binary- size, memory, accessibility, and update contract.

## Licensing

CLDR and Unicode Emoji data are used under the Unicode License v3. The complete notice is retained in [`THIRD_PARTY_NOTICES.md`](../termeric/THIRD_PARTY_NOTICES.md) and is installed with packaged documentation.
