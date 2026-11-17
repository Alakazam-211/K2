//! A two-sheet AV set read together: devices across sheets, the cables and the wire pull list,
//! flag pairs, the sheet index, and the findings.

mod support;

use support::*;

#[test]
pub fn devices_across_sheets() {
    let (v, _, d) = read();
    let devices = list(&d, "devices");
    let ids: Vec<&str> = devices.iter().map(|x| s(x, "id")).collect();
    assert_eq!(
        ids,
        ["CAM-01", "TX-01"],
        "one device per identifier, across sheets"
    );
    let tx = &devices[1];
    assert_eq!(strs(tx, "sheets"), ["video.dxf", "control.dxf"]);
    assert_eq!(s(&list(tx, "places")[0], "insert"), v.addr("tx"));
    let malformed: Vec<&str> = list(&d, "findings")
        .iter()
        .filter(|x| s(x, "code") == "device_id_malformed")
        .map(|x| s(x, "address"))
        .collect();
    assert_eq!(malformed, [v.addr("bad").as_str()]);
}

#[test]
pub fn cables_with_ends_and_labels() {
    let (v, _, d) = read();
    let cables = list(&d, "cables");
    let rows: Vec<(String, &str, &str, &str, Vec<String>)> = cables
        .iter()
        .map(|x| {
            (
                s(x, "cable").to_owned(),
                s(x, "label"),
                s(x, "from"),
                s(x, "to"),
                strs(x, "boundaries"),
            )
        })
        .collect();
    let want: Vec<(String, &str, &str, &str, Vec<String>)> = vec![
        (
            v.addr("c1"),
            "C-101",
            "CAM-01",
            "TX-01",
            vec!["RACK A".into()],
        ),
        (v.addr("inside"), "", "CAM-01", "", vec![]),
        (v.addr("c2"), "C-102", "TX-01", "", vec!["ROOM B".into()]),
    ];
    assert_eq!(
        rows,
        want,
        "every cable, crossing or not; the note on the inside cable isn't a label's shape: {}",
        d.to_json()
    );
    let ends: Vec<Vec<f64>> = list(&cables[1], "ends").iter().map(list_f64).collect();
    assert_eq!(
        ends,
        [vec![5.0, 5.0], vec![10.0, 10.0]],
        "the inside cable's ends"
    );
}

#[test]
pub fn cable_findings() {
    let (v, _, d) = read();
    let of = |code: &str| -> Vec<String> {
        let mut a: Vec<String> = list(&d, "findings")
            .iter()
            .filter(|x| s(x, "code") == code)
            .map(|x| format!("{} {}", s(x, "address"), s(x, "message")))
            .collect();
        a.sort();
        a
    };
    let mut open = vec![
        format!(
            "{} its to end at (10, 10) reaches no device within 1",
            v.addr("inside")
        ),
        format!(
            "{} its to end at (110, 5) reaches no device within 1",
            v.addr("c2")
        ),
    ];
    open.sort();
    assert_eq!(of("cable_end_open"), open, "{}", d.to_json());
    assert_eq!(
        of("cable_unlabelled"),
        [format!("{} no label within 2 of it", v.addr("inside"))]
    );
}

#[test]
pub fn pulls_from_crossings() {
    let (v, _, d) = read();
    let pulls = list(&d, "pulls");
    assert_eq!(
        pulls.len(),
        2,
        "two cables cross a boundary: {}",
        d.to_json()
    );
    let p1 = &pulls[0];
    assert_eq!(
        (s(p1, "cable"), s(p1, "label")),
        (v.addr("c1").as_str(), "C-101")
    );
    assert_eq!((s(p1, "from"), s(p1, "to")), ("CAM-01", "TX-01"));
    assert_eq!(
        strs(p1, "boundaries"),
        ["RACK A"],
        "the rack, a boundary by its layer"
    );
    let p2 = &pulls[1];
    assert_eq!(
        (s(p2, "cable"), s(p2, "label")),
        (v.addr("c2").as_str(), "C-102")
    );
    assert_eq!(s(p2, "from"), "TX-01");
    assert!(
        p2.get("to") == Some(&k2_mpc_base::errors::Value::Null),
        "no device at its far end"
    );
    assert_eq!(
        strs(p2, "boundaries"),
        ["ROOM B"],
        "the room, a boundary by its linetype"
    );
}

