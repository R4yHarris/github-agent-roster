use crate::Error;

pub mod files;
pub mod layout;
pub mod locks;
pub mod migrate;
pub mod provenance;
pub mod redaction;
pub mod roots;
pub mod secure;

#[cfg(test)]
mod ownership_tests;

#[derive(Debug, Clone, PartialEq)]
pub struct RetentionPolicy {
    pub scope: String,
    pub default_window_ms: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RetentionDecision {
    pub keep: bool,
    pub reason: &'static str,
    pub expires_at_ms: Option<f64>,
}

impl RetentionDecision {
    pub fn json(&self) -> String {
        let expires = self
            .expires_at_ms
            .map(|value| {
                let value = if value.is_finite() {
                    value.to_string()
                } else {
                    "null".to_owned()
                };
                format!(",\"expiresAtMs\":{value}")
            })
            .unwrap_or_default();
        format!(
            "{{\"keep\":{},\"reason\":\"{}\"{expires}}}",
            self.keep, self.reason
        )
    }
}

fn invalid(code: &'static str, message: &str) -> Error {
    Error::new("RepoStateError", message).coded(code)
}

pub fn evaluate_retention(
    policy: Option<&RetentionPolicy>,
    now_ms: f64,
    window_ms: Option<f64>,
    opt_out: bool,
    created_at_ms: Option<f64>,
) -> Result<RetentionDecision, Error> {
    let policy = policy.ok_or_else(|| {
        invalid(
            "E_RETENTION_POLICY",
            "evaluateRetention requires a retention policy with a scope.",
        )
    })?;
    if !now_ms.is_finite() || now_ms < 0.0 {
        return Err(invalid(
            "E_RETENTION_CLOCK",
            "evaluateRetention requires a non-negative finite nowMs (injected clock).",
        ));
    }
    let window = window_ms.unwrap_or(policy.default_window_ms);
    if !window.is_finite() || window <= 0.0 {
        return Err(invalid(
            "E_RETENTION_WINDOW",
            "evaluateRetention requires a positive finite windowMs.",
        ));
    }
    if opt_out {
        return Ok(RetentionDecision {
            keep: true,
            reason: "opted-out",
            expires_at_ms: None,
        });
    }
    let created = created_at_ms
        .filter(|value| value.is_finite() && *value >= 0.0)
        .ok_or_else(|| {
            invalid(
                "E_RETENTION_CREATED",
                "evaluateRetention requires a non-negative finite createdAtMs.",
            )
        })?;
    let expires = created + window;
    let expired = now_ms > expires;
    Ok(RetentionDecision {
        keep: !expired,
        reason: if expired { "expired" } else { "within-window" },
        expires_at_ms: Some(expires),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_expiry_boundary_is_retained_and_unknown_age_cannot_delete() -> Result<(), Error> {
        let policy = RetentionPolicy {
            scope: "machine".to_owned(),
            default_window_ms: 100.0,
        };
        assert!(evaluate_retention(Some(&policy), 200.0, None, false, Some(100.0))?.keep);
        assert!(!evaluate_retention(Some(&policy), 201.0, None, false, Some(100.0))?.keep);
        assert!(
            evaluate_retention(Some(&policy), 1000.0, None, false, None)
                .is_err_and(|error| error.code == Some("E_RETENTION_CREATED"))
        );
        assert_eq!(
            evaluate_retention(Some(&policy), 1000.0, None, true, None)?.reason,
            "opted-out"
        );
        Ok(())
    }

    #[test]
    fn invalid_policy_clock_or_window_fails_before_opt_out() {
        let policy = RetentionPolicy {
            scope: "machine".to_owned(),
            default_window_ms: 100.0,
        };
        assert!(
            evaluate_retention(None, 0.0, None, true, None)
                .is_err_and(|error| error.code == Some("E_RETENTION_POLICY"))
        );
        assert!(
            evaluate_retention(Some(&policy), f64::NAN, None, true, None)
                .is_err_and(|error| error.code == Some("E_RETENTION_CLOCK"))
        );
        assert!(
            evaluate_retention(Some(&policy), 0.0, Some(0.0), true, None)
                .is_err_and(|error| error.code == Some("E_RETENTION_WINDOW"))
        );
    }
}
