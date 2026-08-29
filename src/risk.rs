//! Risk prediction grading module.
//!
//! Introduces 3-tier risk levels (High/Medium/Low) for predictions,
//! upgrading the snapshot/validate cycle into a graded risk system.
//!
//! Reference: yologdev/yoyo-evolve -- Day 112 `/risk validate`
//!   `/risk snapshot` saves what I think will break;
//!   `/risk validate` comes back later and asks git what *actually* broke
//!   -- which files appeared in reverts, which needed fixes -- and compares.
//!   Precision at ten. Hits and misses, named.
//!
//! q-body approach: Layer `RiskLevel` enum + `RiskEntry` struct on top of
//! the existing PredictionEntry concept. Type-level prep only; handler.rs
//! runtime wiring deferred per established precedent.

use serde::{Deserialize, Serialize};

/// Three-tier risk level for predictions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RiskLevel {
    /// High risk -- likely to break or fail
    High,
    /// Medium risk -- uncertain outcome
    Medium,
    /// Low risk -- confident in success
    Low,
}

impl RiskLevel {
    /// Returns a numeric severity score (High=3, Medium=2, Low=1).
    pub fn severity(&self) -> u8 {
        match self {
            RiskLevel::High => 3,
            RiskLevel::Medium => 2,
            RiskLevel::Low => 1,
        }
    }
}

/// A risk-graded prediction entry.
/// Wraps a prediction string with a risk level and timestamp.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RiskEntry {
    /// The prediction statement
    pub prediction: String,
    /// Assigned risk level
    pub risk: RiskLevel,
    /// When the prediction was made
    pub predicted_at: chrono::DateTime<chrono::Utc>,
    /// Optional: when the prediction was validated
    pub validated_at: Option<chrono::DateTime<chrono::Utc>>,
    /// Optional: what actually happened
    pub actual: Option<String>,
}

impl RiskEntry {
    /// Create a new risk entry with current timestamp.
    pub fn new(prediction: String, risk: RiskLevel) -> Self {
        Self {
            prediction,
            risk,
            predicted_at: chrono::Utc::now(),
            validated_at: None,
            actual: None,
        }
    }

    /// Record validation result.
    pub fn validate(&mut self, actual: String) {
        self.validated_at = Some(chrono::Utc::now());
        self.actual = Some(actual);
    }

    /// Returns true if this entry has been validated.
    pub fn is_validated(&self) -> bool {
        self.validated_at.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_risk_level_severity() {
        assert_eq!(RiskLevel::High.severity(), 3);
        assert_eq!(RiskLevel::Medium.severity(), 2);
        assert_eq!(RiskLevel::Low.severity(), 1);
    }

    #[test]
    fn test_risk_entry_new() {
        let entry = RiskEntry::new("LLM timeout will occur".into(), RiskLevel::High);
        assert_eq!(entry.prediction, "LLM timeout will occur");
        assert_eq!(entry.risk, RiskLevel::High);
        assert!(!entry.is_validated());
    }

    #[test]
    fn test_risk_entry_validate() {
        let mut entry = RiskEntry::new("port 41242 will be busy".into(), RiskLevel::Medium);
        assert!(!entry.is_validated());
        entry.validate("port was free, no conflict".into());
        assert!(entry.is_validated());
        assert_eq!(entry.actual.unwrap(), "port was free, no conflict");
    }

    #[test]
    fn test_risk_entry_serialize_roundtrip() {
        let entry = RiskEntry::new("disk will fill up".into(), RiskLevel::Low);
        let json = serde_json::to_string(&entry).unwrap();
        let deserialized: RiskEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.prediction, "disk will fill up");
        assert_eq!(deserialized.risk, RiskLevel::Low);
    }
}