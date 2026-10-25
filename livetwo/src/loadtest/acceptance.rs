use serde::Serialize;

use super::LoadtestStats;
use super::whep::VerifyStats;

pub const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Default, Clone, Copy)]
pub struct AcceptanceCriteria {
    pub min_connected: u64,
    pub min_packets: u64,
    pub min_bytes: u64,
    pub fail_on_any_session: bool,
    pub verification_requested: bool,
    pub verification_tolerant: bool,
}

#[derive(Debug, Serialize)]
pub struct RunReport {
    pub schema_version: u32,
    pub stats: LoadtestStats,
    pub verification: Option<VerifyStats>,
    pub failures: Vec<String>,
    pub warnings: Vec<String>,
    pub success: bool,
}

impl RunReport {
    pub fn operational_failure(message: String) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            stats: LoadtestStats::default(),
            verification: None,
            failures: vec![message],
            warnings: Vec::new(),
            success: false,
        }
    }

    pub fn new(
        stats: LoadtestStats,
        verification: Option<VerifyStats>,
        criteria: AcceptanceCriteria,
    ) -> Self {
        let mut failures = Vec::new();
        let mut warnings = Vec::new();

        if stats.sessions_connected == 0
            && stats.sessions_total > 0
            && stats.sessions_cancelled < stats.sessions_total as u64
        {
            failures.push(format!(
                "all {} loadtest session(s) failed",
                stats.sessions_total
            ));
        }
        if stats.sessions_connected < criteria.min_connected {
            failures.push(format!(
                "connected sessions {} below required minimum {}",
                stats.sessions_connected, criteria.min_connected
            ));
        }
        if stats.total_packets < criteria.min_packets {
            failures.push(format!(
                "packets {} below required minimum {}",
                stats.total_packets, criteria.min_packets
            ));
        }
        if stats.total_bytes < criteria.min_bytes {
            failures.push(format!(
                "bytes {} below required minimum {}",
                stats.total_bytes, criteria.min_bytes
            ));
        }

        if criteria.fail_on_any_session {
            if stats.sessions_failed > 0 {
                failures.push(format!("{} session(s) failed", stats.sessions_failed));
            }
            if stats.sessions_aborted > 0 {
                failures.push(format!("{} session(s) aborted", stats.sessions_aborted));
            }
        } else if stats.sessions_connected > 0 {
            if stats.sessions_failed > 0 {
                warnings.push(format!("{} session(s) failed", stats.sessions_failed));
            }
            if stats.sessions_aborted > 0 {
                warnings.push(format!("{} session(s) aborted", stats.sessions_aborted));
            }
        }

        if stats.sessions_connected == 0
            && stats.sessions_total > 0
            && stats.sessions_cancelled == stats.sessions_total as u64
        {
            warnings
                .push("all sessions were cancelled before connecting; no traffic flowed".into());
        }

        if criteria.verification_requested {
            let verification_failure = match &verification {
                Some(verify) if verify.windows_total == 0 => {
                    Some("verification enabled but no full decode window completed".to_string())
                }
                Some(verify) if verify.windows_failed > 0 => Some(format!(
                    "{} verification window(s) failed",
                    verify.windows_failed
                )),
                Some(_) => None,
                None => Some("decode verification did not produce results".to_string()),
            };
            if let Some(message) = verification_failure {
                if criteria.verification_tolerant {
                    warnings.push(message);
                } else {
                    failures.push(message);
                }
            }
        }

        let success = failures.is_empty();
        Self {
            schema_version: SCHEMA_VERSION,
            stats,
            verification,
            failures,
            warnings,
            success,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Case {
        name: &'static str,
        stats: LoadtestStats,
        verification: Option<VerifyStats>,
        criteria: AcceptanceCriteria,
        success: bool,
        failures: &'static [&'static str],
        warnings: &'static [&'static str],
    }

    #[test]
    fn acceptance_table() {
        let healthy = LoadtestStats {
            sessions_total: 2,
            sessions_connected: 2,
            total_packets: 100,
            total_bytes: 10_000,
            ..Default::default()
        };
        let partial = LoadtestStats {
            sessions_failed: 1,
            sessions_connected: 1,
            ..healthy.clone()
        };
        let all_failed = LoadtestStats {
            sessions_connected: 0,
            sessions_failed: 2,
            ..healthy.clone()
        };
        let all_cancelled = LoadtestStats {
            sessions_connected: 0,
            sessions_cancelled: 2,
            ..healthy.clone()
        };
        let verified = VerifyStats {
            windows_total: 1,
            windows_ok: 1,
            ..Default::default()
        };
        let verification_failed = VerifyStats {
            windows_total: 1,
            windows_failed: 1,
            ..Default::default()
        };
        let verification_required = AcceptanceCriteria {
            verification_requested: true,
            ..Default::default()
        };
        let cases = [
            Case {
                name: "healthy defaults",
                stats: healthy.clone(),
                verification: None,
                criteria: AcceptanceCriteria::default(),
                success: true,
                failures: &[],
                warnings: &[],
            },
            Case {
                name: "partial failure remains successful by default",
                stats: partial.clone(),
                verification: None,
                criteria: AcceptanceCriteria::default(),
                success: true,
                failures: &[],
                warnings: &["1 session(s) failed"],
            },
            Case {
                name: "any session failure can be strict",
                stats: partial,
                verification: None,
                criteria: AcceptanceCriteria {
                    fail_on_any_session: true,
                    ..Default::default()
                },
                success: false,
                failures: &["1 session(s) failed"],
                warnings: &[],
            },
            Case {
                name: "all failed always fails",
                stats: all_failed,
                verification: None,
                criteria: AcceptanceCriteria::default(),
                success: false,
                failures: &["all 2 loadtest session(s) failed"],
                warnings: &[],
            },
            Case {
                name: "all cancelled keeps legacy success",
                stats: all_cancelled.clone(),
                verification: None,
                criteria: AcceptanceCriteria::default(),
                success: true,
                failures: &[],
                warnings: &["all sessions were cancelled before connecting; no traffic flowed"],
            },
            Case {
                name: "connected threshold",
                stats: healthy.clone(),
                verification: None,
                criteria: AcceptanceCriteria {
                    min_connected: 3,
                    ..Default::default()
                },
                success: false,
                failures: &["connected sessions 2 below required minimum 3"],
                warnings: &[],
            },
            Case {
                name: "packet threshold",
                stats: healthy.clone(),
                verification: None,
                criteria: AcceptanceCriteria {
                    min_packets: 101,
                    ..Default::default()
                },
                success: false,
                failures: &["packets 100 below required minimum 101"],
                warnings: &[],
            },
            Case {
                name: "byte threshold",
                stats: healthy.clone(),
                verification: None,
                criteria: AcceptanceCriteria {
                    min_bytes: 10_001,
                    ..Default::default()
                },
                success: false,
                failures: &["bytes 10000 below required minimum 10001"],
                warnings: &[],
            },
            Case {
                name: "strict verification failure",
                stats: healthy.clone(),
                verification: Some(verification_failed.clone()),
                criteria: verification_required,
                success: false,
                failures: &["1 verification window(s) failed"],
                warnings: &[],
            },
            Case {
                name: "tolerant verification failure",
                stats: healthy.clone(),
                verification: Some(verification_failed),
                criteria: AcceptanceCriteria {
                    verification_tolerant: true,
                    ..verification_required
                },
                success: true,
                failures: &[],
                warnings: &["1 verification window(s) failed"],
            },
            Case {
                name: "strict missing verification windows",
                stats: healthy.clone(),
                verification: Some(VerifyStats::default()),
                criteria: verification_required,
                success: false,
                failures: &["verification enabled but no full decode window completed"],
                warnings: &[],
            },
            Case {
                name: "tolerant missing verifier result",
                stats: healthy,
                verification: None,
                criteria: AcceptanceCriteria {
                    verification_tolerant: true,
                    ..verification_required
                },
                success: true,
                failures: &[],
                warnings: &["decode verification did not produce results"],
            },
            Case {
                name: "successful verification",
                stats: all_cancelled,
                verification: Some(verified),
                criteria: verification_required,
                success: true,
                failures: &[],
                warnings: &["all sessions were cancelled before connecting; no traffic flowed"],
            },
        ];

        for case in cases {
            let report = RunReport::new(case.stats, case.verification, case.criteria);
            assert_eq!(report.schema_version, SCHEMA_VERSION, "{}", case.name);
            assert_eq!(report.success, case.success, "{}", case.name);
            assert_eq!(report.failures, case.failures, "{}", case.name);
            assert_eq!(report.warnings, case.warnings, "{}", case.name);
        }
    }
}
