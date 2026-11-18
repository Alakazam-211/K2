#!/usr/bin/env python3
"""Generate crates/k2-daemon/src/mail/windows_zones.rs from CLDR windowsZones.xml.

    scripts/gen-windows-zones.py <windowsZones.xml> <cldr-release> [<out.rs>]

<cldr-release> is the CLDR release the file came from, e.g. 49 (the tag
release-49). Fetch the file from that release:

    https://raw.githubusercontent.com/unicode-org/cldr/release-<N>/common/supplemental/windowsZones.xml

Only the mappings are kept: each Windows zone id with its territory-001
IANA zone (the "golden zone" CLDR names for the id). The data is under the
Unicode License v3 (Unicode-3.0); its full text is in NOTICE.md. Used by
the calendar event shaping (crate::mail::calendar_event::iana_zone) to turn
Exchange's Windows zone names into IANA names. Run it again with a newer
CLDR release to update; never edit the generated file by hand.
"""

import sys
import xml.etree.ElementTree as ET

COPYRIGHT_YEARS = "1991-2026"
DEFAULT_OUT = "crates/k2-daemon/src/mail/windows_zones.rs"


def main() -> int:
    if len(sys.argv) not in (3, 4):
        sys.stderr.write(__doc__)
        return 2
    src, release = sys.argv[1], sys.argv[2].strip()
    out = sys.argv[3] if len(sys.argv) == 4 else DEFAULT_OUT
    if not release.isdigit() or int(release) < 44:
        sys.stderr.write("the CLDR release must be a number, 44 or later\n")
        return 2
    root = ET.parse(src).getroot()
    pairs = []
    for zone in root.iter("mapZone"):
        if zone.get("territory") != "001":
            continue
        windows, iana = zone.get("other", ""), zone.get("type", "")
        if not windows or not iana or " " in iana:
            sys.stderr.write(f"unexpected territory-001 row: {windows!r} -> {iana!r}\n")
            return 1
        for s in (windows, iana):
            if '"' in s or "\\" in s:
                sys.stderr.write(f"unexpected character in {s!r}\n")
                return 1
        pairs.append((windows, iana))
    if len(pairs) < 100:
        sys.stderr.write(f"only {len(pairs)} mappings: not a windowsZones.xml?\n")
        return 1
    if len({w for w, _ in pairs}) != len(pairs):
        sys.stderr.write("a Windows zone id appears twice for territory 001\n")
        return 1
    url = (
        f"https://github.com/unicode-org/cldr/blob/release-{release}"
        "/common/supplemental/windowsZones.xml"
    )
    lines = [
        f"// generated from CLDR {release} windowsZones.xml",
        f"// {url}",
        "// by scripts/gen-windows-zones.py: do not edit by hand; run the script",
        "// again with a newer CLDR release to update.",
        "//",
        f"// Copyright © {COPYRIGHT_YEARS} Unicode, Inc.",
        "// SPDX-License-Identifier: Unicode-3.0",
        "// (the licence text is in NOTICE.md)",
        "",
        "//! Windows zone ids → IANA zones (CLDR windowsZones, territory 001).",
        "",
        "/// `(Windows zone id, IANA zone)`, in CLDR's order.",
        "pub const WINDOWS_ZONES: &[(&str, &str)] = &[",
    ]
    lines += [f'    ("{w}", "{i}"),' for w, i in pairs]
    lines += ["];", ""]
    with open(out, "w", encoding="utf-8") as f:
        f.write("\n".join(lines))
    print(f"wrote {out}: {len(pairs)} mappings from CLDR {release}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
