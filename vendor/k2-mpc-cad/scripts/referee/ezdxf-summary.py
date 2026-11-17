# A test checker, run on test machines only and never shipped: reads each drawing with ezdxf
# (a separate program, pinned in requirements.txt) and writes what ezdxf sees beside it, as
# <file>.ezdxf.txt, one fact per line:
#   E <handle> <type> <layer>   every entity of every block record (model space and layouts
#                               included); attributes and vertices belong to their parent
#   L <layer name>              every layer
#   B <block name>              every block record
#   I <block name>              one line per block reference, by the block it names
#   D <block name> <real name>  an anonymous block that draws a dynamic block, and that block's
#                               name, as ezdxf reads the record's extension data
#   X <error> <message>         ezdxf refused the file: the only line (a finding, never skipped)
# `cad-measure referee DIR` compares these with the pack's own reading.
# Run: python3 -I scripts/referee/ezdxf-summary.py DIR
import glob
import sys

import ezdxf

for path in sorted(glob.glob(sys.argv[1] + "/*.dxf")):
    try:
        doc = ezdxf.readfile(path)
    except Exception as e:  # recorded per file: the comparison reports it as a difference
        with open(path + ".ezdxf.txt", "w", encoding="utf-8") as f:
            f.write("X %s %s\n" % (type(e).__name__, str(e)[:200].replace("\n", " ")))
        print("ezdxf-summary", path.split("/")[-1], "REFUSED:", type(e).__name__)
        continue
    out = []
    for blk in doc.blocks:
        out.append("B " + blk.name)
        for e in blk:
            layer = e.dxf.get("layer", "0")
            out.append("E %s %s %s" % (e.dxf.handle, e.dxftype(), layer))
            if e.dxftype() == "INSERT":
                out.append("I " + e.dxf.name)
    for br in doc.block_records:  # a dynamic block's anonymous block: the record it names
        h = next((t.value for t in br.get_xdata("AcDbBlockRepBTag") if t.code == 1005), None) if br.has_xdata("AcDbBlockRepBTag") else None
        real = doc.entitydb.get(str(h).upper()) if h else None
        if real is not None and real.dxftype() == "BLOCK_RECORD":
            out.append("D %s %s" % (br.dxf.name, real.dxf.name))
    for layer in doc.layers:
        out.append("L " + layer.dxf.name)
    with open(path + ".ezdxf.txt", "w", encoding="utf-8") as f:
        f.write("\n".join(out) + "\n")
    print("ezdxf-summary", path.split("/")[-1], len(out), "facts")
