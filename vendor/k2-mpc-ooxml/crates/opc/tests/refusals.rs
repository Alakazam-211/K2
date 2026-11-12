//! The refusal rows (bad zips, missing content types, duplicate parts, hostile XML, every
//! limit), and damage never panics.

mod common;

use common::damage::refusals_hold;
use common::{fixtures, limits, Format, Layout, Model};
use k2_mpc_opc::merge::{merge3_bytes, Generic};
use k2_mpc_opc::OpcPackage;

#[test]
fn each_refusal_row_has_its_code() {
    if let Err(e) = refusals_hold() {
        panic!("{e}");
    }
}

#[test]
fn damage_never_panics() {
    let l = limits();
    let other = fixtures().swap_remove(0).1;
    for layout in [Layout::Plain, Layout::DescSig, Layout::Stored] {
        let f = common::package(&Model::new(Format::Pptx, 0).render(), layout, 1);
        // every truncation
        for n in 0..f.len() {
            let _ = OpcPackage::open(&f[..n], &l);
        }
        // bytes changed through the file; what opens still writes back to itself
        let mut opened = 0;
        for i in (0..f.len()).step_by(5) {
            for v in [0x00, 0xFF, f[i] ^ 0x01] {
                let mut g = f.clone();
                g[i] = v;
                if let Ok(x) = OpcPackage::open(&g, &l) {
                    assert_eq!(x.encode().expect("encode"), g);
                    opened += 1;
                }
                let _ = merge3_bytes(&f, &f, &g, &Generic, &l);
                let _ = merge3_bytes(&g, &f, &other, &Generic, &l);
            }
        }
        assert!(opened > 0, "no flip left the file readable");
    }
}
