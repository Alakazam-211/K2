# A test checker, run on test machines only and never shipped: reads each drawing with ezdxf
# (a separate program, pinned in requirements-draw.txt) and writes the resolved drawing values ezdxf
# gives beside it, as <file>.ezdxf-draw.txt, one fact per line:
#   P <n> <r> <g> <b>                the palette: entry n (0 to 255) of ezdxf's model-space table
#   Y <handle> <#rrggbb> <ltype> <lw> <ltscale>
#                                    a model-space entity's resolved colour, linetype, lineweight
#                                    (mm) and linetype scale
#   M <handle> <a> <b> <c> <d> <e> <f>
#                                    a block reference's map, x' = a x + c y + e, y' = b x + d y + f
#   T <handle> <alignment> <x1> <y1> <x2> <y2>
#                                    a text's alignment and its two alignment points
#   Q <handle> <json text>           a multiline text's plain text
#   S <handle> <x> <y> ...           a spline flattened (distance below F <d>)
#   F <d>                            the flattening distance used for every S line
#   W <handle> <x> <y> ...           a wipeout's boundary in world coordinates
#   H <handle> <count> <length>      a pattern hatch's lines: how many, their total length
#   O <handle> ...                   model space in draw order (by sort handle with a draw-order
#                                    table, else in the file's order)
#   X <error> <message>              ezdxf refused the file: the only line
# `cad-measure referee-draw DIR` compares these with the pack's drawing-model read.
# Run: python3 -I scripts/referee/ezdxf-draw-summary.py DIR
import glob
import json
import sys

import ezdxf
from ezdxf import colors, reorder
from ezdxf.addons.drawing.properties import RenderContext
from ezdxf.render import hatching

FLAT = 1e-6


def num(x):
    return repr(float(x))


for path in sorted(glob.glob(sys.argv[1] + "/*.dxf")):
    try:
        doc = ezdxf.readfile(path)
    except Exception as e:  # recorded per file: the comparison reports it as a difference
        with open(path + ".ezdxf-draw.txt", "w", encoding="utf-8") as f:
            f.write("X %s %s\n" % (type(e).__name__, str(e)[:200].replace("\n", " ")))
        print("ezdxf-draw-summary", path.split("/")[-1], "REFUSED:", type(e).__name__)
        continue
    out = ["F " + num(FLAT)]
    for n in range(0, 256):
        r, g, b = colors.int2rgb(colors.DXF_DEFAULT_COLORS[n])
        out.append("P %d %d %d %d" % (n, r, g, b))
    msp = doc.modelspace()
    ctx = RenderContext(doc)
    ctx.set_current_layout(msp)
    for e in msp:
        h = e.dxf.handle
        t = e.dxftype()
        try:
            p = ctx.resolve_all(e)
            out.append(
                "Y %s %s %s %s %s"
                % (h, p.color[:7].lower(), p.linetype_name.upper(), num(p.lineweight), num(p.linetype_scale))
            )
        except Exception as ex:
            out.append("Y %s ERROR %s" % (h, type(ex).__name__))
        if t == "INSERT":
            m = e.matrix44()
            o = m.transform((0, 0, 0))
            x = m.transform((1, 0, 0))
            y = m.transform((0, 1, 0))
            out.append(
                "M %s %s %s %s %s %s %s"
                % (h, num(x.x - o.x), num(x.y - o.y), num(y.x - o.x), num(y.y - o.y), num(o.x), num(o.y))
            )
        elif t == "TEXT":
            align, p1, p2 = e.get_placement()
            p2 = p2 if p2 is not None else p1
            out.append("T %s %s %s %s %s %s" % (h, align.name, num(p1.x), num(p1.y), num(p2.x), num(p2.y)))
        elif t == "MTEXT":
            out.append("Q %s %s" % (h, json.dumps(e.plain_text(split=False))))
        elif t == "SPLINE":
            try:
                pts = list(e.flattening(FLAT))
                out.append("S %s %s" % (h, " ".join("%s %s" % (num(q.x), num(q.y)) for q in pts)))
            except Exception as ex:
                out.append("S %s ERROR %s" % (h, type(ex).__name__))
        elif t in ("WIPEOUT", "IMAGE"):
            pts = e.boundary_path_wcs()
            out.append("W %s %s" % (h, " ".join("%s %s" % (num(q.x), num(q.y)) for q in pts)))
        elif t == "HATCH" and not e.dxf.solid_fill:
            count, length = 0, 0.0
            try:
                for a, b in hatching.hatch_entity(e, jiggle_origin=False):
                    count += 1
                    length += (b - a).magnitude
                    if count > 200000:
                        break
                out.append("H %s %d %s" % (h, count, num(length)))
            except Exception as ex:
                out.append("H %s ERROR %s" % (h, type(ex).__name__))
    # draw order: by sort handle when model space has a draw-order table; else the file's own
    # order (as the drawing-model read draws), not handle order
    redraw = list(msp.get_redraw_order())
    order = reorder.ascending(msp, redraw) if redraw else list(msp)
    out.append("O " + " ".join(e.dxf.handle for e in order))
    with open(path + ".ezdxf-draw.txt", "w", encoding="utf-8") as f:
        f.write("\n".join(out) + "\n")
    print("ezdxf-draw-summary", path.split("/")[-1], len(out), "facts")
