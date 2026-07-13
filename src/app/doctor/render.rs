use super::*;
// Doctor text output is buffered and emitted through `output::write_stdout` so a
// closed pipe surfaces as a recoverable `BrokenPipe` instead of a `println!` panic.
use std::fmt::Write as _;

use serde_json::Value;

pub(super) fn print_doctor_text(out: &mut String, report: &DoctorReport) {
    let _ = writeln!(
        out,
        "agentscan doctor — {} (ok: {}, warn: {}, fail: {})",
        report.summary.status.label(),
        report.summary.ok_count,
        report.summary.warn_count,
        report.summary.fail_count,
    );
    for check in &report.checks {
        let _ = writeln!(
            out,
            "[{}] {} — {}",
            check.status.label(),
            check.id,
            check.message
        );
        if let Some(details) = &check.details {
            print_detail_lines(out, details);
        }
    }
}

fn print_detail_lines(out: &mut String, details: &Value) {
    let Value::Object(map) = details else {
        return;
    };
    for (key, value) in map {
        if value.is_null() {
            continue;
        }
        let rendered = match value {
            Value::String(text) => text.clone(),
            other => other.to_string(),
        };
        let _ = writeln!(out, "    {key}: {rendered}");
    }
}
