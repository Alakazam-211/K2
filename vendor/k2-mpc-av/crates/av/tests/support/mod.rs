//! A generated two-sheet AV set and a neutral example configuration.
#![allow(dead_code)]

use k2_mpc_base::errors::Value;
use k2_mpc_dxf::testgen::{Built, Ent, Layer, Spec, Ver};

/// A neutral example configuration (no project's conventions).
pub const CONFIG: &str = "\
# an example: a project's own configuration stays with the project
device.tags = DEVICE
boundary.layers = ROOMS
boundary.ltypes = HIDDEN
cable.layers = AV-CABLE
cable.snap = 1.0
cable.label_within = 2.0
cable.label_letters = 1-3
cable.label_digits = 3
flag.out_blocks = FLAG-OUT
flag.in_blocks = FLAG-IN
flag.id_tag = FLAG
flag.sheet_tag = SHEET
title.block = TITLE
title.sheet_tag = SHEET_NO
title.name_tag = SHEET_NAME
";

fn base() -> Spec {
    Spec::new(Ver::R2018)
        .layer("AV-CABLE", 1)
        .layer("TEXT", 7)
        .layer_full(Layer {
            name: "ROOMS".into(),
            color: 3,
            frozen: false,
            locked: false,
            ltype: "DASHED".into(),
        })
        .block("DEV", [0.0, 0.0], vec![Ent::circle([0.0, 0.0], 0.5)])
        .block(
            "FLAG-OUT",
            [0.0, 0.0],
            vec![Ent::line([0.0, 0.0], [1.0, 0.0])],
        )
        .block(
            "FLAG-IN",
            [0.0, 0.0],
            vec![Ent::line([0.0, 0.0], [1.0, 0.0])],
        )
        // an outgoing flag as a dynamic block draws it, set longer
        .dynamic(
            "*U7",
            [0.0, 0.0],
            vec![Ent::line([0.0, 0.0], [2.0, 0.0])],
            "FLAG-OUT",
        )
        .block(
            "TITLE",
            [0.0, 0.0],
            vec![Ent::line([0.0, 0.0], [10.0, 0.0])],
        )
}

/// The first sheet: a rack (a boundary by its layer) and a room (by its linetype), devices,
/// three cables (two crossing them; one inside the rack with a note on it that isn't a label's
/// shape, and an open end), a malformed identifier, two outgoing flags (one drawn through a
/// dynamic block's anonymous block), a title block.
pub fn video() -> Built {
    let sq = |x: f64| {
        [
            ([x, 0.0], 0.0),
            ([x + 20.0, 0.0], 0.0),
            ([x + 20.0, 20.0], 0.0),
            ([x, 20.0], 0.0),
        ]
    };
    base()
        .model(Ent::lwpoly(&sq(0.0), true).on("ROOMS").label("rack"))
        .model(Ent::text([2.0, 18.0], 1.0, "RACK A").on("TEXT"))
        .model(
            Ent::lwpoly(&sq(100.0), true)
                .on("0")
                .ltype("HIDDEN")
                .label("room"),
        )
        .model(Ent::text([102.0, 18.0], 1.0, "ROOM B").on("TEXT"))
        .model(
            Ent::insert("DEV", [5.0, 5.0])
                .attrib("DEVICE", "CAM-01")
                .label("cam"),
        )
        .model(
            Ent::insert("DEV", [40.0, 5.0])
                .attrib("DEVICE", "TX-01")
                .label("tx"),
        )
        .model(
            Ent::insert("DEV", [10.0, 15.0])
                .attrib("DEVICE", "BAD_ID")
                .label("bad"),
        )
        .model(
            Ent::line([5.0, 5.0], [40.0, 5.0])
                .on("AV-CABLE")
                .label("c1"),
        )
        .model(Ent::text([30.0, 6.0], 0.5, "C-101").on("TEXT"))
        .model(
            Ent::line([5.0, 5.0], [10.0, 10.0])
                .on("AV-CABLE")
                .label("inside"),
        )
        .model(Ent::text([8.0, 8.0], 0.5, "NOTE").on("TEXT"))
        .model(
            Ent::line([40.0, 5.0], [110.0, 5.0])
                .on("AV-CABLE")
                .label("c2"),
        )
        .model(Ent::text([60.0, 6.0], 0.5, "C-102").on("TEXT"))
        .model(
            Ent::insert("*U7", [50.0, 10.0])
                .attrib("FLAG", "F1")
                .attrib("SHEET", "2")
                .label("f1"),
        )
        .model(
            Ent::insert("FLAG-OUT", [50.0, 12.0])
                .attrib("FLAG", "F9")
                .attrib("SHEET", "2")
                .label("f9"),
        )
        .paper(
            Ent::insert("TITLE", [0.0, 0.0])
                .attrib("SHEET_NO", "1")
                .attrib("SHEET_NAME", "VIDEO")
                .attrib("REV1", "A")
                .attrib("REV2", "B")
                .attrib("REV3", ""),
        )
        .build()
}

