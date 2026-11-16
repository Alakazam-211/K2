"""Test tooling, never shipped: an independent count of what each public corpus file holds, for
crates/timeline/tests/corpus.rs to compare with the readers. It reads the raw files with
Python's standard library only and shares no code with the readers.

    python3 -I scripts/corpus-expected.py DIR > crates/timeline/tests/corpus/expected.tsv

One row per timeline: file, timeline number (from 1, in the reader's order), then
  otio:   tracks, clips (Clip objects on its tracks), transitions, clip markers
  fcpxml: story elements on the primary storyline
  prproj: tracks, clip items, transition items, sequence markers
"""
import gzip
import json
import os
import re
import sys
import xml.etree.ElementTree as ET


def otio_rows(name, text):
    # OTIO's writer emits Inf and -Inf, which Python's json doesn't read: as Infinity
    text = re.sub(r"(?<![A-Za-z\"])(-?)Inf(?![a-z])", r"\1Infinity", text)
    root = json.loads(text)
    timelines = []

    def collect(o):
        s = o.get("OTIO_SCHEMA", "")
        if s.startswith("Timeline."):
            timelines.append(o)
        elif s.startswith("SerializableCollection."):
            for c in o.get("children") or []:
                collect(c)

    collect(root)
    rows = []
    for n, t in enumerate(timelines, 1):
        tracks = [c for c in t["tracks"].get("children") or []]
        clips = transitions = markers = 0
        for tr in tracks:
            for c in tr.get("children") or []:
                s = c.get("OTIO_SCHEMA", "")
                if s.startswith("Clip."):
                    clips += 1
                    markers += len(c.get("markers") or [])
                elif s.startswith("Transition."):
                    transitions += 1
        rows.append([name, n, len(tracks), clips, transitions, markers])
    return rows


def fcpxml_rows(name, text):
    root = ET.fromstring(text)
    seqs = []
    # projects in document order (root, library, events), then compound clips in resource order
    for p in root.iter("project"):
        seqs.append(p.find("sequence"))
    res = root.find("resources")
    if res is not None:
        for m in res.findall("media"):
            s = m.find("sequence")
            if s is not None:
                seqs.append(s)
    rows = []
    for n, s in enumerate(seqs, 1):
        spine = s.find("spine")
        rows.append([name, n, len(list(spine))])
    return rows


def prproj_rows(name, data):
    root = ET.fromstring(gzip.decompress(data))
    by_id = {o.get("ObjectID"): o for o in root if o.get("ObjectID")}
    by_uid = {o.get("ObjectUID"): o for o in root if o.get("ObjectUID")}

    def ref(e):
        return by_id.get(e.get("ObjectRef")) if e.get("ObjectRef") else by_uid.get(e.get("ObjectURef"))

    rows = []
    n = 0
    for s in root:
        if s.tag != "Sequence":
            continue
        n += 1
        tracks = clips = transitions = 0
        for g in s.find("TrackGroups").findall("TrackGroup"):
            group = ref(g.find("Second"))
            ts = group.find("TrackGroup").find("Tracks")
            for t in [] if ts is None else ts.findall("Track"):
                tracks += 1
                obj = ref(t)
                ct = next(obj.iter("ClipTrack"))
                ci = ct.find("ClipItems")
                ti = ct.find("TransitionItems")
                for part, add in ((ci, "c"), (ti, "t")):
                    items = None if part is None else part.find("TrackItems")
                    k = 0 if items is None else len(items.findall("TrackItem"))
                    if add == "c":
                        clips += k
                    else:
                        transitions += k
        markers = 0
        owner = s.find("MarkerOwner")
        if owner is not None:
            ms = ref(owner.find("Markers"))
            inner = ms.find("Markers")
            markers = 0 if inner is None else len(inner.findall("Marker"))
        rows.append([name, n, tracks, clips, transitions, markers])
    return rows


def main():
    d = sys.argv[1]
    print("# file\ttimeline\tcounts (scripts/corpus-expected.py; see its header)")
    for name in sorted(os.listdir(d)):
        p = os.path.join(d, name)
        data = open(p, "rb").read()
        try:
            if name.endswith(".otio"):
                rows = otio_rows(name, data.decode("utf-8"))
            elif name.endswith(".fcpxml"):
                rows = fcpxml_rows(name, data.decode("utf-8"))
            elif name.endswith(".prproj"):
                rows = prproj_rows(name, data)
            else:
                continue
        except Exception as e:  # noqa: BLE001 - a row saying the checker couldn't read it
            print(f"{name}\t-\tunread: {type(e).__name__}")
            continue
        for r in rows:
            print("\t".join(str(x) for x in r))


main()
