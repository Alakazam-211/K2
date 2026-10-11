//! TEST ONLY (`--features test-keys`): the controller + owner stand-in for
//! the Linux end-to-end (tests/cli/node_admin_linux.sh). Never shipped.
//!
//!   k2-node-testsign keygen <controller-key.pem>        → prints "fp spki_b64"
//!   k2-node-testsign owner-pub <seed-hex>               → prints ed25519:<b64>
//!   k2-node-testsign build-sig <file> <os> <arch> <version> <protocol>  → minisign text (TEST key)
//!   k2-node-testsign cmd --key PEM --node-fp FP --verb V [--args JSON] [--owner-seed HEX]
//!                        [--iat-offset S] [--ttl S] [--nonce N] [--request-id R]  → SignedCommand JSON
//!   k2-node-testsign send <socket> command <cmd.json>   → executor answer (JSON)
//!   k2-node-testsign send <socket> update <cmd.json> <build> <build.sig>
//!   k2-node-testsign send <socket> status
//!   k2-node-testsign send-raw <socket> <file>           → send a raw header frame (garbage tests)

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use k2_node::admin::exec::{LocalRequest, LocalResponse};
use k2_node::admin::server;
use k2_node_proto::admin::{self, ApprovalSigner, Class, Command, SignedCommand};
use k2_node_proto::crypto::{self, SigningKey};

fn seed(hex: &str) -> [u8; 32] {
    let mut out = [0u8; 32];
    for (i, b) in out.iter_mut().enumerate() {
        *b = u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).expect("seed hex");
    }
    out
}

fn flags(a: &[String]) -> BTreeMap<String, String> {
    let mut m = BTreeMap::new();
    let mut i = 0;
    while i + 1 < a.len() {
        m.insert(a[i].trim_start_matches("--").to_string(), a[i + 1].clone());
        i += 2;
    }
    m
}

fn print_resp(r: Result<LocalResponse, admin::AdminRefusal>) {
    match r {
        Ok(r) => println!("{}", serde_json::to_string(&r).unwrap()),
        Err(e) => {
            println!("{}", serde_json::json!({"kind": "client_error", "code": e.code, "message": e.message}));
            std::process::exit(5);
        }
    }
}

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    match a.first().map(String::as_str) {
        Some("keygen") => {
            let k = SigningKey::generate().unwrap();
            std::fs::write(&a[1], k.to_pkcs8_pem().unwrap()).unwrap();
            println!("{} {}", k.fingerprint(), crypto::b64(k.spki_der()));
        }
        Some("owner-pub") => println!("{}", ApprovalSigner::from_seed(&seed(&a[1])).unwrap().public().encode()),
        Some("build-sig") => {
            let bytes = std::fs::read(&a[1]).unwrap();
            let tc = k2_root_proto::node_build::trusted_comment(&a[2], &a[3], &a[4], a[5].parse().unwrap());
            print!("{}", k2_root_proto::testsign::sign_with_test_key(&bytes, &tc));
        }
        Some("cmd") => {
            let f = flags(&a[1..]);
            let key = SigningKey::from_pkcs8_pem(&std::fs::read_to_string(&f["key"]).unwrap()).unwrap();
            let now = k2_node::util::now() + f.get("iat-offset").map(|s| s.parse::<i64>().unwrap()).unwrap_or(0);
            let owner = f.get("owner-seed").map(|s| ApprovalSigner::from_seed(&seed(s)).unwrap());
            let cmd = Command {
                v: admin::ADMIN_V,
                cmd_id: format!("e2e-{}", &crypto::random_id().unwrap()[..12]),
                node_fp: f["node-fp"].clone(),
                controller_fp: key.fingerprint(),
                class: if owner.is_some() { Class::Privileged } else { Class::Operate },
                verb: f["verb"].clone(),
                args: serde_json::from_str(f.get("args").map(String::as_str).unwrap_or("{}")).unwrap(),
                plan: None,
                proposed_by: "agent:e2e".into(),
                approved_by: owner.as_ref().map(|_| "owner:e2e".to_string()),
                approved_via: owner.as_ref().map(|_| "local-auth".to_string()),
                request_id: owner.as_ref().map(|_| f.get("request-id").cloned().unwrap_or_else(|| "car_e2e".into())),
                iat: now,
                exp: now + f.get("ttl").map(|s| s.parse::<i64>().unwrap()).unwrap_or(600),
                nonce: f.get("nonce").cloned().unwrap_or_else(|| admin::new_nonce().unwrap()),
            };
            let mut s = admin::sign_controller(&key, &cmd).unwrap();
            if let Some(o) = owner {
                let body = crypto::unb64(&s.body_b64).unwrap();
                s = s.with_owner_sig(&o.sign(&admin::owner_message(&body)));
            }
            println!("{}", s.to_json().unwrap());
        }
        Some("send") => {
            let sock = Path::new(&a[1]);
            let t = Duration::from_secs(180);
            let read_cmd = |p: &str| -> SignedCommand { serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap() };
            match a[2].as_str() {
                "status" => print_resp(server::request(sock, &LocalRequest::Status, None, t)),
                "command" => print_resp(server::request(sock, &LocalRequest::Command { cmd: read_cmd(&a[3]) }, None, t)),
                "update" => {
                    let bytes = std::fs::read(&a[4]).unwrap();
                    let sig = std::fs::read_to_string(&a[5]).unwrap();
                    let req = LocalRequest::Update { cmd: read_cmd(&a[3]), sig, size: bytes.len() as u64 };
                    print_resp(server::request(sock, &req, Some(&bytes), t));
                }
                o => panic!("send {o}?"),
            }
        }
        Some("send-raw") => {
            use std::io::Write;
            let mut s = std::os::unix::net::UnixStream::connect(&a[1]).unwrap();
            let body = std::fs::read(&a[2]).unwrap();
            server::write_frame(&mut s, &body).unwrap();
            s.flush().unwrap();
            let r = server::read_frame(&mut s, 1 << 22).unwrap();
            println!("{}", String::from_utf8_lossy(&r));
        }
        _ => {
            eprintln!("see the module doc");
            std::process::exit(2);
        }
    }
}
