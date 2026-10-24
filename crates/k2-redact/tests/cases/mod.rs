//! Pinned redactor cases (SE24b). The same cases run against
//! `k2_vault::redact` (crates/k2-vault/tests/redact_pin.rs) and the node's
//! redactor, so the vault and k2-node never drift apart. Fake values only.

/// (name, value, input, expected output).
pub const CASES: &[(&str, &str, &str, &str)] = &[
    ("STRIPE_KEY", "k2test-secret-4eC39HqLyjWDarjt", "key=k2test-secret-4eC39HqLyjWDarjt&x=1", "key=[k2v:STRIPE_KEY]&x=1"),
    // base64 (std, no padding) of the value at offset 0.
    ("STRIPE_KEY", "k2test-secret-4eC39HqLyjWDarjt", "b64 azJ0ZXN0LXNlY3JldC00ZUMzOUhxTHlqV0Rhcmp0 end", "b64 [k2v:STRIPE_KEY] end"),
    // lower and upper hex.
    ("H", "k2test-secret-hexhex", "6b32746573742d7365637265742d686578686578", "[k2v:H]"),
    ("H", "k2test-secret-hexhex", "x 6B32746573742D7365637265742D686578686578 y", "x [k2v:H] y"),
    // percent and JSON escapes.
    ("P", "k2test/secret value+1", "q=k2test%2Fsecret%20value%2B1", "q=[k2v:P]"),
    ("P", "k2test/secret value+1", r#"{"v":"k2test\/secret value+1"}"#, r#"{"v":"[k2v:P]"}"#),
    // Under the surface floor is still matched by the redactor itself; the
    // floor is the caller's rule (SURFACE_MIN_LEN).
    ("S", "k2t-abc", "a k2t-abc b", "a [k2v:S] b"),
    // Nothing to match.
    ("N", "k2test-secret-none", "nothing here", "nothing here"),
];