/// The second sheet: the transmitter again, the incoming flag F1 and an unpaired F2, a title.
pub fn control() -> Built {
    base()
        .model(
            Ent::insert("DEV", [5.0, 5.0])
                .attrib("DEVICE", "TX-01")
                .label("tx"),
        )
        .model(
            Ent::insert("FLAG-IN", [0.0, 10.0])
                .attrib("FLAG", "F1")
                .attrib("SHEET", "1")
                .label("f1"),
        )
        .model(
            Ent::insert("FLAG-IN", [0.0, 12.0])
                .attrib("FLAG", "F2")
                .label("f2"),
        )
        .paper(
            Ent::insert("TITLE", [0.0, 0.0])
                .attrib("SHEET_NO", "2")
                .attrib("SHEET_NAME", "CONTROL")
                .attrib("REV1", "A"),
        )
        .build()
}

/// A neutral configuration for a set whose title block is shared by reference: the sheet's
/// number and name in a block of its own, the revision rows (dated) in the referenced drawing.
pub const CONFIG_XREF: &str = "\
title.block = TITLE
title.sheet_block = SHEETNO
title.sheet_tag = SHEET_NO
title.name_tag = SHEET_NAME
title.date_prefix = DATE
";

/// A title block drawing: the title block, its revision rows `revs` (revision, date), no
/// sheet number.
pub fn title_drawing(revs: &[(&str, &str)]) -> Built {
    let mut t = Ent::insert("TITLE", [0.0, 0.0]);
    for (n, (r, d)) in revs.iter().enumerate() {
        t = t
            .attrib(&format!("REV{}", n + 1), r)
            .attrib(&format!("DATE{}", n + 1), d);
    }
    base().model(t).build()
}

/// A sheet whose title block is the drawing at `path` (by reference): its own number and name
/// in a `SHEETNO` block, in paper space, and a backdrop drawing referenced in model space.
pub fn xref_sheet(number: &str, name: &str, path: &str) -> Spec {
    base()
        .block(
            "SHEETNO",
            [0.0, 0.0],
            vec![Ent::line([0.0, 0.0], [2.0, 0.0])],
        )
        .xref("TB", path)
        .xref("BACKDROP", "../xrefs/backdrop.dwg")
        .model(Ent::insert("BACKDROP", [0.0, 0.0]))
        .paper(Ent::insert("TB", [0.0, 0.0]))
        .paper(
            Ent::insert("SHEETNO", [100.0, 5.0])
                .attrib("SHEET_NO", number)
                .attrib("SHEET_NAME", name),
        )
}

