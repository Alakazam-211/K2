//! The top-bar usage chip right after a server default switch.
//!
//! On the old usage path (`usageSingleRecord` off) the chip reads the
//! shared cache (`subscription_usage`), whose row for a tool still holds
//! the outgoing token's numbers until the re-check after the switch lands.
//! The switch route calls [`follow_live_owner`] so the chip shows the new
//! default at once. The single-record path needs none of this: its GET
//! reads the live token every time.

use crate::subscription_usage as su;

/// Point `harness`'s cache row at the token now in the live store: its own
/// last reading (with its own time), or no numbers (and no plan) if it was
/// never read. Returns whether it wrote. The caller announces the change.
pub(crate) fn follow_live_owner(harness: &str) -> bool {
    let Some((owner, _)) = su::wallet_generation(harness) else { return false };
    let mine = su::wallet_row(&owner);
    su::update_cache_row(harness, |prev| {
        if prev.is_some_and(|p| p.account_id.as_deref() == Some(owner.as_str())) {
            return None;
        }
        // Not another token's numbers, time or plan: the re-check fills them.
        let mut row = mine.unwrap_or_default();
        row.harness = harness.to_string();
        row.account_id = Some(owner.clone());
        Some(row)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::subscription_usage::tests::{future_reset, harness, seed_wallet, with_isolated, write_cache_for_tests};
    use crate::subscription_usage::{HarnessUsage, SubscriptionFile, UsageWindow};
    use chrono::Utc;
    use serde_json::{json, Value};

    const CLAUDE: &str = "claude";

    /// Set a token's own last reading in the wallet (what Settings shows).
    fn set_wallet_reading(id: &str, row: &HarnessUsage) {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "UPDATE llm_accounts SET usage_json = ?2 WHERE id = ?1",
            rusqlite::params![id, serde_json::to_string(row).expect("row json")],
        )
        .expect("set reading");
    }

    fn cached_claude() -> HarnessUsage {
        su::cached_rows().into_iter().find(|h| h.harness == CLAUDE).expect("claude row")
    }

    /// The cache as it is right after a switch to acc_new: acc_old's numbers.
    fn seed_old_row() {
        let now = Utc::now();
        let mut old = harness(
            CLAUDE,
            vec![UsageWindow { label: "old-weekly".into(), used: 0.95, resets_at: future_reset(now), ..Default::default() }],
            "",
            &now.to_rfc3339(),
        );
        old.account_id = Some("acc_old".into());
        write_cache_for_tests(&SubscriptionFile { harnesses: vec![old] });
    }

    #[test]
    fn after_a_switch_the_cache_row_shows_the_new_defaults_own_reading() {
        with_isolated(|| {
            // The switch already ran: acc_new is the default and live.
            seed_wallet("acc_new", "acc_old");
            seed_old_row();
            let now = Utc::now();
            let mine = harness(
                CLAUDE,
                vec![UsageWindow { label: "new-weekly".into(), used: 0.12, resets_at: future_reset(now), ..Default::default() }],
                "",
                &(now - chrono::Duration::minutes(3)).to_rfc3339(),
            );
            set_wallet_reading("acc_new", &mine);

            assert!(follow_live_owner(CLAUDE), "the row belonged to the old token: rewritten");
            let row = cached_claude();
            assert_eq!(row.account_id.as_deref(), Some("acc_new"));
            assert_eq!(row.windows, mine.windows, "the new default's own numbers");
            assert_eq!(row.checked_at, mine.checked_at, "with their own time, not re-stamped");
            // What the chip's GET serves.
            let body: Value = serde_json::from_str(&su::handle_get().body).expect("json");
            let served = body["harnesses"]
                .as_array()
                .expect("rows")
                .iter()
                .find(|r| r["harness"] == CLAUDE)
                .expect("claude")
                .clone();
            assert_eq!(served["windows"][0]["label"], json!("new-weekly"), "{body}");

            // Already the live token's row: left alone.
            assert!(!follow_live_owner(CLAUDE));
        });
    }

    #[test]
    fn after_a_switch_to_a_token_never_read_the_old_numbers_are_not_shown() {
        with_isolated(|| {
            seed_wallet("acc_new", "acc_old");
            seed_old_row();

            assert!(follow_live_owner(CLAUDE));
            let row = cached_claude();
            assert_eq!(row.account_id.as_deref(), Some("acc_new"));
            assert!(row.windows.is_empty(), "another token's windows were shown: {row:?}");
            assert_eq!(row.checked_at, "", "never read for this token");
            assert_eq!(row.plan, "", "nor the old token's plan");
        });
    }
}
