//! The container over synthetic files from the test builder (`support/container.rs`): the file
//! header and its copy, the page map with gaps, sections over several pages, each page's
//! checks, the encrypted and unsupported refusals, and the builder's app-info held to our own
//! writer name.

mod support;

use k2_mpc_dwg::{read_container, read_section, Container, Limits};
use support::container::{build, sec, standard, Spec, ALLOWED_WRITERS, APP_INFO};
use support::refused;

fn read(bytes: &[u8]) -> Container {
    read_container(bytes, &Limits::default())
        .unwrap_or_else(|e| panic!("the container refused: {}", e.summary()))
}

fn section(bytes: &[u8], c: &Container, name: &str) -> Vec<u8> {
    read_section(bytes, c, name, &Limits::default())
        .unwrap_or_else(|e| panic!("section {name}: {}", e.summary()))
}

fn every_section_reads_back(spec: &Spec, bytes: &[u8], c: &Container) {
    for s in &spec.sections {
        assert!(
            section(bytes, c, &s.name) == s.data,
            "section {} reads back",
            s.name
        );
    }
}

#[test]
pub fn header_and_its_copy() {
    let spec = standard();
    let built = build(&spec);
    let c = read(&built.bytes);
    assert_eq!(
        (
            c.version.as_str(),
            c.maintenance,
            c.code_page,
            c.security_flags
        ),
        ("AC1032", 0, 30, 0)
    );
    // a changed byte in the masked header, in both copies: only the CRC can see it
    let mut damaged = built.bytes.clone();
    let n = damaged.len();
    damaged[0x80 + 0x20] ^= 0x01;
    damaged[n - 0x6C + 0x20] ^= 0x01;
    refused(
        read_container(&damaged, &Limits::default()),
        "DWG-E006",
        "a header byte changed in both copies",
    );
    // the second copy alone changed
    let mut damaged = built.bytes.clone();
    damaged[n - 1] ^= 0x01;
    refused(
        read_container(&damaged, &Limits::default()),
        "DWG-E006",
        "the header's two copies differ",
    );
}

#[test]
pub fn page_map_with_gaps() {
    let mut spec = standard();
    spec.gaps_after = vec![0, 3];
    let built = build(&spec);
    let c = read(&built.bytes);
    let gaps: Vec<(u64, u64)> = c.gaps.iter().map(|g| (g.address, g.size)).collect();
    assert_eq!(gaps, built.gaps, "the gaps, where the builder put them");
    let placed: Vec<(u32, u64)> = built
        .data_pages
        .iter()
        .map(|&(n, _)| {
            let p = c
                .pages
                .iter()
                .find(|p| p.number == n)
                .expect("in the page map");
            (n, p.address)
        })
        .collect();
    assert_eq!(
        placed, built.data_pages,
        "each data page's address, past the gaps"
    );
    every_section_reads_back(&spec, &built.bytes, &c);
}

#[test]
pub fn sections_over_several_pages() {
    let spec = standard();
    let built = build(&spec);
    let c = read(&built.bytes);
    let rows: Vec<(&str, u32, u64, usize, bool)> = c
        .sections
        .iter()
        .map(|s| (s.name.as_str(), s.id, s.size, s.pages.len(), s.compressed))
        .collect();
    assert_eq!(
        rows,
        [
            ("AcDb:Header", 1, 300, 3, true),
            ("AcDb:Classes", 2, 40, 1, false),
            ("AcDb:AcDbObjects", 3, 1000, 3, true),
            (APP_INFO, 4, 44, 1, false),
        ],
        "the section map"
    );
    let starts: Vec<u64> = c.sections[2].pages.iter().map(|p| p.start).collect();
    assert_eq!(starts, [0, 400, 800], "each page's start in its section");
    every_section_reads_back(&spec, &built.bytes, &c);
}

#[test]
pub fn page_checks() {
    let spec = standard();
    let built = build(&spec);
    let c = read(&built.bytes);
    let classes = c.section("AcDb:Classes").expect("listed").pages[0].number;
    let at = c
        .pages
        .iter()
        .find(|p| p.number == classes)
        .expect("placed")
        .address as usize;
    // a byte of an uncompressed page's data: only its checksums can see it
    let mut damaged = built.bytes.clone();
    damaged[at + 32 + 5] ^= 0x01;
    refused(
        read_section(&damaged, &c, "AcDb:Classes", &Limits::default()),
        "DWG-E006",
        "a data byte changed",
    );
    // a byte of a page's masked header
    let header = c.section("AcDb:Header").expect("listed").pages[1].number;
    let at = c
        .pages
        .iter()
        .find(|p| p.number == header)
        .expect("placed")
        .address as usize;
    let mut damaged = built.bytes.clone();
    damaged[at + 8] ^= 0x01;
    refused(
        read_section(&damaged, &c, "AcDb:Header", &Limits::default()),
        "DWG-E006",
        "a page header byte changed",
    );
}

#[test]
pub fn encrypted_refused() {
    let mut spec = standard();
    spec.security = 1;
    refused(
        read_container(&build(&spec).bytes, &Limits::default()),
        "DWG-E007",
        "the security flags set",
    );
    let mut spec = standard();
    spec.sections[1].encrypted = true;
    refused(
        read_container(&build(&spec).bytes, &Limits::default()),
        "DWG-E007",
        "an encrypted section",
    );
    let mut spec = standard();
    spec.sections
        .push(sec("AcDb:Security", 9, vec![1, 2, 3, 4], 0x7400, false));
    refused(
        read_container(&build(&spec).bytes, &Limits::default()),
        "DWG-E007",
        "a security section",
    );
}

#[test]
pub fn versions() {
    for v in [b"AC1018", b"AC1024", b"AC1027", b"AC1032"] {
        let mut spec = standard();
        spec.version = *v;
        let c = read(&build(&spec).bytes);
        assert_eq!(c.version.as_bytes(), v, "the 2004 to 2018 family reads");
    }
    for (v, id) in [
        (b"AC1021", "DWG-E008"),
        (b"AC1015", "DWG-E008"),
        (b"AC1009", "DWG-E008"),
        (b"MC0.0\0", "DWG-E009"),
    ] {
        let mut spec = standard();
        spec.version = *v;
        refused(
            read_container(&build(&spec).bytes, &Limits::default()),
            id,
            &String::from_utf8_lossy(v),
        );
    }
}

#[test]
pub fn app_info_is_ours_alone() {
    let built = build(&standard());
    let c = read(&built.bytes);
    let named: Vec<&str> = c
        .sections
        .iter()
        .map(|s| s.name.as_str())
        .filter(|n| n.contains("AppInfo"))
        .collect();
    assert_eq!(
        named,
        [APP_INFO],
        "one app-info section, no history beside it"
    );
    let raw = section(&built.bytes, &c, APP_INFO);
    let units: Vec<u16> = raw
        .chunks(2)
        .map(|p| u16::from_le_bytes([p[0], *p.get(1).unwrap_or(&0)]))
        .collect();
    let value = String::from_utf16_lossy(&units);
    assert!(
        ALLOWED_WRITERS.contains(&value.as_str()),
        "the app-info holds {value:?}, not one of {ALLOWED_WRITERS:?}"
    );
}