#[test]
pub fn flags_paired() {
    let (v, c, d) = read();
    let pairs = list(&d, "flags");
    assert_eq!(pairs.len(), 1, "{}", d.to_json());
    assert_eq!(s(&pairs[0], "id"), "F1");
    let out = pairs[0].get("out").unwrap();
    let inn = pairs[0].get("in").unwrap();
    assert_eq!(
        (s(out, "sheet"), s(out, "flag")),
        ("video.dxf", v.addr("f1").as_str())
    );
    assert_eq!(
        (s(inn, "sheet"), s(inn, "flag")),
        ("control.dxf", c.addr("f1").as_str())
    );
    let mut unpaired: Vec<&str> = list(&d, "findings")
        .iter()
        .filter(|x| s(x, "code") == "flag_unpaired")
        .map(|x| s(x, "address"))
        .collect();
    unpaired.sort();
    let mut want = vec![v.addr("f9"), c.addr("f2")];
    want.sort();
    assert_eq!(unpaired, want);
}

#[test]
pub fn latest_revision() {
    let (_, _, d) = read();
    let sheets = list(&d, "sheets");
    let rows: Vec<(&str, &str, &str)> = sheets
        .iter()
        .map(|x| (s(x, "number"), s(x, "name"), s(x, "revision")))
        .collect();
    assert_eq!(
        rows,
        [("1", "VIDEO", "B"), ("2", "CONTROL", "A")],
        "the latest revision, an empty row skipped"
    );
}

#[test]
pub fn a_sheet_without_a_title_block() {
    let notes = k2_mpc_dxf::testgen::Spec::new(k2_mpc_dxf::testgen::Ver::R2018).build();
    let files = vec![
        ("control.dxf".to_owned(), control().bytes),
        ("notes.dxf".to_owned(), notes.bytes),
    ];
    let d = k2_mpc_av::read_files(CONFIG, &files).unwrap().data;
    let missing: Vec<&str> = list(&d, "findings")
        .iter()
        .filter(|x| s(x, "code") == "title_missing")
        .map(|x| s(x, "sheet"))
        .collect();
    assert_eq!(missing, ["notes.dxf"]);
    let e = k2_mpc_av::read_files(
        CONFIG,
        &[(
            "bin.dxf".to_owned(),
            b"AutoCAD Binary DXF\r\n\x1a\x00".to_vec(),
        )],
    )
    .expect_err("a binary file refused");
    assert_eq!(e.code.id, "AV-E002");
}

#[test]
pub fn title_block_by_reference() {
    let d = k2_mpc_av::read_files(CONFIG_XREF, &xref_files())
        .expect("the set reads")
        .data;
    let sheet = list(&d, "sheets")
        .iter()
        .find(|x| s(x, "sheet") == "sheets/av-101.dxf")
        .unwrap_or_else(|| panic!("no sheet row: {}", d.to_json()));
    assert_eq!(
        (
            s(sheet, "number"),
            s(sheet, "name"),
            s(sheet, "revision"),
            s(sheet, "revision_date"),
            s(sheet, "title_from")
        ),
        (
            "101",
            "VIDEO RISER",
            "B",
            "2026-02-01",
            "xrefs/titleblock.dxf"
        ),
        "the number from the sheet's own block, the latest dated revision from the title block \
         it references (resolved by its path, not the stale copy's): {}",
        d.to_json()
    );
}

#[test]
pub fn cables_labelled_by_leaders() {
    let b = leader_sheet();
    let d = k2_mpc_av::read_files(CONFIG, &[("leaders.dxf".to_owned(), b.bytes.clone())])
        .expect("the set reads")
        .data;
    let rows: Vec<(String, &str)> = list(&d, "cables")
        .iter()
        .map(|x| (s(x, "cable").to_owned(), s(x, "label")))
        .collect();
    assert_eq!(
        rows,
        [(b.addr("c1"), "C-201"), (b.addr("c2"), "C-202")],
        "each cable labelled by the leader whose arrow (on any of its lines) touches it, by its \
         plain text, not by the text nearest it: {}",
        d.to_json()
    );
}

