//! Writes the test scenes (`testgen::scenes::ALL`) into a folder, each as `DIR/<name>.blend`,
//! for another program to open and save again. Test tooling only.
//!
//!     cargo run --example blend_scenes --features testgen -- DIR

use std::path::PathBuf;

use k2_mpc_blend::testgen::scenes;

fn main() {
    let mut args = std::env::args_os().skip(1);
    let (Some(dir), None) = (args.next(), args.next()) else {
        eprintln!("usage: blend_scenes DIR");
        std::process::exit(2);
    };
    let dir = PathBuf::from(dir);
    if let Err(e) = std::fs::create_dir_all(&dir) {
        eprintln!("blend_scenes: the folder can't be made: {e}");
        std::process::exit(1);
    }
    for (name, write) in scenes::ALL {
        let bytes = write();
        if let Err(e) = std::fs::write(dir.join(name), &bytes) {
            eprintln!("blend_scenes: {name}: {e}");
            std::process::exit(1);
        }
        println!("{name}\t{} bytes", bytes.len());
    }
}
