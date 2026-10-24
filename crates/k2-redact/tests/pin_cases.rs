//! Pinned redactor output (SE24b); the cases live in `cases/mod.rs`.

mod cases;
use cases::CASES;

#[test]
fn pinned_outputs() {
    for (name, value, input, want) in CASES {
        let r = k2_redact::Redactor::new([(*name, value.as_bytes())]);
        assert_eq!(r.redact_str(input), *want, "case {name} / {input}");
        let mut s = r.stream();
        let mut out = Vec::new();
        for b in input.as_bytes() {
            out.extend(s.push(std::slice::from_ref(b)));
        }
        out.extend(s.finish());
        assert_eq!(String::from_utf8(out).expect("utf8"), *want, "stream case {name} / {input}");
    }
}

#[test]
fn wipe_zeroes() {
    let mut v = b"k2test-secret-wipe".to_vec();
    k2_redact::wipe(&mut v);
    assert!(v.iter().all(|&b| b == 0));
}