#[test]
pub fn cables_drawn_as_blocks() {
    let b = run_sheet();
    let d = k2_mpc_av::read_files(CONFIG_RUNS, &[("runs.dxf".to_owned(), b.bytes.clone())])
        .expect("the set reads")
        .data;
    let cables = list(&d, "cables");
    assert_eq!(cables.len(), 1, "{}", d.to_json());
    let c = &cables[0];
    assert_eq!(
        (
            s(c, "cable"),
            s(c, "block"),
            s(c, "label"),
            s(c, "from"),
            s(c, "to"),
            strs(c, "boundaries")
        ),
        (
            b.addr("run").as_str(),
            "RUN",
            "C-301",
            "AB-01",
            "BC-01",
            vec!["ROOM C".to_owned()]
        ),
        "a run drawn as a block: its ends from its block as placed, through the room: {}",
        d.to_json()
    );
    let attribs = c.get("attribs").expect("its attributes");
    assert_eq!((s(attribs, "SIZE"), s(attribs, "FILL")), ("1IN", "40%"));
}

#[test]
pub fn reference_drawings() {
    let d = k2_mpc_av::read_files(CONFIG_XREF, &xref_files())
        .expect("the set reads")
        .data;
    let none = Vec::new();
    let refs: Vec<(&str, Vec<String>)> = match d.get("references") {
        Some(k2_mpc_base::errors::Value::List(l)) => l,
        _ => &none,
    }
    .iter()
    .map(|x| (s(x, "sheet"), strs(x, "referenced_by")))
    .collect();
    let by = vec![
        "sheets/av-101.dxf".to_owned(),
        "sheets/av-102.dxf".to_owned(),
    ];
    assert_eq!(
        refs,
        [
            ("xrefs/backdrop.dxf", by.clone()),
            ("xrefs/titleblock.dxf", by)
        ],
        "a drawing the set's sheets reference, with no sheet title of its own: {}",
        d.to_json()
    );
    let sheets: Vec<&str> = list(&d, "sheets").iter().map(|x| s(x, "sheet")).collect();
    assert_eq!(
        sheets,
        [
            "old/titleblock.dxf",
            "sheets/av-101.dxf",
            "sheets/av-102.dxf"
        ],
        "a titled sheet another references stays a sheet"
    );
    let missing: Vec<&str> = list(&d, "findings")
        .iter()
        .filter(|x| s(x, "code") == "title_missing")
        .map(|x| s(x, "sheet"))
        .collect();
    assert!(missing.is_empty(), "{missing:?}");
}

#[test]
pub fn flags_of_one_kind() {
    let (a, b) = one_kind_files();
    let files = vec![
        ("a.dxf".to_owned(), a.bytes.clone()),
        ("b.dxf".to_owned(), b.bytes.clone()),
    ];
    let d = k2_mpc_av::read_files(CONFIG_ONE_KIND, &files)
        .expect("the set reads")
        .data;
    let flag = |x: &k2_mpc_base::errors::Value, end: &str| -> String {
        x.get(end)
            .map(|f| format!("{} {}", s(f, "sheet"), s(f, "flag")))
            .unwrap_or_default()
    };
    let pairs: Vec<(String, String, String)> = list(&d, "flags")
        .iter()
        .map(|x| (s(x, "id").to_owned(), flag(x, "a"), flag(x, "b")))
        .collect();
    let at =
        |sheet: &str, b: &k2_mpc_dxf::testgen::Built, l: &str| format!("{sheet} {}", b.addr(l));
    assert_eq!(
        pairs,
        [
            (
                "7".to_owned(),
                at("a.dxf", &a, "seven"),
                at("b.dxf", &b, "seven")
            ),
            ("A".to_owned(), at("a.dxf", &a, "a"), at("b.dxf", &b, "a"))
        ],
        "one kind of flag, paired by value, with no direction: {}",
        d.to_json()
    );
    let unpaired: Vec<&str> = list(&d, "findings")
        .iter()
        .filter(|x| s(x, "code") == "flag_unpaired")
        .map(|x| s(x, "address"))
        .collect();
    assert_eq!(unpaired, [a.addr("q").as_str()]);
    let mut without: Vec<String> = list(&d, "findings")
        .iter()
        .filter(|x| s(x, "code") == "flag_without_id")
        .map(|x| format!("{} {}", s(x, "sheet"), s(x, "address")))
        .collect();
    without.sort();
    assert_eq!(
        without,
        [
            format!("a.dxf {}", a.addr("blank")),
            format!("b.dxf {}", b.addr("blank"))
        ],
        "an empty value is no identifier: two blanks never pair"
    );
}