/// The shared-title set: two sheets, the title block drawing they reference (two revisions
/// dated, an empty third row), a stale copy of it in another folder (one revision), and the
/// backdrop drawing; the second sheet also references the first (an overlay). Read with
/// [`CONFIG_XREF`].
pub fn xref_files() -> Vec<(String, Vec<u8>)> {
    let backdrop = base().model(Ent::line([0.0, 0.0], [100.0, 0.0])).build();
    vec![
        (
            "sheets/av-101.dxf".to_owned(),
            xref_sheet("101", "VIDEO RISER", "..\\xrefs\\titleblock.dwg")
                .build()
                .bytes,
        ),
        (
            "sheets/av-102.dxf".to_owned(),
            xref_sheet("102", "AUDIO RISER", "..\\xrefs\\titleblock.dwg")
                .xref("OVERLAY", "av-101.dwg")
                .model(Ent::insert("OVERLAY", [0.0, 0.0]))
                .build()
                .bytes,
        ),
        (
            "old/titleblock.dxf".to_owned(),
            title_drawing(&[("A", "2026-01-05")]).bytes,
        ),
        (
            "xrefs/titleblock.dxf".to_owned(),
            title_drawing(&[("A", "2026-01-05"), ("B", "2026-02-01"), ("", "")]).bytes,
        ),
        ("xrefs/backdrop.dxf".to_owned(), backdrop.bytes),
    ]
}

/// A sheet whose two cables are labelled by leaders, each text set away from its cable and
/// nearer the other: `c1` (from `A-01` to `B-01`) by a multileader whose second leader's arrow
/// touches it (its text formatted),
/// `c2` (from `C-01` to `D-01`) by a leader to a multiline text. Read with [`CONFIG`].
pub fn leader_sheet() -> Built {
    let dev = |id: &str, at: [f64; 2]| Ent::insert("DEV", at).attrib("DEVICE", id);
    base()
        .model(dev("AB-01", [0.0, 0.0]))
        .model(dev("BC-01", [50.0, 0.0]))
        .model(dev("CD-01", [0.0, 20.0]))
        .model(dev("DE-01", [50.0, 20.0]))
        .model(
            Ent::line([0.0, 0.0], [50.0, 0.0])
                .on("AV-CABLE")
                .label("c1"),
        )
        .model(
            Ent::line([0.0, 20.0], [50.0, 20.0])
                .on("AV-CABLE")
                .label("c2"),
        )
        .model(
            // two leaders: the first's arrow on nothing, the second's on `c1`; its text formatted
            Ent::mleader_branches(
                &[
                    (vec![vec![[60.0, 10.0], [40.0, 15.0]]], [30.0, 18.5]),
                    (vec![vec![[25.0, 0.0], [28.0, 10.0]]], [30.0, 18.5]),
                ],
                [31.0, 19.0],
                "{\\fArial|b0|i0;C-201}",
            )
            .on("TEXT"),
        )
        .model(
            Ent::mtext([12.5, 1.0], 0.5, "C-202")
                .on("TEXT")
                .handle(0x700),
        )
        .model(Ent::leader(&[[10.0, 20.0], [12.0, 1.5]], Some(0x700)).on("TEXT"))
        .build()
}

/// A configuration whose cables are also references of the block `RUN`.
pub const CONFIG_RUNS: &str = "\
device.tags = DEVICE
boundary.layers = ROOMS
cable.layers = AV-CABLE
cable.blocks = RUN
cable.label_letters = 1-3
cable.label_digits = 3
";

