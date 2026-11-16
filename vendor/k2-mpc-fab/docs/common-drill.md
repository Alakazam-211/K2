# Common drill exports

The pack reads drill files two ways. The strict reader (`drill`) reads the XNC subset and nothing
else. The common-export reading (`drill::common`) reads common drill exports: the drill files CAD
tools commonly write that XNC doesn't cover. It is a separate reading, and **every file it reads
is reported not conforming**, with the constructs it relied on listed (`CommonDrill::relied`; in
the index, `relies.<construct>` with a count). `CommonDrillPack` reads a file strictly first, and
uses the common reading only when the strict reader refuses it; the index then also carries the
strict reader's refusal (`strict_refusal`).

## How the rules were learned

Only from permissively licensed sample files, by comparing them with each other, and described
here in our own words. The samples are third-party test files, fetched by reference and never
committed: `crates/fab/tests/corpus/manifest.tsv` lists each with its URL, licence and checksum,
under the names used below. No vendor manual was read, and no reader's or writer's source.

Every construct below is accepted exactly as described, and nothing else: a line that is neither
XNC nor one of these is refused (`not-common`, with the kind of line). Where a sample could be
read more than one way, the reading refuses it rather than choose.

## The number format

Coordinates are decimals in the file's unit; the model keeps them as integers in millionths of
the unit, exactly (more decimals are refused). A coordinate **with** a decimal point reads as
written, whatever the header says. A coordinate **without** one reads only when the header fixes
the number format in one of these three ways; otherwise it is refused (`format-not-fixed`), never
guessed. In each, `<unit>` is `METRIC` (mm) or `INCH`, and a template is a run of `0`s, a point
and a run of `0`s whose lengths give the whole digits and the fraction digits.

1. **`unit-template`**: a unit line `<unit>,<template>` (no zero mode), for example
   `METRIC,000.000`. Every coordinate then has exactly the template's digits, whole part first;
   a coordinate with any other digit count is refused (`coordinate-digits`).
   Samples: `fritzing/combined.txt` (`METRIC,000.000`, every coordinate six digits),
   `diptrace/panel.drl` (`METRIC,0000.00`, every coordinate six digits after a `+`).
2. **`unit-trailing-zeros-template`**: a unit line `<unit>,TZ,<template>`, for example
   `METRIC,TZ,000.000`. Trailing zeros are kept, so the last fraction-length digits are the
   fraction and leading zeros may be left out; a coordinate with more digits than the template
   is refused. Sample: `eagle-newer/drills.xln` (`METRIC,TZ,000.000`; coordinates of four and
   five digits).
3. **`unit-leading-zeros-file-format`**: a unit line `<unit>,LZ` together with a header comment
   `;FILE_FORMAT=<w>:<f>` (one digit each). Leading zeros are kept, so the first `<w>` digits are
   the whole part and trailing zeros may be left out, so a short coordinate is read with zeros
   put back after it (`X018` with `4:4` is 180 mm); a coordinate with more than `<w>+<f>` digits
   is refused. Samples: `altium-composite-drill/LimeSDR-QPCIe_1v2-RoundHoles.TXT`
   and `...-SlotHoles.TXT` (`;FILE_FORMAT=4:4` and `METRIC,LZ`; coordinates of four to eight
   digits).

Each reading was checked against the same sample set's photoplot files: read this way, the
drill coordinates fall on the photoplot files' pads and track ends; read the other ways a
coordinate without a point could be read (counting from the other end), they don't.

Nothing else fixes the format. In particular a bare unit line (`INCH` in `diptrace/keyboard.drl`
and `diptrace/mainboard.drl`), a zero mode without a template (`METRIC,TZ` in the three
`siemens-2/*.ncd` samples, whose only other statement of the format is free text in a comment
before the header), a file-format comment without `,LZ`, and a leading-zeros unit line with a
template (no sample shows one) all leave it unfixed.

## Constructs

Each heading is the construct's name as `relies.<name>` reports it.

- **`percent-before-header`**: one `%` line before the `M48` header start. It holds no hole and
  changes nothing; a second one is refused. Sample: `fritzing/combined.txt` (its first line).
- **`unit-template`**, **`unit-trailing-zeros-template`**, **`unit-leading-zeros-file-format`**:
  the number formats above.
