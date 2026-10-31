//! A small table pack for the kit's grid tests (A9-A12): `grid-lite`, one record per line,
//! fields split by `,`, no quoting, the first record the header. It stands in for the CSV pack
//! (C1h), and lists the kit's grid verbs ([`k2_mpc_base::table::verbs::VERBS`], C1i), with
//! its own block decoder for their stdin (`cells set`, `row insert`): one record per line,
//! fields split by `,`.
#![allow(dead_code)]

use k2_mpc_base::doc::{Doc, Neutral};
use k2_mpc_base::schema::Schema;
use k2_mpc_base::session::{Declined, Pack, PackContent, PackError, View};
use k2_mpc_base::table::{self, Field, Record, Shape, Table};
use k2_mpc_base::tools::PackVerb;

pub struct GridLite;

pub const LITE: Schema = Schema {
    name: "grid-lite",
    roots: table::ROOTS,
};

fn records(s: &str) -> Vec<Record> {
    s.lines()
        .map(|l| {
            if l.is_empty() {
                Vec::new()
            } else {
                l.split(',').map(Field::new).collect()
            }
        })
        .collect()
}

impl Pack for GridLite {
    fn schema(&self) -> Schema {
        LITE
    }
    fn decode(&self, b: &[u8]) -> Result<PackContent, Declined> {
        let s = std::str::from_utf8(b).map_err(|_| Declined {
            code: "not_utf8",
            message: "not UTF-8".into(),
        })?;
        let recs = records(s);
        let (_, edits) =
            Table::import(&recs, true, Shape::detect(&recs)).map_err(|e| Declined {
                code: "malformed",
                message: e.to_string(),
            })?;
        let mut d = Doc::new(LITE).unwrap();
        d.edit_as(2, &edits).unwrap();
        Ok(PackContent {
            roots: d.export_neutral().unwrap(),
            codec: vec![1],
        })
    }
    fn encode(&self, c: &[(String, Neutral)], _: &[u8]) -> Result<Vec<u8>, PackError> {
        let t = View::of_content(c)
            .table()
            .map_err(|e| PackError(e.to_string()))?;
        Ok(render(&t).into_bytes())
    }
    fn decode_block(&self, b: &[u8], _: &[u8]) -> Result<Vec<Record>, Declined> {
        let s = std::str::from_utf8(b).map_err(|_| Declined {
            code: "not_utf8",
            message: "not UTF-8".into(),
        })?;
        Ok(records(s))
    }
    fn verbs(&self) -> &'static [PackVerb] {
        table::verbs::VERBS
    }
}

/// The file a table renders to.
pub fn render(t: &Table) -> String {
    let mut out = String::new();
    for rec in t.render() {
        let f: Vec<&str> = rec.iter().map(|f| f.value.as_str()).collect();
        out.push_str(&f.join(","));
        out.push('\n');
    }
    out
}
