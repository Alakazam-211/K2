# A test checker, run on test machines only and never shipped: reads each edit verb's input and
# output (`cad-measure edits DIR` writes them) with ezdxf (a separate program, pinned in
# requirements.txt) and writes what it reads of every handle the call's intent names, beside each
# file, as <file>.ezdxf-edits.txt, one fact per line:
#   E <h> <type> <layer>        the entity
#   G <h>                       no entity has that handle
#   C <h> <colour number>       T <h> <true colour or none>     LT <h> <linetype>
#   P <h> <name> <x> <y>        a point (start, end, center, insert, v<n>, a:<tag> for an attribute)
#   V <h> <name> <x> <y>        a vector (an ellipse's major axis): never moved by a move
#   R <h> <name> <degrees>      an angle (start, end, rotation, ar:<tag> an attribute's), in [0, 360)
#   H <h> <name> <value>        a size (radius, height, xscale, yscale, ah:<tag> an attribute's height)
#   A <h> <tag> <value>         an attribute's value       S <h> <text>   a text's
#                               (both with the format's \U+XXXX escapes decoded: ezdxf keeps them
#                               as written, the app shows the character)
#   AUDIT <errors>              ezdxf's audit of the file
#   X <error> <message>         ezdxf refused the file: the only line
# `cad-measure edit-referee DIR` compares them.
# Run: python3 -I scripts/referee/ezdxf-edits.py DIR
import glob
import sys

import ezdxf
from ezdxf.lldxf.encoding import decode_dxf_unicode


def facts(doc, h):
    e = doc.entitydb.get(h.upper()) or doc.entitydb.get(h.lower())
    if e is None or not e.is_alive:
        return ["G %s" % h]
    t = e.dxftype()
    out = ["E %s %s %s" % (h, t, e.dxf.get("layer", "0"))]
    out.append("C %s %s" % (h, e.dxf.get("color", 256)))
    tc = e.dxf.get("true_color", None)
    out.append("T %s %s" % (h, "none" if tc is None else tc))
    out.append("LT %s %s" % (h, e.dxf.get("linetype", "BYLAYER")))

    def p(name, v, kind="P"):
        out.append("%s %s %s %r %r" % (kind, h, name, float(v[0]), float(v[1])))

    def r(name, a):
        out.append("R %s %s %r" % (h, name, float(a) % 360.0))

    def size(name, v):
        out.append("H %s %s %r" % (h, name, float(v)))

    if t == "LINE":
        p("start", e.dxf.start)
        p("end", e.dxf.end)
    elif t in ("CIRCLE", "ARC"):
        p("center", e.dxf.center)
        size("radius", e.dxf.radius)
        if t == "ARC":
            r("start", e.dxf.start_angle)
            r("end", e.dxf.end_angle)
    elif t == "ELLIPSE":
        p("center", e.dxf.center)
        p("major", e.dxf.major_axis, "V")
    elif t == "LWPOLYLINE":
        for i, v in enumerate(e.get_points("xy")):
            p("v%d" % i, v)
    elif t == "POLYLINE":
        for i, v in enumerate(e.vertices):
            p("v%d" % i, v.dxf.location)
    elif t in ("TEXT", "MTEXT"):
        p("insert", e.dxf.insert)
        size("height", e.dxf.get("char_height" if t == "MTEXT" else "height", 0))
        if t == "TEXT":
            r("rotation", e.dxf.get("rotation", 0))
            out.append("S %s %s" % (h, decode_dxf_unicode(e.dxf.text)))
    elif t == "INSERT":
        p("insert", e.dxf.insert)
        r("rotation", e.dxf.get("rotation", 0))
        size("xscale", e.dxf.get("xscale", 1))
        size("yscale", e.dxf.get("yscale", 1))
        for a in e.attribs:
            p("a:" + a.dxf.tag, a.dxf.insert)
            r("ar:" + a.dxf.tag, a.dxf.get("rotation", 0))
            size("ah:" + a.dxf.tag, a.dxf.get("height", 0))
            out.append("A %s %s %s" % (h, a.dxf.tag, decode_dxf_unicode(a.dxf.text)))
    return out


for intent in sorted(glob.glob(sys.argv[1] + "/*.intent.txt")):
    base = intent[: -len(".intent.txt")]
    handles = []
    with open(intent, encoding="utf-8") as f:
        for line in f:
            w = line.split()
            if len(w) >= 2 and w[1] not in handles:
                handles.append(w[1])
    for side in ("in", "out"):
        path = "%s.%s.dxf" % (base, side)
        try:
            doc = ezdxf.readfile(path)
        except Exception as e:  # recorded per file: the comparison reports it as a difference
            lines = ["X %s %s" % (type(e).__name__, str(e)[:200].replace("\n", " "))]
        else:
            lines = []
            for h in handles:
                lines.extend(facts(doc, h))
            lines.append("AUDIT %d" % len(doc.audit().errors))
        with open(path + ".ezdxf-edits.txt", "w", encoding="utf-8") as f:
            f.write("\n".join(lines) + "\n")
    print("ezdxf-edits", base.split("/")[-1], len(handles), "handles")