/// A sheet whose one cable is a block reference: `RUN` (a one-unit straight run, two parallel
/// lines) placed from `AB-01` to `BC-01`, scaled 30 along it and turned a quarter, with its
/// size and fill as attributes, through room `ROOM C`, labelled `C-301`. Read with
/// [`CONFIG_RUNS`].
pub fn run_sheet() -> Built {
    let room = [
        ([-5.0, 10.0], 0.0),
        ([5.0, 10.0], 0.0),
        ([5.0, 20.0], 0.0),
        ([-5.0, 20.0], 0.0),
    ];
    base()
        .block(
            "RUN",
            [0.0, 0.0],
            vec![
                Ent::line([0.0, -0.05], [1.0, -0.05]),
                Ent::line([0.0, 0.05], [1.0, 0.05]),
            ],
        )
        .model(Ent::insert("DEV", [0.0, 0.0]).attrib("DEVICE", "AB-01"))
        .model(Ent::insert("DEV", [0.0, 30.0]).attrib("DEVICE", "BC-01"))
        .model(Ent::lwpoly(&room, true).on("ROOMS"))
        .model(Ent::text([-4.0, 18.0], 0.5, "ROOM C").on("TEXT"))
        .model(Ent::text([1.0, 15.0], 0.5, "C-301").on("TEXT"))
        .model(
            Ent::insert("RUN", [0.0, 0.0])
                .placed(30.0, 1.0, 90.0)
                .on("AV-CABLE")
                .attrib("SIZE", "1IN")
                .attrib("FILL", "40%")
                .label("run"),
        )
        .build()
}

/// A configuration whose flags are of one kind, `OPC`, paired by the value of `CONN`.
pub const CONFIG_ONE_KIND: &str = "\
flag.blocks = OPC
flag.id_tag = CONN
";

/// Two sheets of one-kind flags, paired by values that don't look like identifiers: `7` and
/// `A` on both (in the opposite order on the second), `Q` on the first only. Read with
/// [`CONFIG_ONE_KIND`].
pub fn one_kind_files() -> (Built, Built) {
    let opc = |v: &str, y: f64, l: &str| Ent::insert("OPC", [0.0, y]).attrib("CONN", v).label(l);
    let with = |s: Spec| s.block("OPC", [0.0, 0.0], vec![Ent::circle([0.0, 0.0], 0.5)]);
    let a = with(base())
        .model(opc("7", 0.0, "seven"))
        .model(opc("A", 2.0, "a"))
        .model(opc("Q", 4.0, "q"))
        .model(opc("", 6.0, "blank"))
        .build();
    let b = with(base())
        .model(opc("A", 0.0, "a"))
        .model(opc("7", 2.0, "seven"))
        .model(opc("", 4.0, "blank"))
        .build();
    (a, b)
}

/// The set's data, read from its files.
pub fn read() -> (Built, Built, Value) {
    let (v, c) = (video(), control());
    let files = vec![
        ("video.dxf".to_owned(), v.bytes.clone()),
        ("control.dxf".to_owned(), c.bytes.clone()),
    ];
    let out = match k2_mpc_av::read_files(CONFIG, &files) {
        Ok(o) => o,
        Err(e) => panic!("the set refused: {e}"),
    };
    assert_eq!(out.spec, ("av.set", 1));
    (v, c, out.data)
}

pub fn list<'v>(v: &'v Value, key: &str) -> &'v [Value] {
    match v.get(key) {
        Some(Value::List(l)) => l,
        _ => panic!("{key} is not a list: {}", v.to_json()),
    }
}

pub fn s<'v>(v: &'v Value, key: &str) -> &'v str {
    v.get(key).and_then(Value::as_str).unwrap_or("")
}

pub fn strs(v: &Value, key: &str) -> Vec<String> {
    list(v, key)
        .iter()
        .map(|x| x.as_str().unwrap_or("").to_owned())
        .collect()
}

pub fn list_f64(v: &Value) -> Vec<f64> {
    match v {
        Value::List(l) => l.iter().map(|x| x.as_f64().unwrap_or(f64::NAN)).collect(),
        _ => panic!("not a list: {}", v.to_json()),
    }
}

/// The text of a summary line that starts with `head`, or "".
pub fn summary_line(d: &Value, head: &str) -> String {
    k2_mpc_av::set::summary(d)
        .lines()
        .find(|l| l.starts_with(head))
        .unwrap_or("")
        .to_owned()
}

