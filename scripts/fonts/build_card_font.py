"""Builds a static social-card font with a legacy `kern` table.

Vyasa draws social cards with `ab_glyph`, which reads kerning from the
`kern` table only, not GPOS. Current OFL fonts ship GPOS kerning only, so
this script takes a variable OFL font, pins one instance, subsets it and
flattens its GPOS pair kerning into a format-0 `kern` table.

Usage (fontTools is the only requirement):

    uv run --with fonttools python scripts/fonts/build_card_font.py \
        NotoSans[wdth,wght].ttf out.ttf --wght 600 --unicodes basic

`--unicodes basic` keeps Latin, Latin-1, Latin Extended-A, Greek, Cyrillic
and general punctuation; `--unicodes ascii` keeps printable ASCII plus the
ellipsis; `--drop U+2026,...` removes code points (test fonts without an
ellipsis or a full stop). Pairs are capped so the table stays within one
format-0 subtable.
"""

from __future__ import annotations

import argparse
from collections import defaultdict

from fontTools import subset
from fontTools.ttLib import TTFont, newTable
from fontTools.ttLib.tables._k_e_r_n import KernTable_format_0
from fontTools.varLib import instancer

RANGES = {
    "ascii": [(0x20, 0x7E), (0x2026, 0x2026)],
    "basic": [
        (0x20, 0x7E),
        (0xA0, 0x17F),
        (0x370, 0x3FF),
        (0x400, 0x45F),
        (0x2010, 0x2027),
        (0x2030, 0x203A),
        (0x20AC, 0x20AC),
        (0x2122, 0x2122),
    ],
}
# One format-0 subtable holds at most 10920 pairs (its searchRange is a uint16).
MAX_PAIRS = 10_000


def pair_kerning(font: TTFont) -> dict[tuple[str, str], int]:
    """GPOS PairPos x-advance adjustments, first lookup wins per pair."""
    pairs: dict[tuple[str, str], int] = {}
    if "GPOS" not in font:
        return pairs
    for lookup in font["GPOS"].table.LookupList.Lookup:
        for sub in lookup.SubTable:
            if lookup.LookupType == 9:
                sub = sub.ExtSubTable
            if getattr(sub, "LookupType", lookup.LookupType) != 2:
                continue
            firsts = sub.Coverage.glyphs
            if sub.Format == 1:
                for first, pset in zip(firsts, sub.PairSet):
                    for rec in pset.PairValueRecord:
                        adv = getattr(rec.Value1, "XAdvance", 0) if rec.Value1 else 0
                        if adv:
                            pairs.setdefault((first, rec.SecondGlyph), adv)
            elif sub.Format == 2:
                class2_glyphs: dict[int, list[str]] = defaultdict(list)
                for glyph, cls in sub.ClassDef2.classDefs.items():
                    class2_glyphs[cls].append(glyph)
                for first in firsts:
                    c1 = sub.ClassDef1.classDefs.get(first, 0)
                    row = sub.Class1Record[c1]
                    for c2, rec in enumerate(row.Class2Record):
                        adv = getattr(rec.Value1, "XAdvance", 0) if rec.Value1 else 0
                        if not adv or c2 == 0:
                            continue
                        for second in class2_glyphs[c2]:
                            pairs.setdefault((first, second), adv)
    return pairs


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("src")
    ap.add_argument("dst")
    ap.add_argument("--wght", type=float, default=600)
    ap.add_argument("--wdth", type=float, default=100)
    ap.add_argument("--unicodes", choices=sorted(RANGES), default="basic")
    ap.add_argument("--drop", default="")
    ap.add_argument("--extra", default="", help="extra characters to keep")
    args = ap.parse_args()

    font = TTFont(args.src)
    if "fvar" in font:
        axes = {a.axisTag for a in font["fvar"].axes}
        pin = {k: v for k, v in (("wght", args.wght), ("wdth", args.wdth)) if k in axes}
        font = instancer.instantiateVariableFont(font, pin)

    keep = {cp for lo, hi in RANGES[args.unicodes] for cp in range(lo, hi + 1)}
    keep |= {ord(c) for c in args.extra}
    keep -= {int(u.removeprefix("U+"), 16) for u in args.drop.split(",") if u}
    opts = subset.Options()
    opts.layout_features = ["kern"]
    opts.name_IDs = ["*"]
    opts.notdef_outline = True
    sub = subset.Subsetter(opts)
    sub.populate(unicodes=sorted(keep))
    sub.subset(font)

    pairs = pair_kerning(font)
    if len(pairs) > MAX_PAIRS:
        pairs = dict(sorted(pairs.items(), key=lambda kv: -abs(kv[1]))[:MAX_PAIRS])
    kern = newTable("kern")
    kern.version = 0
    table = KernTable_format_0()
    table.version = 0
    table.coverage = 1
    table.format = 0
    table.kernTable = pairs
    kern.kernTables = [table]
    font["kern"] = kern
    font.save(args.dst)
    print(f"{args.dst}: {len(pairs)} kern pairs")


if __name__ == "__main__":
    main()