- **`format-version-2`**: the header line `FMAT,2`. It changes nothing this reading uses; any
  other `FMAT` value is refused. Samples: `eagle-newer/drills.xln`, `siemens-2/*.ncd`.
- **`incremental-off`**: the header line `ICI,OFF`, saying coordinates are absolute, as this
  reading reads them; `ICI,ON` is refused. Samples: `eagle-newer/drills.xln`, `siemens-2/*.ncd`.
- **`tool-number-by-value`**: a tool number of one or three digits rather than two, in a
  declaration or a selection. A tool is matched by the number's value, so a tool declared as
  `T3` is the one `T03` selects; a selection that matches no declaration is refused, and a
  number over 255 is refused. Samples: `eagle-newer/drills.xln` (`T1`, `T2`),
  `fritzing/combined.txt` (`T100` to `T105`, each declared and selected with three digits),
  `altium-composite-drill/...RoundHoles.TXT` (declared `T3`, selected `T03`).
- **`tool-feed-speed`**: a declaration with feed and speed fields, digits after `F` and `S`, in
  that order before the diameter (`T3F00S00C0.8000`). They don't change a hole and aren't kept;
  any other field is refused. Samples: both `altium-composite-drill` drill files.
- **`absolute-mode`**: `G90` in the body: absolute coordinates, as this reading reads them.
  `G91` is refused. Samples: `eagle-newer/drills.xln`, `altium-composite-drill/...SlotHoles.TXT`,
  `siemens-2/*.ncd`.
- **`unit-code-in-body`**: `M71` (mm) or `M72` (inch) in the body, accepted only when it agrees
  with the header's unit; a disagreeing one is refused. Samples: `eagle-newer/drills.xln` and
  `siemens-2/*.ncd` (`M71` after a `METRIC` header).
- **`zero-set-origin`**: `G93X0Y0` in the body: the origin stays where it is. Any other values
  are refused. Sample: `siemens-2/*.ncd`.
- **`modal-coordinate`**: a coordinate line with only X or only Y; the other axis is the current
  point's. Refused when there is no current point yet. Samples: `siemens-2/*.ncd` (hits and rout
  moves), `altium-composite-drill/...RoundHoles.TXT` (runs of hits with only Y),
  `...SlotHoles.TXT` (rout moves).
- **`drill-mode-by-default`**: hits before any `G05` or `G00`: the body starts in drill mode.
  After a rout move a hit still needs `G05`. Samples: every sample with hits
  (`diptrace/*.drl`, `eagle-newer/drills.xln`, `fritzing/combined.txt`,
  `altium-composite-drill/...RoundHoles.TXT`).
- **`tool-deselect`**: `T0` or `T00`: no tool is selected; a hit before the next selection is
  refused. Samples: `diptrace/*.drl` and `siemens-2/*.ncd` (`T00` just before the end).
- **`end-code-after-tool-up`**: `M17` with the tool up, accepted only when the end code `M30`
  is the very next line, so it can't change a hole. Sample:
  `altium-composite-drill/...SlotHoles.TXT` (after its last tool up).

Everything else follows the strict reader's rules: printable ASCII and line breaks only, spaces
only in comments, `M48` first (after comments and the one `%`), the unit before the tools, the
header ending with `%`, rout paths between `M15` and `M16` after a `G00` move, `G01` lines inside
them, and `M30` last. Attributes in standard comments read as in XNC. Rout arcs (`G02`, `G03`)
are refused: no permissive sample shows one outside XNC.

## The samples

What each permissive drill sample in the corpus gives (the corpus run prints each):

| Sample | Strict | Common |
|---|---|---|
| `diptrace/keyboard.drl`, `diptrace/mainboard.drl` | refused | refused: `format-not-fixed` (a bare `INCH` header) |
| `diptrace/panel.drl` | refused | read |
| `eagle-newer/drills.xln` | refused | read |
| `fritzing/combined.txt` | refused | read |
| `siemens-2/ContourPlated.ncd`, `ThruHoleNonPlated.ncd`, `ThruHolePlated.ncd` | refused | refused: `format-not-fixed` (`METRIC,TZ` without a template) |
| `altium-composite-drill/...RoundHoles.TXT`, `...SlotHoles.TXT` | refused | read |