/// Reads `files` with configuration `config`.
pub fn read_with(config: &str, files: &[(&str, &Built)]) -> Value {
    let files: Vec<(String, Vec<u8>)> = files
        .iter()
        .map(|(n, b)| ((*n).to_owned(), b.bytes.clone()))
        .collect();
    match k2_mpc_av::read_files(config, &files) {
        Ok(o) => o.data,
        Err(e) => panic!("the set refused: {e}"),
    }
}

/// An attribute definition (a block's template): its tag at `at`.
pub fn attdef(at: [f64; 2], tag: &str) -> Ent {
    let (x, y) = (
        k2_mpc_dxf::testgen::num(at[0]),
        k2_mpc_dxf::testgen::num(at[1]),
    );
    Ent::raw(
        "ATTDEF",
        &[
            (100, "AcDbText"),
            (10, x.as_str()),
            (20, y.as_str()),
            (30, "0.0"),
            (40, "0.25"),
            (1, ""),
            (100, "AcDbAttributeDefinition"),
            (3, tag),
            (2, tag),
            (70, "0"),
        ],
    )
}

/// Sheet numbers in two variants of the per-view block (`SHEETNO` by `SHEET_NO`, `SHEETNO2` by
/// `NO`) and an unrelated block that also carries `SHEET_NO`.
pub const CONFIG_NUMBERS: &str = "\
title.sheet_block = SHEETNO, SHEETNO2
title.sheet_tag = SHEET_NO, NO
title.name_tag = SHEET_NAME, NAME
";

/// Three sheets: the first by `SHEETNO` (`11`, beside the unrelated block's `99`), the second by
/// `SHEETNO2` (`12`), the third by both, disagreeing (`13`, `14`).
pub fn numbered_sheets() -> [Built; 3] {
    let with = || {
        base()
            .block(
                "SHEETNO",
                [0.0, 0.0],
                vec![Ent::line([0.0, 0.0], [2.0, 0.0])],
            )
            .block(
                "SHEETNO2",
                [0.0, 0.0],
                vec![Ent::line([0.0, 0.0], [3.0, 0.0])],
            )
            .block("OTHER", [0.0, 0.0], vec![Ent::point([0.0, 0.0])])
    };
    let a = with()
        .paper(Ent::insert("OTHER", [0.0, 0.0]).attrib("SHEET_NO", "99"))
        .paper(
            Ent::insert("SHEETNO", [10.0, 0.0])
                .attrib("SHEET_NO", "11")
                .attrib("SHEET_NAME", "RISER"),
        )
        .build();
    let b = with()
        .paper(
            Ent::insert("SHEETNO2", [10.0, 0.0])
                .attrib("NO", "12")
                .attrib("NAME", "PLAN"),
        )
        .build();
    let c = with()
        .paper(Ent::insert("SHEETNO", [10.0, 0.0]).attrib("SHEET_NO", "13"))
        .paper(Ent::insert("SHEETNO2", [20.0, 0.0]).attrib("NO", "14"))
        .build();
    [a, b, c]
}

/// A title block shared by reference whose rows are `REVISION_n` and `DATE_n`.
pub const CONFIG_NESTED: &str = "\
title.block = TITLE
title.sheet_block = SHEETNO
title.sheet_tag = SHEET_NO
title.rev_prefix = REVISION
title.date_prefix = DATE
";