#[test]
pub fn summary_counts_never_values() {
    let (_, _, d) = read();
    let out = k2_mpc_av::set::summary(&d);
    for line in [
        "sheets 2 with-number 2 with-name 2 with-revision 2 with-revision-date 0 title-by-reference 0",
        "references 0",
        "devices 2 on-several-sheets 1",
        "cables 3 drawn-as-blocks 0 labelled 2 both-ends-at-devices 1",
        "pulls 2",
        "flags 1 directed 1 one-kind 0",
        "finding flag_unpaired 2",
    ] {
        assert!(out.lines().any(|l| l == line), "no line {line:?} in:\n{out}");
    }
    for value in ["CAM-01", "TX-01", "C-101", "VIDEO", "video.dxf", "F1"] {
        assert!(!out.contains(value), "a value reached the summary: {value}");
    }
}

fn sheet_row<'v>(d: &'v k2_mpc_base::errors::Value, name: &str) -> &'v k2_mpc_base::errors::Value {
    list(d, "sheets")
        .iter()
        .find(|x| s(x, "sheet") == name)
        .unwrap_or_else(|| panic!("no sheet {name}: {}", d.to_json()))
}

fn codes<'v>(d: &'v k2_mpc_base::errors::Value, code: &str) -> Vec<&'v str> {
    list(d, "findings")
        .iter()
        .filter(|x| s(x, "code") == code)
        .map(|x| s(x, "sheet"))
        .collect()
}

#[test]
pub fn sheet_numbers_by_any_configured_tag() {
    let [a, b, c] = numbered_sheets();
    let d = read_with(
        CONFIG_NUMBERS,
        &[("a.dxf", &a), ("b.dxf", &b), ("c.dxf", &c)],
    );
    let got: Vec<(&str, &str)> = ["a.dxf", "b.dxf", "c.dxf"]
        .iter()
        .map(|n| (s(sheet_row(&d, n), "number"), s(sheet_row(&d, n), "name")))
        .collect();
    assert_eq!(
        got,
        [("11", "RISER"), ("12", "PLAN"), ("13", "")],
        "the number from either per-view block by either tag, never the unrelated block: {}",
        d.to_json()
    );
    assert_eq!(
        codes(&d, "sheet_number_many"),
        ["c.dxf"],
        "the two blocks disagree"
    );
}

#[test]
pub fn title_rows_from_the_referenced_drawing() {
    let (sheet, tb) = (nested_title_sheet(), title_block_drawing());
    let d = read_with(
        CONFIG_NESTED,
        &[("sheets/av-201.dxf", &sheet), ("xrefs/tb.dxf", &tb)],
    );
    let r = sheet_row(&d, "sheets/av-201.dxf");
    assert_eq!(
        (
            s(r, "number"),
            s(r, "revision"),
            s(r, "revision_date"),
            s(r, "title_from")
        ),
        ("201", "C", "2026-03-09", "xrefs/tb.dxf"),
        "the rows from the referenced drawing in the set, not the copy inside the reference's \
         block; REVISION_n read by the prefix REVISION: {}",
        d.to_json()
    );
    assert_eq!(
        summary_line(&d, "title-chain"),
        "title-chain sheets 1 own 0 by-reference 1 copy-in-reference 0 placing-a-resolved-reference 1 with-revision-rows 1"
    );
    // without the referenced drawing, the copy inside the reference's block
    let d = read_with(CONFIG_NESTED, &[("sheets/av-201.dxf", &sheet)]);
    let r = sheet_row(&d, "sheets/av-201.dxf");
    assert_eq!(
        (s(r, "revision"), s(r, "title_from")),
        ("B", "..\\xrefs\\tb.dwg"),
        "{}",
        d.to_json()
    );
}

