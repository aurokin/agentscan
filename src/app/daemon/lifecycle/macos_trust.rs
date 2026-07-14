// Only the macOS-gated items reference parent names (`Command`, `Path`); the
// test-visible helpers are self-contained. An ungated glob is an unused import
// on non-macOS builds (lib and test alike), which CI rejects via `-D warnings`.
#[cfg(target_os = "macos")]
use super::*;

#[cfg(any(test, target_os = "macos"))]
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum MacExecutableAssessment {
    Trusted,
    Untrusted(String),
}
#[cfg(target_os = "macos")]
pub(crate) fn assess_macos_executable_for_daemon_autostart(path: &Path) -> MacExecutableAssessment {
    let display_output = match Command::new("/usr/bin/codesign")
        .args(["-dv", "--verbose=4"])
        .arg(path)
        .output()
    {
        Ok(output) => output,
        Err(error) => {
            return MacExecutableAssessment::Untrusted(format!(
                "codesign inspection could not run: {error}"
            ));
        }
    };
    let display_text = command_output_text(&display_output);
    if let Some(assessment) =
        macos_codesign_display_rejection(display_output.status.success(), &display_text)
    {
        return assessment;
    }

    let verify_output = match Command::new("/usr/bin/codesign")
        .args(["--verify", "--verbose=4"])
        .arg(path)
        .output()
    {
        Ok(output) => output,
        Err(error) => {
            return MacExecutableAssessment::Untrusted(format!(
                "codesign verification could not run: {error}"
            ));
        }
    };
    if let MacExecutableAssessment::Untrusted(reason) = assess_macos_codesign_verification(
        verify_output.status.success(),
        &command_output_text(&verify_output),
    ) {
        return MacExecutableAssessment::Untrusted(reason);
    }

    MacExecutableAssessment::Trusted
}

#[cfg(any(test, target_os = "macos"))]
fn macos_codesign_display_rejection(
    status_success: bool,
    text: &str,
) -> Option<MacExecutableAssessment> {
    if !status_success {
        return Some(MacExecutableAssessment::Untrusted(format!(
            "codesign inspection failed: {}",
            compact_command_output(text)
        )));
    }

    let lower = text.to_ascii_lowercase();
    if lower.contains("signature=adhoc") || lower.contains("(adhoc") || lower.contains("adhoc,") {
        return Some(MacExecutableAssessment::Untrusted(format!(
            "codesign reports an ad-hoc executable: {}",
            compact_command_output(text)
        )));
    }
    None
}

#[cfg(any(test, target_os = "macos"))]
fn assess_macos_codesign_verification(status_success: bool, text: &str) -> MacExecutableAssessment {
    if status_success {
        MacExecutableAssessment::Trusted
    } else {
        MacExecutableAssessment::Untrusted(format!(
            "codesign verification failed: {}",
            compact_command_output(text)
        ))
    }
}

#[cfg(test)]
pub(crate) fn test_macos_executable_assessment_for_outputs(
    display_status_success: bool,
    display_text: &str,
    verify_status_success: bool,
    verify_text: &str,
) -> std::result::Result<(), String> {
    if let Some(MacExecutableAssessment::Untrusted(reason)) =
        macos_codesign_display_rejection(display_status_success, display_text)
    {
        return Err(reason);
    }

    if let MacExecutableAssessment::Untrusted(reason) =
        assess_macos_codesign_verification(verify_status_success, verify_text)
    {
        return Err(reason);
    }

    Ok(())
}

#[cfg(target_os = "macos")]
fn command_output_text(output: &std::process::Output) -> String {
    let mut text = String::new();
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    if !output.stdout.is_empty() {
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(&String::from_utf8_lossy(&output.stdout));
    }
    text
}

#[cfg(any(test, target_os = "macos"))]
fn compact_command_output(text: &str) -> String {
    let compact = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("; ");
    if compact.chars().count() > 500 {
        let prefix = compact.chars().take(500).collect::<String>();
        format!("{prefix}...")
    } else {
        compact
    }
}