/// A sheet that references the title block drawing `xrefs/tb.dxf` and carries a copy of its
/// content inside the reference's block (as a converter writes it): the copy's rows are
/// `REVISION_1` `A` and `REVISION_2` `B`.
pub fn nested_title_sheet() -> Built {
    let mut s = base().block(
        "SHEETNO",
        [0.0, 0.0],
        vec![Ent::line([0.0, 0.0], [2.0, 0.0])],
    );
    s.blocks.push(k2_mpc_dxf::testgen::Block {
        name: "TB".into(),
        base: [0.0, 0.0],
        ents: vec![Ent::insert("TITLE", [0.0, 0.0])
            .attrib("REVISION_1", "A")
            .attrib("DATE_1", "2026-01-05")
            .attrib("REVISION_2", "B")
            .attrib("DATE_2", "2026-02-01")],
        flags: 4,
        path: "..\\xrefs\\tb.dwg".into(),
        rep_of: None,
    });
    s.paper(Ent::insert("TB", [0.0, 0.0]))
        .paper(Ent::insert("SHEETNO", [100.0, 5.0]).attrib("SHEET_NO", "201"))
        .build()
}

/// The title block drawing itself: three dated revisions, the copy's two and a third.
pub fn title_block_drawing() -> Built {
    base()
        .paper(
            Ent::insert("TITLE", [0.0, 0.0])
                .attrib("REVISION_1", "A")
                .attrib("DATE_1", "2026-01-05")
                .attrib("REVISION_2", "B")
                .attrib("DATE_2", "2026-02-01")
                .attrib("REVISION_3", "C")
                .attrib("DATE_3", "2026-03-09"),
        )
        .build()
}

/// One-kind flags whose configured identifier tag holds a direction (`DIR`: `TO`, `FROM`)
/// while the pairing value sits in the other tag (`LABEL`).
pub const CONFIG_FLAG_DIR: &str = "\
flag.blocks = OPC
flag.id_tag = DIR
";

/// Two sheets of `OPC` flags: `DIR` then `LABEL`; one flag with `LABEL` only.
pub fn flag_dir_sheets() -> [Built; 2] {
    let with = || base().block("OPC", [0.0, 0.0], vec![Ent::circle([0.0, 0.0], 0.5)]);
    let f = |dir: &str, label: &str, y: f64| {
        let e = Ent::insert("OPC", [0.0, y]);
        let e = if dir.is_empty() {
            e
        } else {
            e.attrib("DIR", dir)
        };
        e.attrib("LABEL", label)
    };
    let a = with()
        .model(f("TO", "7", 0.0))
        .model(f("TO", "K", 2.0))
        .model(f("FROM", "Q", 4.0))
        .model(f("", "Z", 6.0))
        .build();
    let b = with()
        .model(f("FROM", "7", 0.0))
        .model(f("FROM", "K", 2.0))
        .build();
    [a, b]
}

/// Two device tags, the second spelled differently.
pub const CONFIG_DEVICES: &str = "\
device.tags = DEVICE, DEVICE_ID
device.digits = 1-2
";

/// Devices whose first tag is empty, or holds a value that isn't an identifier for one reason
/// each: lowercase, another separator, no separator, a suffix, too many letters, other.
pub fn device_sheet() -> Built {
    let d = |y: f64, l: &str| Ent::insert("DEV", [0.0, y]).label(l);
    base()
        .model(
            d(0.0, "r1")
                .attrib("DEVICE", "")
                .attrib("DEVICE_ID", "CAM-01"),
        )
        .model(d(2.0, "r2").attrib("DEVICE", ""))
        .model(d(4.0, "r3").attrib("DEVICE", "cam-02"))
        .model(d(6.0, "r4").attrib("DEVICE", "CAM_03"))
        .model(d(8.0, "r5").attrib("DEVICE", "CAM04"))
        .model(d(10.0, "r6").attrib("DEVICE", "CAM-05A"))
        .model(d(12.0, "r7").attrib("DEVICE", "CAMERA-06"))
        .model(d(14.0, "r8").attrib("DEVICE", "?"))
        .build()
}

/// A run drawn as a conduit block whose size and label templates sit above it, ending at the
/// edges of two device symbols (circles of radius 2), labelled by its own attribute.
pub const CONFIG_SYMBOLS: &str = "\
device.tags = DEVICE
cable.blocks = RUNA
cable.label_tags = LABEL1, LABEL2
cable.label_letters = 1-3
cable.label_digits = 3
";

