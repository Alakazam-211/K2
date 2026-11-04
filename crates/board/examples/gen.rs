//! Writes everything generated in this repo: `schema/board-schema.json`, the tables in
//! `docs/FORMAT.md`, and `vectors/`. Run it from the repo root; the tests check the tree matches.

#[path = "../tests/support/mod.rs"]
mod support;
use support::cases;

use std::fs;
use std::path::Path;

fn main() -> std::io::Result<()> {
    fs::create_dir_all("schema")?;
    fs::write("schema/board-schema.json", k2_mpc_board::schema_json())?;

    let doc = fs::read_to_string("docs/FORMAT.md")?;
    let (head, rest) = doc.split_once(BEGIN).expect("the begin marker");
    let (_, tail) = rest.split_once(END).expect("the end marker");
    let tables = k2_mpc_board::schema::format_tables();
    fs::write(
        "docs/FORMAT.md",
        format!("{head}{BEGIN}\n\n{tables}\n{END}{tail}"),
    )?;

    let root = Path::new("vectors");
    for (path, bytes) in cases::files() {
        let full = root.join(path);
        if let Some(dir) = full.parent() {
            fs::create_dir_all(dir)?;
        }
        fs::write(full, bytes)?;
    }
    println!("generated");
    Ok(())
}

const BEGIN: &str = "<!-- generated:begin -->";
const END: &str = "<!-- generated:end -->";
