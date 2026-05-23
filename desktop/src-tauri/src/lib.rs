use std::{
    env,
    ffi::{OsStr, OsString},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

const PREFLIGHT_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct DesktopProfile {
    id: &'static str,
    name: &'static str,
    kind: &'static str,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct AgentscanPreflight {
    binary: String,
    ok: bool,
    version: Option<String>,
    error: Option<String>,
}

#[tauri::command]
fn local_profiles() -> Vec<DesktopProfile> {
    vec![DesktopProfile {
        id: "local",
        name: "Local",
        kind: "local",
    }]
}

#[tauri::command]
fn preflight_agentscan() -> AgentscanPreflight {
    run_agentscan_preflight(agentscan_binary())
}

fn agentscan_binary() -> OsString {
    env::var_os("AGENTSCAN_DESKTOP_AGENTSCAN_BIN")
        .or_else(|| find_known_agentscan_binary().map(PathBuf::into_os_string))
        .unwrap_or_else(|| OsString::from("agentscan"))
}

fn find_known_agentscan_binary() -> Option<PathBuf> {
    known_agentscan_paths(env::var_os("HOME").as_deref()).find(|path| path.is_file())
}

fn known_agentscan_paths(home: Option<&OsStr>) -> impl Iterator<Item = PathBuf> {
    let home_candidate = home
        .filter(|home| !home.is_empty())
        .map(|home| Path::new(home).join(".cargo/bin/agentscan"));

    [
        home_candidate,
        Some(PathBuf::from("/opt/homebrew/bin/agentscan")),
        Some(PathBuf::from("/usr/local/bin/agentscan")),
    ]
    .into_iter()
    .flatten()
}

fn run_agentscan_preflight(binary: OsString) -> AgentscanPreflight {
    run_agentscan_preflight_with_timeout(binary, PREFLIGHT_TIMEOUT)
}

fn run_agentscan_preflight_with_timeout(binary: OsString, timeout: Duration) -> AgentscanPreflight {
    let binary_display = binary.to_string_lossy().into_owned();

    let mut child = match Command::new(&binary)
        .arg("--version")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(error) => {
            return AgentscanPreflight {
                binary: binary_display,
                ok: false,
                version: None,
                error: Some(error.to_string()),
            };
        }
    };

    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if start.elapsed() >= timeout => {
                let _ = child.kill();
                let _ = child.wait();
                return AgentscanPreflight {
                    binary: binary_display,
                    ok: false,
                    version: None,
                    error: Some(format!(
                        "agentscan preflight timed out after {}ms",
                        timeout.as_millis()
                    )),
                };
            }
            Ok(None) => thread::sleep(Duration::from_millis(10)),
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return AgentscanPreflight {
                    binary: binary_display,
                    ok: false,
                    version: None,
                    error: Some(error.to_string()),
                };
            }
        }
    }

    match child.wait_with_output() {
        Ok(output) if output.status.success() => AgentscanPreflight {
            binary: binary_display,
            ok: true,
            version: Some(String::from_utf8_lossy(&output.stdout).trim().to_owned()),
            error: None,
        },
        Ok(output) => AgentscanPreflight {
            binary: binary_display,
            ok: false,
            version: None,
            error: Some(preflight_stderr_or_status(&output.stderr, output.status)),
        },
        Err(error) => AgentscanPreflight {
            binary: binary_display,
            ok: false,
            version: None,
            error: Some(error.to_string()),
        },
    }
}

fn preflight_stderr_or_status(stderr: &[u8], status: std::process::ExitStatus) -> String {
    let stderr = String::from_utf8_lossy(stderr).trim().to_owned();

    if stderr.is_empty() {
        format!("agentscan exited with status {status}")
    } else {
        stderr
    }
}

pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            local_profiles,
            preflight_agentscan
        ])
        .run(tauri::generate_context!())
        .expect("error while running agentscan desktop");
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, os::unix::fs::PermissionsExt};

    #[test]
    fn local_profile_is_built_in() {
        assert_eq!(
            local_profiles(),
            vec![DesktopProfile {
                id: "local",
                name: "Local",
                kind: "local"
            }]
        );
    }

    #[test]
    fn missing_preflight_binary_reports_failure() {
        let result = run_agentscan_preflight(OsString::from("agentscan-missing-for-test"));

        assert_eq!(result.binary, "agentscan-missing-for-test");
        assert!(!result.ok);
        assert!(result.version.is_none());
        assert!(result.error.is_some());
    }

    #[test]
    fn preflight_times_out_hanging_binary() {
        let script = env::temp_dir().join(format!(
            "agentscan-preflight-hang-{}-{}.sh",
            std::process::id(),
            "timeout"
        ));
        fs::write(&script, "#!/bin/sh\nsleep 5\n").expect("write test script");
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755))
            .expect("make test script executable");

        let result = run_agentscan_preflight_with_timeout(
            script.clone().into_os_string(),
            Duration::from_millis(50),
        );
        let _ = fs::remove_file(script);

        assert!(!result.ok);
        assert!(result.version.is_none());
        assert!(result.error.as_deref().unwrap_or("").contains("timed out"));
    }

    #[test]
    fn known_agentscan_paths_include_gui_launch_locations() {
        let paths: Vec<_> = known_agentscan_paths(Some(OsStr::new("/Users/example"))).collect();

        assert_eq!(
            paths,
            vec![
                PathBuf::from("/Users/example/.cargo/bin/agentscan"),
                PathBuf::from("/opt/homebrew/bin/agentscan"),
                PathBuf::from("/usr/local/bin/agentscan"),
            ]
        );
    }

    #[test]
    fn known_agentscan_paths_skip_empty_home() {
        let paths: Vec<_> = known_agentscan_paths(Some(OsStr::new(""))).collect();

        assert_eq!(
            paths,
            vec![
                PathBuf::from("/opt/homebrew/bin/agentscan"),
                PathBuf::from("/usr/local/bin/agentscan"),
            ]
        );
    }
}