pub fn symbol_sheet() -> Built {
    base()
        .block("SYM", [0.0, 0.0], vec![Ent::circle([0.0, 0.0], 2.0)])
        .block(
            "RUNA",
            [0.0, 0.0],
            vec![
                Ent::line([0.0, -0.05], [1.0, -0.05]),
                Ent::line([0.0, 0.05], [1.0, 0.05]),
                attdef([0.5, 3.0], "SIZE"),
                attdef([0.5, 4.0], "LABEL1"),
            ],
        )
        .model(Ent::insert("SYM", [0.0, 0.0]).attrib("DEVICE", "AB-01"))
        .model(Ent::insert("SYM", [30.0, 0.0]).attrib("DEVICE", "BC-01"))
        .model(
            Ent::insert("RUNA", [2.0, 0.0])
                .placed(26.0, 1.0, 0.0)
                .attrib("SIZE", "1IN")
                .attrib("LABEL1", "")
                .attrib("LABEL2", "C-401")
                .label("run"),
        )
        .build()
}

/// Flags that don't pair: each a labelled end (`LABEL`) of one kind (`KIND`).
pub const CONFIG_FLAG_ENDS: &str = "\
device.tags = DEVICE
flag.blocks = OPC
flag.kind_tag = KIND
flag.label_tag = LABEL
title.sheet_block = SHEETNO
title.sheet_tag = SHEET_NO
title.name_tag = SHEET_NAME
";

/// Two sheets: the first with a device and four flags (labelled with the device, with the
/// second sheet's name, with nothing, and with a place, the last of no kind); the second titled.
pub fn flag_end_sheets() -> [Built; 2] {
    let with = || {
        base()
            .block("OPC", [0.0, 0.0], vec![Ent::circle([0.0, 0.0], 0.5)])
            .block(
                "SHEETNO",
                [0.0, 0.0],
                vec![Ent::line([0.0, 0.0], [2.0, 0.0])],
            )
    };
    let a = with()
        .model(Ent::insert("DEV", [50.0, 0.0]).attrib("DEVICE", "AB-01"))
        .model(
            Ent::insert("OPC", [0.0, 0.0])
                .attrib("KIND", "X")
                .attrib("LABEL", "AB-01"),
        )
        .model(
            Ent::insert("OPC", [0.0, 2.0])
                .attrib("KIND", "X")
                .attrib("LABEL", "RISER"),
        )
        .model(
            Ent::insert("OPC", [0.0, 4.0])
                .attrib("KIND", "X")
                .attrib("LABEL", ""),
        )
        .model(Ent::insert("OPC", [0.0, 6.0]).attrib("LABEL", "ELSEWHERE"))
        .paper(Ent::insert("SHEETNO", [0.0, 0.0]).attrib("SHEET_NO", "1"))
        .build();
    let b = with()
        .paper(
            Ent::insert("SHEETNO", [0.0, 0.0])
                .attrib("SHEET_NO", "2")
                .attrib("SHEET_NAME", "RISER"),
        )
        .build();
    [a, b]
}

/// Cables whose ends meet no device within a small snap distance (0.25), at known distances
/// from device symbols (circles of radius 1), two of them meeting each other.
pub const CONFIG_OPEN: &str = "\
device.tags = DEVICE
cable.layers = AV-CABLE
cable.snap = 0.25
";

pub fn open_end_sheet() -> Built {
    let cable = |a: [f64; 2], b: [f64; 2]| Ent::line(a, b).on("AV-CABLE");
    base()
        .block("SYM", [0.0, 0.0], vec![Ent::circle([0.0, 0.0], 1.0)])
        .model(Ent::insert("SYM", [0.0, 0.0]).attrib("DEVICE", "AB-01"))
        .model(Ent::insert("SYM", [100.0, 0.0]).attrib("DEVICE", "BC-01"))
        // 0.4 from a symbol; its other end meets the next cable's start
        .model(cable([1.4, 0.0], [20.0, 0.0]))
        .model(cable([20.0, 0.0], [20.0, 10.0]))
        // 4 and 0.5 from the second symbol
        .model(cable([95.0, 0.0], [98.5, 0.0]))
        .build()
}

