"""Builds the UI fonts of vr-radio from Noto Sans JP (SIL OFL 1.1).

Static instances at 400 / 500 / 700, subset to the characters of Windows-31J
(JIS X 0208 kanji, kana, symbols) plus Latin-1, renamed to "VR Radio Sans"
so that they never mix with a system-installed Noto Sans JP.

    python make_fonts.py <NotoSansJP variable font (.ttf)>

Source: https://github.com/google/fonts/tree/main/ofl/notosansjp
(also shipped by Windows as Fonts/NotoSansJP-VF.ttf). Needs fonttools.
"""

import sys
from pathlib import Path

from fontTools import subset
from fontTools.ttLib import TTFont
from fontTools.varLib import instancer

FAMILY = "VR Radio Sans"
WEIGHTS = {"Regular": 400, "Medium": 500, "Bold": 700}
OUT = Path(__file__).resolve().parent


def characters() -> list[int]:
    chars = set(range(0x20, 0x7F)) | set(range(0xA0, 0x180))
    for cp in range(0x80, 0x10000):
        try:
            chr(cp).encode("cp932")
        except UnicodeEncodeError:
            continue
        chars.add(cp)
    return sorted(chars)


def rename(font: TTFont, style: str) -> None:
    ribbi = style in ("Regular", "Bold")
    names = {
        1: FAMILY if ribbi else f"{FAMILY} {style}",
        2: style if ribbi else "Regular",
        3: f"{FAMILY.replace(' ', '')}-{style}",
        4: f"{FAMILY} {style}",
        6: f"{FAMILY.replace(' ', '')}-{style}",
        16: FAMILY,
        17: style,
    }
    table = font["name"]
    for record in list(table.names):
        if record.nameID in (1, 2, 3, 4, 6, 16, 17, 21, 22, 25):
            table.removeNames(nameID=record.nameID)
    for name_id, value in names.items():
        table.setName(value, name_id, 3, 1, 0x409)


def main(source: str) -> None:
    chars = characters()
    for style, weight in WEIGHTS.items():
        font = instancer.instantiateVariableFont(TTFont(source), {"wght": weight})
        options = subset.Options()
        options.layout_features = ["*"]
        options.name_IDs = ["*"]
        options.notdef_outline = True
        subsetter = subset.Subsetter(options)
        subsetter.populate(unicodes=chars)
        subsetter.subset(font)
        font["OS/2"].usWeightClass = weight
        rename(font, style)
        path = OUT / f"VRRadioSans-{style}.ttf"
        font.save(path)
        print(f"{path.name}: {path.stat().st_size // 1024} KiB")


if __name__ == "__main__":
    main(sys.argv[1])
