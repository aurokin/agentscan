use super::*;

pub(crate) trait ProcessInspector {
    fn descendant_processes(&self, root_pid: u32) -> Result<Vec<ProcessEvidence>>;

    fn foreground_processes(&self, pane_tty: &str) -> Result<Vec<ProcessEvidence>> {
        let _ = pane_tty;
        Ok(Vec::new())
    }
}

#[derive(Default)]
pub(crate) struct ProcProcessInspector;

impl ProcessInspector for ProcProcessInspector {
    fn descendant_processes(&self, root_pid: u32) -> Result<Vec<ProcessEvidence>> {
        descendant_processes(root_pid)
    }

    fn foreground_processes(&self, pane_tty: &str) -> Result<Vec<ProcessEvidence>> {
        foreground_processes(pane_tty)
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) struct ProcessEvidence {
    pub(crate) pid: u32,
    pub(crate) command: String,
    pub(crate) argv: Vec<String>,
    pub(crate) env: Vec<(String, String)>,
}

impl ProcessEvidence {
    pub(crate) fn command_for_diagnostics(&self) -> String {
        if !self.command.trim().is_empty() {
            return self.command.trim().to_string();
        }

        self.argv
            .first()
            .and_then(|arg| command_basename(arg))
            .unwrap_or_else(|| format!("pid:{}", self.pid))
    }