/// Revision and date tags on a title block, all empty, beside a numbered tag of another prefix.
pub const CONFIG_REVS: &str = "\
title.block = TITLE
title.rev_prefix = REV
title.date_prefix = DATE
";

pub fn unrevised_sheet() -> Built {
    base()
        .paper(
            Ent::insert("TITLE", [0.0, 0.0])
                .attrib("REV1", "")
                .attrib("DATE1", "")
                .attrib("REV2", "")
                .attrib("DATE2", "")
                .attrib("ISSUE1", "FOR REVIEW"),
        )
        .build()
}

/// A sheet's number from its sheet block, else its paper layout's name, else its file's name.
pub const CONFIG_NUMBER_FROM: &str = "\
title.sheet_block = SHEETNO
title.sheet_tag = SHEET_NO
title.number_from = blocks, layout, file-stem
";

/// Three sheets: the first numbered by its block (`11`), the second by its layout's name
/// (`AV-202`), the third by its file's name (its layout keeps the default name).
pub fn number_from_sheets() -> [Built; 3] {
    let with = || {
        base().block(
            "SHEETNO",
            [0.0, 0.0],
            vec![Ent::line([0.0, 0.0], [2.0, 0.0])],
        )
    };
    let a = with()
        .paper(Ent::insert("SHEETNO", [0.0, 0.0]).attrib("SHEET_NO", "11"))
        .build();
    let mut b = with().paper(Ent::line([0.0, 0.0], [1.0, 0.0]));
    b.draw.layout = Some("AV-202".into());
    let c = with().paper(Ent::line([0.0, 0.0], [1.0, 0.0])).build();
    [a, b.build(), c]
}

/// Cables that end at flags (labelled off-sheet ends) rather than at devices.
pub const CONFIG_FLAG_TERMINALS: &str = "\
device.tags = DEVICE
cable.layers = AV-CABLE
cable.snap = 0.25
flag.blocks = OPC
flag.label_tag = LABEL
";

/// A device symbol (radius 1) at the origin, a flag beside it, flags labelled `PANEL 3` at 30,
/// unlabelled at 60 and `X` at 100 (symbols of radius 0.5), and three cables: the first from
/// the device to the first flag, the second from open space to the unlabelled flag, the third
/// open at both ends, between flags.
pub fn flag_terminal_sheet() -> Built {
    let cable = |a: [f64; 2], b: [f64; 2], l: &str| Ent::line(a, b).on("AV-CABLE").label(l);
    let flag = |x: f64, label: &str| {
        let f = Ent::insert("OPC", [x, 0.0]);
        if label.is_empty() {
            f
        } else {
            f.attrib("LABEL", label)
        }
    };
    base()
        .block("SYM", [0.0, 0.0], vec![Ent::circle([0.0, 0.0], 1.0)])
        .block("OPC", [0.0, 0.0], vec![Ent::circle([0.0, 0.0], 0.5)])
        .model(Ent::insert("SYM", [0.0, 0.0]).attrib("DEVICE", "AB-01"))
        .model(flag(1.2, "NEAR"))
        .model(flag(30.0, "PANEL 3"))
        .model(flag(60.0, ""))
        .model(flag(100.0, "X"))
        .model(cable([1.0, 0.0], [29.6, 0.0], "c1"))
        .model(cable([40.0, 0.0], [59.8, 0.0], "c2"))
        .model(cable([80.0, 0.0], [85.0, 0.0], "c3"))
        .build()
}
