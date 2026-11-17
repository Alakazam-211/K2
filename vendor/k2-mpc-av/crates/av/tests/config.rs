//! The project's configuration: every key read, an unknown key or a wrong value refused,
//! naming the accepted keys.

mod support;

use k2_mpc_av::Config;

#[test]
pub fn config_reads_and_refuses() {
    let c = Config::parse(support::CONFIG).expect("the example reads");
    assert_eq!(c.device_tags, ["DEVICE"]);
    assert_eq!(c.boundary_ltypes, ["HIDDEN"]);
    assert_eq!(c.title_block.as_deref(), Some("TITLE"));
    let o = Config::parse(support::CONFIG_ONE_KIND).expect("the one-kind example reads");
    assert_eq!(o.flag_blocks, ["OPC"]);
    let f = Config::parse(support::CONFIG_NUMBER_FROM).expect("the number sources read");
    assert_eq!(f.number_from, ["blocks", "layout", "file-stem"]);
    assert!(Config::parse("title.number_from = blocks, page").is_err());
    let r = Config::parse(support::CONFIG_RUNS).expect("the runs example reads");
    assert_eq!(r.cable_blocks, ["RUN"]);
    let x = Config::parse(support::CONFIG_XREF).expect("the shared-title example reads");
    assert_eq!(
        (x.title_sheet_blocks.as_slice(), x.date_prefix.as_deref()),
        (&["SHEETNO".to_owned()][..], Some("DATE"))
    );
    assert_eq!((c.letters, c.digits), ((2, 4), (2, 3)), "the defaults");
    assert!(c.is_device_id("CAM-01") && c.is_device_id("TXR-123"));
    assert!(!c.is_device_id("cam-01") && !c.is_device_id("CAM01") && !c.is_device_id("C-01"));
    assert_eq!(
        (c.label_letters, c.label_digits),
        (Some((1, 3)), Some((3, 3)))
    );
    assert!(c.is_label("C-101") && !c.is_label("NOTE") && !c.is_label("C-1"));
    assert!(
        Config::default().is_label("NOTE"),
        "without a shape, any text labels"
    );
    for (bad, why) in [
        ("device.colour = red", "no key"),
        ("device.letters = 4-2", "a count or a range"),
        ("cable.snap = -1", "a distance"),
        ("cable.label_digits = three", "a count or a range"),
        ("device.tags = A\ndevice.tags = B", "given twice"),
        ("just words", "not key = value"),
    ] {
        let e = Config::parse(bad)
            .err()
            .unwrap_or_else(|| panic!("{bad:?} read"));
        assert_eq!(e.code.id, "AV-E001", "{bad:?}");
        assert!(e.why.contains(why), "{bad:?}: {}", e.why);
        assert!(
            e.next.contains("device.tags") && e.next.contains("title.rev_prefix"),
            "{}",
            e.next
        );
    }
}