#[test]
pub fn flag_tags_in_counts() {
    let [a, b] = flag_dir_sheets();
    let d = read_with(CONFIG_FLAG_DIR, &[("a.dxf", &a), ("b.dxf", &b)]);
    for line in [
        "flag-refs 6 with-id-tag 5",
        "flag-tag id values 5 empty 0 distinct 2 paired 1 single 0 more 1 id-shaped 0",
        "flag-tag other-1 values 6 empty 0 distinct 4 paired 2 single 2 more 0 id-shaped 0",
    ] {
        assert!(
            k2_mpc_av::set::summary(&d).lines().any(|l| l == line),
            "no line {line:?} in:\n{}",
            k2_mpc_av::set::summary(&d)
        );
    }
}

#[test]
pub fn device_values_one_finding_per_reference() {
    let b = device_sheet();
    let d = read_with(CONFIG_DEVICES, &[("dev.dxf", &b)]);
    let ids: Vec<&str> = list(&d, "devices").iter().map(|x| s(x, "id")).collect();
    assert_eq!(ids, ["CAM-01"], "{}", d.to_json());
    let mut bad: Vec<&str> = list(&d, "findings")
        .iter()
        .filter(|x| s(x, "code") == "device_id_malformed")
        .map(|x| s(x, "address"))
        .collect();
    bad.sort();
    let mut want: Vec<String> = ["r3", "r4", "r5", "r6", "r7", "r8"]
        .iter()
        .map(|l| b.addr(l))
        .collect();
    want.sort();
    assert_eq!(
        bad, want,
        "an empty value isn't malformed; a reference whose other tag holds its identifier isn't"
    );
    assert_eq!(
        summary_line(&d, "device-tag 1 "),
        "device-tag 1 values 8 empty 2 id-shaped 0 lowercase 1 separator 1 no-separator 1 suffix 1 out-of-range 1 other 1"
    );
    assert_eq!(
        summary_line(&d, "device-tag 2 "),
        "device-tag 2 values 1 empty 0 id-shaped 1 lowercase 0 separator 0 no-separator 0 suffix 0 out-of-range 0 other 0"
    );
}

#[test]
pub fn runs_end_at_device_symbols() {
    let b = symbol_sheet();
    let d = read_with(CONFIG_SYMBOLS, &[("sym.dxf", &b)]);
    let c = &list(&d, "cables")[0];
    assert_eq!(
        (s(c, "cable"), s(c, "from"), s(c, "to"), s(c, "label")),
        (b.addr("run").as_str(), "AB-01", "BC-01", "C-401"),
        "a run's ends at the edges of the device symbols, its label its own attribute: {}",
        d.to_json()
    );
    assert_eq!(
        summary_line(&d, "cable-ends"),
        "cable-ends 2 at-insertion 0 at-symbol 2 at-flag 0 junction 0 open 0"
    );
}

#[test]
pub fn flags_as_labelled_ends() {
    let [a, b] = flag_end_sheets();
    let d = read_with(CONFIG_FLAG_ENDS, &[("p1.dxf", &a), ("p2.dxf", &b)]);
    let flagged: Vec<&str> = list(&d, "findings")
        .iter()
        .map(|x| s(x, "code"))
        .filter(|c| c.starts_with("flag_"))
        .collect();
    assert!(flagged.is_empty(), "labelled ends never pair: {flagged:?}");
    assert_eq!(
        summary_line(&d, "flag-ends"),
        "flag-ends 4 labelled 3 unlabelled 1 label-is-device 1 label-is-sheet 1 kinds 1",
        "{}",
        d.to_json()
    );
}

