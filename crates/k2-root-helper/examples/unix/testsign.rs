use std::io::Write;

use k2_root_proto::keys::TrustedKey;
use k2_root_proto::sig::{encode_payload, trusted_comment, verify_signed_build};
use k2_root_proto::testsign::sign_with_test_key;

pub fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let r = |i: usize| std::fs::read(&a[i]).expect("read file");
    let out = match a.first().map(String::as_str) {
        Some("sig") => sign_with_test_key(
            &r(1),
            &trusted_comment(&a[4], &a[2], a[3].parse().expect("protocol")),
        )
        .into_bytes(),
        Some("payload") => {
            let elf = r(1);
            let sig = sign_with_test_key(
                &elf,
                &trusted_comment(&a[4], &a[2], a[3].parse().expect("protocol")),
            );
            encode_payload(&sig, &elf)
        }
        // k2-app-engine (apps install-engine stdin / the daemon's .sig).
        Some("engine-sig") => sign_with_test_key(
            &r(1),
            &k2_root_proto::engine::trusted_comment(&a[4], &a[2], a[3].parse().expect("protocol")),
        )
        .into_bytes(),
        Some("engine-payload") => {
            let elf = r(1);
            let sig = sign_with_test_key(
                &elf,
                &k2_root_proto::engine::trusted_comment(&a[4], &a[2], a[3].parse().expect("protocol")),
            );
            encode_payload(&sig, &elf)
        }
        Some("payload-raw") => {
            let sig = String::from_utf8(r(2)).expect("sig text");
            encode_payload(&sig, &r(1))
        }
        Some("verify") => {
            let key: &'static str = Box::leak(a[1].clone().into_boxed_str());
            let k = TrustedKey {
                role: "test",
                key_id: "-",
                public_key_b64: key,
            };
            let sig = String::from_utf8(r(3)).expect("sig text");
            let arch: &'static str = Box::leak(a[4].clone().into_boxed_str());
            match verify_signed_build(&r(2), &sig, &[k], arch) {
                Ok(v) => format!("ok version={} protocol={}\n", v.version, v.protocol).into_bytes(),
                Err(e) => {
                    eprintln!("refused: {e}");
                    std::process::exit(4);
                }
            }
        }
        _ => {
            eprintln!("usage: see the file header");
            std::process::exit(2);
        }
    };
    std::io::stdout().write_all(&out).expect("write");
}
