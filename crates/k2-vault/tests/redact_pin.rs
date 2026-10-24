//! The vault's redactor is the shared `k2-redact` one (SE24b): the pinned
//! cases give the same output through `k2_vault::redact`.

#[path = "../../k2-redact/tests/cases/mod.rs"]
mod cases;

#[test]
fn vault_redactor_matches_the_pinned_cases() {
    for (name, value, input, want) in cases::CASES {
        let r = k2_vault::redact::Redactor::new([(*name, value.as_bytes())]);
        assert_eq!(r.redact_str(input), *want, "case {name} / {input}");
    }
    assert_eq!(k2_vault::redact::SURFACE_MIN_LEN, 8);
    // The vault's hex and the redactor's hex agree (variants depend on it).
    assert_eq!(k2_vault::crypto::hex(b"\x00\xffk2"), k2_redact::hex(b"\x00\xffk2"));
}
