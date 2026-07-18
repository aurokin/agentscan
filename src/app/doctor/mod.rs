use super::*;
use serde_json::Value;

mod checks;
mod render;

use checks::*;
use render::print_doctor_text;

/// Versioned envelope for `agentscan doctor --format json`. Bump when the report
/// shape changes in a way machine consumers must notice.
const DOCTOR_SCHEMA_VERSION: u32 = 2;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
enum CheckStatus {
    Ok,
    Warn,
    Fail,
    Info,
}

impl CheckStatus {
    fn label(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Warn => "warn",
            Self::Fail => "fail",
            Self::Info => "info",
        }
    }
}

#[derive(Debug, Serialize)]
struct DoctorCheck {
    id: &'static str,
    status: CheckStatus,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    details: Option<Value>,
}

impl DoctorCheck {
    fn new(id: &'static str, status: CheckStatus, message: String, details: Option<Value>) -> Self {
        Self {
            id,
            status,
            message,
            details,
        }
    }
}

#[derive(Debug, Serialize)]
struct DoctorSummary {
    status: CheckStatus,
    ok_count: usize,
    warn_count: usize,
    fail_count: usize,
}

#[derive(Debug, Serialize)]
struct DoctorReport {
    schema_version: u32,
    generated_at: String,
    summary: DoctorSummary,
    checks: Vec<DoctorCheck>,
}

pub(crate) fn run_doctor(args: DoctorArgs) -> Result<()> {
    let report = build_report(args);
    match args.format {
        OutputFormat::Json => output::print_json(&report)?,
        OutputFormat::Text => {
            let mut out = String::new();
            print_doctor_text(&mut out, &report);
            output::write_stdout(&out)?;
        }
    }
    Ok(())
}

fn build_report(args: DoctorArgs) -> DoctorReport {
    let checks = collect_checks(args);
    let summary = summarize_checks(&checks);
    DoctorReport {
        schema_version: DOCTOR_SCHEMA_VERSION,
        generated_at: snapshot::now_rfc3339().unwrap_or_default(),
        summary,
        checks,
    }
}

fn collect_checks(args: DoctorArgs) -> Vec<DoctorCheck> {
    let refresh = args.refresh.refresh;
    // A direct tmux read only when explicitly requested; the daemon snapshot is
    // always queried read-only (never auto-starts) for the default summary and
    // for the `--refresh` comparison baseline.
    let direct: Option<Result<SnapshotEnvelope, String>> =
        refresh.then(|| scanner::snapshot_from_tmux().map_err(|error| format!("{error:#}")));
    let daemon_snapshot = daemon::snapshot_via_socket(no_auto_start_policy());

    // `discovery.summary` reports on the *requested* source so a failed direct
    // `--refresh` read stays visible as a warning rather than silently falling
    // back to daemon state.
    let summary_source: Result<&SnapshotEnvelope, String> = match &direct {
        Some(result) => result.as_ref().map_err(Clone::clone),
        None => daemon_snapshot.as_ref().map_err(ToString::to_string),
    };
    let summary_source_ref: Result<&SnapshotEnvelope, &str> = summary_source
        .as_ref()
        .map(|snapshot| *snapshot)
        .map_err(String::as_str);
    // The picker contract uses whatever snapshot is available (direct preferred,
    // daemon fallback) so a broken direct read does not also hide the daemon's
    // usable picker state.
    let picker_input = best_snapshot(&direct, &daemon_snapshot);

    let mut checks = vec![
        binary_version_check(),
        macos_trust_check(),
        config_check(),
        tmux_check(),
        daemon_health_check(args.events),
        discovery_summary_check(summary_source_ref, refresh),
    ];
    if refresh
        && let (Some(Ok(direct_snapshot)), Ok(daemon_snapshot)) =
            (&direct, daemon_snapshot.as_ref())
    {
        checks.push(discovery_compare_check(direct_snapshot, daemon_snapshot));
    }
    checks.push(picker_contract_check(picker_input));
    checks
}

fn best_snapshot<'a>(
    direct: &'a Option<Result<SnapshotEnvelope, String>>,
    daemon: &'a Result<SnapshotEnvelope, daemon::DaemonSnapshotError>,
) -> Option<&'a SnapshotEnvelope> {
    if let Some(Ok(snapshot)) = direct {
        return Some(snapshot);
    }
    daemon.as_ref().ok()
}

fn no_auto_start_policy() -> daemon::AutoStartPolicy {
    daemon::AutoStartPolicy::from_args(AutoStartArgs {
        no_auto_start: true,
    })
}

fn summarize_checks(checks: &[DoctorCheck]) -> DoctorSummary {
    let mut ok_count = 0;
    let mut warn_count = 0;
    let mut fail_count = 0;
    for check in checks {
        match check.status {
            CheckStatus::Ok => ok_count += 1,
            CheckStatus::Warn => warn_count += 1,
            CheckStatus::Fail => fail_count += 1,
            CheckStatus::Info => {}
        }
    }
    let status = if fail_count > 0 {
        CheckStatus::Fail
    } else if warn_count > 0 {
        CheckStatus::Warn
    } else {
        CheckStatus::Ok
    };
    DoctorSummary {
        status,
        ok_count,
        warn_count,
        fail_count,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(id: &'static str, status: CheckStatus) -> DoctorCheck {
        DoctorCheck::new(id, status, "synthetic".to_string(), None)
    }

    #[test]
    fn summary_status_prefers_fail_over_warn_over_ok() {
        let checks = vec![
            check("a", CheckStatus::Ok),
            check("b", CheckStatus::Warn),
            check("c", CheckStatus::Fail),
            check("d", CheckStatus::Info),
        ];
        let summary = summarize_checks(&checks);
        assert_eq!(summary.status, CheckStatus::Fail);
        assert_eq!(summary.ok_count, 1);
        assert_eq!(summary.warn_count, 1);
        assert_eq!(summary.fail_count, 1);
    }

    #[test]
    fn summary_status_is_warn_without_failures() {
        let checks = vec![check("a", CheckStatus::Ok), check("b", CheckStatus::Warn)];
        assert_eq!(summarize_checks(&checks).status, CheckStatus::Warn);
    }

    #[test]
    fn summary_status_is_ok_when_only_ok_and_info() {
        let checks = vec![check("a", CheckStatus::Ok), check("b", CheckStatus::Info)];
        let summary = summarize_checks(&checks);
        assert_eq!(summary.status, CheckStatus::Ok);
        // Info checks are excluded from all counts.
        assert_eq!(summary.ok_count, 1);
        assert_eq!(summary.warn_count, 0);
        assert_eq!(summary.fail_count, 0);
    }
}
