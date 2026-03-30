//! Pure scoring helpers — no I/O, no DB access.
//! Used by both `hooks::prompt` and `db::facts`.

use crate::config::CategoryWeights;
use crate::db::facts::FactCategory;
use chrono::{DateTime, Utc};

/// Category weight from a config-driven weight table.
pub fn category_weight(cat: &FactCategory, weights: &CategoryWeights) -> f64 {
    match cat {
        FactCategory::Convention => weights.convention,
        FactCategory::Decision   => weights.decision,
        FactCategory::Fix        => weights.fix,
        FactCategory::User       => weights.user,
        FactCategory::Context    => weights.context,
        FactCategory::File       => weights.file,
    }
}

/// Recency multiplier: 1.0 today, decays linearly to 0.5 at `decay_days`, clamped there.
pub fn recency_factor(created_at: DateTime<Utc>, now: DateTime<Utc>, decay_days: f64) -> f64 {
    let days = (now - created_at).num_days().max(0) as f64;
    (1.0_f64 - (days / decay_days) * 0.5).max(0.5)
}

/// Access-count multiplier: logarithmic growth from 1.0, capped at 1.5.
pub fn access_boost(access_count: i64) -> f64 {
    (1.0 + (access_count as f64).max(0.0).ln_1p() / 3.0).min(1.5)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ScroogeConfig;

    fn w() -> CategoryWeights { ScroogeConfig::default().category_weights }

    #[test]
    fn convention_outweighs_file() {
        assert!(category_weight(&FactCategory::Convention, &w()) > category_weight(&FactCategory::File, &w()));
    }

    #[test]
    fn recency_is_one_today() {
        let now = Utc::now();
        assert!((recency_factor(now, now, 90.0) - 1.0).abs() < 0.01);
    }

    #[test]
    fn recency_is_half_at_decay_days() {
        let now = Utc::now();
        let old = now - chrono::Duration::days(90);
        assert!((recency_factor(old, now, 90.0) - 0.5).abs() < 0.01);
    }

    #[test]
    fn recency_clamped_at_half() {
        let now = Utc::now();
        let very_old = now - chrono::Duration::days(365);
        assert!(recency_factor(very_old, now, 90.0) >= 0.5);
    }

    #[test]
    fn access_boost_baseline() {
        assert!((access_boost(0) - 1.0).abs() < 0.01);
    }

    #[test]
    fn access_boost_cap() {
        assert!(access_boost(100_000) <= 1.5);
        assert!((access_boost(100_000) - 1.5).abs() < 0.01);
    }
}