#[test]
pub fn open_ends_in_counts() {
    let b = open_end_sheet();
    let d = read_with(CONFIG_OPEN, &[("open.dxf", &b)]);
    // two ends meet each other: a junction, the run going on, not an open end
    for line in [
        "cable-ends 6 at-insertion 0 at-symbol 0 at-flag 0 junction 2 open 4",
        "open-ends 4 lt0.5 1 lt1 1 lt3 0 lt6 1 lt12 0 ge12 1 no-device 0 near-flag-12 0 near-flag-24 0",
        "junction-ends 2 lt0.5 0 lt1 0 lt3 0 lt6 0 lt12 0 ge12 2 no-device 0 near-flag-12 0 near-flag-24 0",
    ] {
        assert_eq!(
            summary_line(&d, line.split(' ').next().unwrap_or("")),
            line,
            "{}",
            d.to_json()
        );
    }
    let open = list(&d, "findings")
        .iter()
        .filter(|x| s(x, "code") == "cable_end_open")
        .count();
    assert_eq!(open, 4, "a junction is no open end: {}", d.to_json());
}

#[test]
pub fn revision_tags_in_counts() {
    let b = unrevised_sheet();
    let d = read_with(CONFIG_REVS, &[("rev.dxf", &b)]);
    assert_eq!(
        summary_line(&d, "revision-rows"),
        "revision-rows sheets-with-rows 1 rev-tags 2 rev-filled 0 date-tags 2 date-filled 0 other-numbered-tags 1"
    );
}

#[test]
pub fn sheet_numbers_from_layout_or_file() {
    let [a, b, c] = number_from_sheets();
    let d = read_with(
        CONFIG_NUMBER_FROM,
        &[
            ("sheet-a.dxf", &a),
            ("proj_av-202.dxf", &b),
            ("plan AV-303.dxf", &c),
        ],
    );
    // the index is ordered by number; compare by sheet
    let mut got: Vec<(&str, &str)> = list(&d, "sheets")
        .iter()
        .map(|x| (s(x, "sheet"), s(x, "number")))
        .collect();
    got.sort();
    assert_eq!(
        got,
        [
            ("plan AV-303.dxf", "AV-303"),
            ("proj_av-202.dxf", "AV-202"),
            ("sheet-a.dxf", "11")
        ],
        "the block's number, else the layout's name, else the file's; never a default layout \
         name: {}",
        d.to_json()
    );
    for line in [
        "number-source tag-1 1 layout 1 file-stem 1 none 0",
        "layout-names sheet-like 1 not 2 none 0",
        "file-stems sheet-like 2 not 1",
    ] {
        assert!(
            k2_mpc_av::set::summary(&d).lines().any(|l| l == line),
            "no line {line:?} in:\n{}",
            k2_mpc_av::set::summary(&d)
        );
    }
}

#[test]
pub fn cables_end_at_flags() {
    let b = flag_terminal_sheet();
    let d = read_with(CONFIG_FLAG_TERMINALS, &[("flags.dxf", &b)]);
    let rows: Vec<(String, &str, &str)> = list(&d, "cables")
        .iter()
        .map(|x| (s(x, "cable").to_owned(), s(x, "from"), s(x, "destination")))
        .collect();
    assert_eq!(
        rows,
        [
            (b.addr("c1"), "AB-01", "PANEL 3"),
            (b.addr("c2"), "", ""),
            (b.addr("c3"), "", "")
        ],
        "a device at an end before a flag beside it; a flag's label the cable's destination: {}",
        d.to_json()
    );
    for line in [
        "cable-ends 6 at-insertion 0 at-symbol 1 at-flag 2 junction 0 open 3",
        "open-ends 3 lt0.5 0 lt1 0 lt3 0 lt6 0 lt12 0 ge12 3 no-device 0 near-flag-12 1 near-flag-24 3",
        "cable-flags cables-at-a-flag 2 labelled-by-flag 1",
    ] {
        assert_eq!(
            summary_line(&d, line.split(' ').next().unwrap_or("")),
            line,
            "{}",
            d.to_json()
        );
    }
    let open = list(&d, "findings")
        .iter()
        .filter(|x| s(x, "code") == "cable_end_open")
        .count();
    assert_eq!(open, 3, "an end at a flag is no open end");
}