    pub(crate) fn has_env(&self, key: &str, expected: &str) -> bool {
        self.env
            .iter()
            .any(|(env_key, value)| env_key == key && value == expected)
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn descendant_processes(root_pid: u32) -> Result<Vec<ProcessEvidence>> {
    const MAX_PROCESSES: usize = 64;

    let mut processes = Vec::new();
    let mut queue = vec![root_pid];
    let mut visited = std::collections::HashSet::new();

    while let Some(pid) = queue.pop() {
        if !visited.insert(pid) || visited.len() > MAX_PROCESSES {
            continue;
        }

        if let Some(process) = process_evidence_for_pid(pid) {
            processes.push(process);
        }

        queue.extend(children_for_pid(pid)?);
    }

    Ok(processes)
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn descendant_processes(_root_pid: u32) -> Result<Vec<ProcessEvidence>> {
    Ok(Vec::new())
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn foreground_processes(pane_tty: &str) -> Result<Vec<ProcessEvidence>> {
    let Some(tty) = ps_tty_name(pane_tty) else {
        return Ok(Vec::new());
    };

    foreground_pids_for_tty(&tty).map(|pids| {
        pids.into_iter()
            .filter_map(process_evidence_for_pid)
            .collect()
    })
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn foreground_processes(_pane_tty: &str) -> Result<Vec<ProcessEvidence>> {
    Ok(Vec::new())
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn foreground_pids_for_tty(tty: &str) -> Result<Vec<u32>> {
    let output = Command::new("ps")
        .args(["-t", tty, "-o", "pid=", "-o", "stat="])
        .output()
        .context("failed to execute ps for foreground process fallback")?;

    if !output.status.success() {
        return Ok(Vec::new());
    }

    let stdout = String::from_utf8(output.stdout).context("ps output was not valid UTF-8")?;
    Ok(stdout
        .lines()
        .filter_map(foreground_pid_from_ps_line)
        .collect())
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn foreground_pid_from_ps_line(line: &str) -> Option<u32> {
    let mut parts = line.split_whitespace();
    let pid = parts.next()?.parse::<u32>().ok()?;
    let stat = parts.next()?;
    stat.contains('+').then_some(pid)
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn ps_tty_name(pane_tty: &str) -> Option<String> {
    let tty = pane_tty.trim();
    if tty.is_empty() || tty == "not a tty" {
        return None;
    }

    Some(tty.strip_prefix("/dev/").unwrap_or(tty).to_string())
}

#[cfg(target_os = "linux")]
fn children_for_pid(pid: u32) -> Result<Vec<u32>> {
    let path = format!("/proc/{pid}/task/{pid}/children");
    let Ok(contents) = fs::read_to_string(path) else {
        return Ok(Vec::new());
    };

    Ok(contents
        .split_whitespace()
        .filter_map(|value| value.parse::<u32>().ok())
        .collect())
}

#[cfg(target_os = "linux")]
fn process_evidence_for_pid(pid: u32) -> Option<ProcessEvidence> {
    let argv = argv_for_pid(pid);
    let command = command_from_comm(pid).or_else(|| {
        argv.first()
            .and_then(|argv0| command_basename(argv0))
            .filter(|command| !command.trim().is_empty())
    })?;
    Some(ProcessEvidence {
        pid,
        command,
        argv,
        env: selected_env_for_pid(pid),
    })
}

#[cfg(target_os = "linux")]
fn argv_for_pid(pid: u32) -> Vec<String> {
    let path = format!("/proc/{pid}/cmdline");
    let Ok(contents) = fs::read(path) else {
        return Vec::new();
    };

    contents
        .split(|byte| *byte == 0)
        .filter(|part| !part.is_empty())
        .filter_map(|part| std::str::from_utf8(part).ok())
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(str::to_string)
        .collect()
}

#[cfg(target_os = "linux")]
fn selected_env_for_pid(pid: u32) -> Vec<(String, String)> {
    const SELECTED_ENV_KEYS: &[&str] = &[
        "CLAUDECODE",
        "CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS",
        "CLAUDE_CODE_ENTRYPOINT",
        "CLAUDE_CODE_AGENT",
        "CLAUDE_CODE_REMOTE",
        "PI_CODING_AGENT",
        "OPENCODE",
        "OPENCODE_PID",
        "OPENCODE_RUN_ID",
        "OPENCODE_PROCESS_ROLE",
    ];

    let path = format!("/proc/{pid}/environ");
    let Ok(contents) = fs::read(path) else {
        return Vec::new();
    };

    contents
        .split(|byte| *byte == 0)
        .filter(|part| !part.is_empty())
        .filter_map(|part| std::str::from_utf8(part).ok())
        .filter_map(|entry| entry.split_once('='))
        .filter(|(key, _)| SELECTED_ENV_KEYS.contains(key))
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect()
}

#[cfg(target_os = "linux")]
fn command_from_comm(pid: u32) -> Option<String> {
    let path = format!("/proc/{pid}/comm");
    let raw = fs::read_to_string(path).ok()?;
    command_basename(raw.trim())
}

#[cfg(target_os = "macos")]
fn children_for_pid(pid: u32) -> Result<Vec<u32>> {
    let output = Command::new("pgrep")
        .args(["-P", &pid.to_string()])
        .output()
        .context("failed to execute pgrep for process fallback")?;

    if !output.status.success() {
        return Ok(Vec::new());
    }

    let stdout = String::from_utf8(output.stdout).context("pgrep output was not valid UTF-8")?;
    Ok(stdout
        .split_whitespace()
        .filter_map(|value| value.parse::<u32>().ok())
        .collect())
}

#[cfg(target_os = "macos")]
fn process_evidence_for_pid(pid: u32) -> Option<ProcessEvidence> {
    let comm = ps_field(pid, "comm=").unwrap_or_default();
    let args = ps_field(pid, "args=").unwrap_or_default();
    let argv = split_process_args(&args);
    let command = command_basename(&comm)
        .or_else(|| argv.first().and_then(|argv0| command_basename(argv0)))?;

    Some(ProcessEvidence {
        pid,
        command,
        argv,
        env: Vec::new(),
    })
}

#[cfg(target_os = "macos")]
fn ps_field(pid: u32, field: &str) -> Option<String> {
    let output = Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", field])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }

    String::from_utf8(output.stdout)
        .ok()
        .map(|stdout| stdout.trim().to_string())
        .filter(|value| !value.is_empty())
}

#[cfg(target_os = "macos")]
fn split_process_args(args: &str) -> Vec<String> {
    args.split_whitespace()
        .filter(|arg| !arg.trim().is_empty())
        .map(str::to_string)
        .collect()
}

fn command_basename(raw: &str) -> Option<String> {
    let value = raw.trim();
    if value.is_empty() {
        return None;
    }

    Path::new(value)
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.trim().is_empty())
        .map(str::to_string)
}

#[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
mod tests {
    use super::*;

    #[test]
    fn process_tree_fallback_includes_root_process() {
        let mut child = Command::new("sleep")
            .arg("5")
            .spawn()
            .expect("spawn sleep process");

        let processes = descendant_processes(child.id()).expect("collect process evidence");

        let _ = child.kill();
        let _ = child.wait();

        assert!(
            processes.iter().any(|process| process.pid == child.id()),
            "expected root process evidence, got {processes:?}"
        );
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn foreground_ps_helpers_accept_only_foreground_rows() {
        assert_eq!(foreground_pid_from_ps_line("1234 Ss+"), Some(1234));
        assert_eq!(foreground_pid_from_ps_line("1235 S"), None);
        assert_eq!(foreground_pid_from_ps_line("not-a-pid Ss+"), None);
        assert_eq!(ps_tty_name("/dev/ttys001").as_deref(), Some("ttys001"));
        assert_eq!(ps_tty_name("/dev/pts/4").as_deref(), Some("pts/4"));
        assert_eq!(ps_tty_name("not a tty"), None);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_process_evidence_does_not_claim_hidden_env_support() {
        let mut child = Command::new("sleep")
            .arg("5")
            .env("PI_CODING_AGENT", "true")
            .spawn()
            .expect("spawn sleep process with env");

        let evidence = process_evidence_for_pid(child.id()).expect("process evidence");

        let _ = child.kill();
        let _ = child.wait();

        assert_eq!(evidence.env, Vec::<(String, String)>::new());
    }
}
