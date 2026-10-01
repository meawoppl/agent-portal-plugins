# Newstroke font provenance

Research for the T3 schematic renderer (see `docs/javascript-triage.md`), done
before any glyph data enters kicadmium. Nothing below has been imported yet.

## Primary source

- **Author:** Vladimir Uryvaev ("vovanium"), <vovanius@bk.ru>.
- **Project page:** <https://vovanium.ru/sledy/newstroke/en> (en),
  <https://vovanium.ru/sledy/newstroke> (ru). The page says: "It is released
  under CC0 licence which is effectively equivalent to public domain so you can
  do with it whatever you want." It shows the release dated 2015/11/19.
- **Release archive:** <https://vovanium.ru/_media/sledy/newstroke/newstroke-font.tgz>,
  gzip timestamp 2015-11-18, SHA-256
  `dba1c334834e21bdbd15395dc610ecc2bf2eb6675ba0c4585597b375b7bd099e`.
- **`README.txt` in that archive (verbatim):**

  > Newstroke is a stroke (plotter) font originally designed for KiCAD.
  >
  > Previously released under CC-BY licence.
  > Now released under CC0 licence, to give it right to be used in any project.
  > Licence text here: http://creativecommons.org/publicdomain/zero/1.0/

- **Contents:** the glyph sources (`font.lib`, `symbol.lib`, `charlist.txt`),
  the generator (`fontconv.awk`), and a generated `newstroke_font.cpp`/`.h`
  covering U+0020–U+2BFF (11,232 code points). The generated `.cpp` carries
  KiCad's GPL-2.0-or-later file header because it was produced as a KiCad source
  file. The author's licence for the font itself is the CC0 statement above.

## Downstream copies (not authoritative)

- **KiCad** `common/newstroke_font.cpp` / `include/newstroke_font.h`: the file
  header is GPL-2.0-or-later, (C) 2010 Vladimir Uryvaev, (C) the KiCad
  developers. The 2019+ header adds an MIT grant for CJK ideograph code
  "(c) 2018 Lingdong Huang", plus Adobe Source Han Sans glyphs under SIL Open
  Font License 1.1 with Reserved Font Name "Source". Debian's
  `/usr/share/doc/kicad-libraries/copyright` lists these files as GPL-2+.
- **KiCanvas** (<https://kicanvas.org/license/>) summarises this as "Originally
  licensed under Creative Commons CC0 1.0, amended with an MIT-like license, and
  utilizes glyphs that are licensed under the SIL Open Font License Version
  1.1". That paraphrases KiCad's header.
- **kicad-rs/kicad_newstroke_font** ships KiCad's header as its `LICENSE.txt`.

## Comparison: vendored `glyph-full.js` vs the 2015 CC0 release

Over U+0020–U+2BFF (the CC0 release's whole range), using
`frontend/static/kicad-viewer/glyph-full.js`:

- **2,346 literal glyph strings:** 2,344 are byte-identical to the 2015 CC0
  table. Two differ, which means KiCad edited them after 2015:
  - U+007E `~` tilde (moved vertically)
  - U+2126 `Ω` ohm sign (redrawn)
- **8,886 entries are integer references** to KiCad's shared-glyph table, at
  positions where the 2015 table has a duplicate glyph, i.e. de-duplication. The
  shared table isn't in `glyph-full.js`, so this can't be byte-verified from that
  file.
- **U+2C00–U+2FFF (1,024 code points)** are all references to one shared
  placeholder glyph, so there are no new designs in that range.
- **U+3000 and above** (kana and CJK, from Lingdong Huang and Adobe) are outside
  the range this comparison or any planned import covers.

## Conclusion and import rules for T3

1. **Source:** take glyph data only from the author's 2015 CC0 release (the
   tarball above, checksum pinned), never from KiCad, KiCanvas or kicad-rs
   copies. Generate the Rust table from `font.lib`, `symbol.lib` and
   `charlist.txt`, or from the release's generated table, and check it into
   kicadmium with a generator note.
2. **Licence:** CC0 1.0 needs no notice, but keep attribution anyway: "NewStroke
   font by Vladimir Uryvaev (vovanium), CC0 1.0, release 2015-11-19,
   <https://vovanium.ru/sledy/newstroke/en>". Record it in `docs/third-party.md`.
3. **Exclude KiCad-modified glyphs:** don't copy KiCad's versions of U+007E or
   U+2126. Use the 2015 CC0 glyphs, accepting the small visual difference from
   current KiCad, or draw replacements ourselves.
4. **CJK out of scope:** U+3000+ (Lingdong Huang MIT code and Adobe Source Han
   Sans OFL glyphs) is not imported. Adding it later needs the MIT notice and the
   OFL text, and must respect the Reserved Font Name "Source".
5. **Vendored `glyph-full.js`:** it is KiCanvas's copy of KiCad's table,
   including the two KiCad-modified glyphs and KiCad's shared-glyph
   restructuring. It goes away when T3 replaces KiCanvas. Until then it's
   redistributed as part of the KiCanvas bundle under KiCanvas's own notice.
