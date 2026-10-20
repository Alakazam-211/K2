//! Examples for the wire types S24 adds (prd-sew-24 §7.2).

use crate::mgmt::*;
use crate::registry::Example;

impl Example for AuditVerifyRequest {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            ("full", AuditVerifyRequest { full: true }),
            ("min", AuditVerifyRequest { full: false }),
        ]
    }
}

impl Example for AuditVerifyReport {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                AuditVerifyReport {
                    mode: AuditVerifyMode::Full,
                    checked: 811,
                    head_seq: 811,
                    head_hash: "3f0c1a2b".repeat(8),
                    broken_at: Some(812),
                    reason: Some("hash mismatch at chain_seq 812".into()),
                },
            ),
            (
                "min",
                AuditVerifyReport {
                    mode: AuditVerifyMode::Incremental,
                    checked: 1_204,
                    head_seq: 1_204,
                    head_hash: "3f0c1a2b".repeat(8),
                    broken_at: None,
                    reason: None,
                },
            ),
        ]
    }
}

impl Example for ExportRequest {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![(
            "full",
            ExportRequest {
                recipients: vec![
                    "age1ql3z7hjy54pw3hyww5ayyfg7zqgvc7w3j2elw8zmrj2kg5sfn9aqmcac8p".into(),
                ],
            },
        )]
    }
}
