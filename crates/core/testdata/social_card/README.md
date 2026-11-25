# Social card test fonts

Subsets of Noto Sans (wdth 100, wght 600) and Noto Sans SC (wght 600), both from
`github.com/google/fonts` (`ofl/notosans`, `ofl/notosanssc`), under the SIL Open Font
License 1.1 (`OFL.txt`; no Reserved Font Name). Built with
`scripts/fonts/build_card_font.py`, which also flattens GPOS pair kerning into a `kern`
table (the only kerning `ab_glyph` reads):

| File | `--unicodes` | Other flags |
| --- | --- | --- |
| `noto-sans-semibold-card.ttf` | `basic` | |
| `noto-ascii.ttf` | `ascii` | |
| `noto-ascii-no-ellipsis.ttf` | `ascii` | `--drop U+2026` |
| `noto-ascii-no-dots.ttf` | `ascii` | `--drop U+2026,U+2E` |
| `noto-sans-sc-subset.ttf` (from Noto Sans SC) | `ascii` | `--extra` with the test's CJK characters |
