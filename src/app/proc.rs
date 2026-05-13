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

#[cfg(target_os = "linux")]
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

#[cfg(target_os = "linux")]
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

#[cfg(target_os = "linux")]
fn foreground_pid_from_ps_line(line: &str) -> Option<u32> {
    let mut parts = line.split_whitespace();
    let pid = parts.next()?.parse::<u32>().ok()?;
    let stat = parts.next()?;
    stat.contains('+').then_some(pid)
}

#[cfg(target_os = "linux")]
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
    Ok(macos_list_child_pids(pid))
}

#[cfg(target_os = "macos")]
fn process_evidence_for_pid(pid: u32) -> Option<ProcessEvidence> {
    let process = macos_process_info_for_pid(pid)?;
    process_evidence_from_macos_info(process)
}

#[cfg(target_os = "macos")]
fn foreground_processes(pane_tty: &str) -> Result<Vec<ProcessEvidence>> {
    let Some(tty_device) = macos_tty_device_id(pane_tty) else {
        return Ok(Vec::new());
    };

    Ok(macos_list_all_pids()
        .into_iter()
        .filter_map(macos_process_info_for_pid)
        .filter(|process| process.is_foreground_on_tty(tty_device))
        .filter_map(process_evidence_from_macos_info)
        .collect())
}

#[cfg(target_os = "macos")]
#[derive(Clone, Debug)]
struct MacProcessInfo {
    pid: u32,
    process_group_id: u32,
    tty_device: u32,
    tty_process_group_id: u32,
    command: String,
}

#[cfg(target_os = "macos")]
impl MacProcessInfo {
    fn is_foreground_on_tty(&self, tty_device: u32) -> bool {
        self.tty_device == tty_device
            && self.tty_process_group_id != 0
            && self.process_group_id == self.tty_process_group_id
    }
}

#[cfg(target_os = "macos")]
fn process_evidence_from_macos_info(process: MacProcessInfo) -> Option<ProcessEvidence> {
    let argv = macos_argv_for_pid(process.pid);
    let command = command_basename(&process.command)
        .or_else(|| argv.first().and_then(|argv0| command_basename(argv0)))?;

    Some(ProcessEvidence {
        pid: process.pid,
        command,
        argv,
        env: Vec::new(),
    })
}

#[cfg(target_os = "macos")]
fn macos_process_info_for_pid(pid: u32) -> Option<MacProcessInfo> {
    let mut info = std::mem::MaybeUninit::<libc::proc_taskallinfo>::zeroed();
    let info_size = std::mem::size_of::<libc::proc_taskallinfo>() as libc::c_int;
    let bytes = unsafe {
        libc::proc_pidinfo(
            pid as libc::c_int,
            libc::PROC_PIDTASKALLINFO,
            0,
            info.as_mut_ptr().cast(),
            info_size,
        )
    };
    if bytes != info_size {
        return None;
    }

    let info = unsafe { info.assume_init() };
    let bsd = info.pbsd;
    Some(MacProcessInfo {
        pid: bsd.pbi_pid,
        process_group_id: bsd.pbi_pgid,
        tty_device: bsd.e_tdev,
        tty_process_group_id: bsd.e_tpgid,
        command: c_char_array_to_string(&bsd.pbi_comm),
    })
}

#[cfg(target_os = "macos")]
fn macos_list_child_pids(pid: u32) -> Vec<u32> {
    macos_pid_list(16, |buffer, bytes| unsafe {
        libc::proc_listchildpids(pid as libc::pid_t, buffer, bytes)
    })
}

#[cfg(target_os = "macos")]
fn macos_list_all_pids() -> Vec<u32> {
    macos_pid_list(1024, |buffer, bytes| unsafe {
        libc::proc_listallpids(buffer, bytes)
    })
}

#[cfg(target_os = "macos")]
fn macos_pid_list(
    initial_capacity: usize,
    mut list: impl FnMut(*mut libc::c_void, libc::c_int) -> libc::c_int,
) -> Vec<u32> {
    let pid_size = std::mem::size_of::<libc::pid_t>();
    let mut pids = vec![0 as libc::pid_t; initial_capacity.max(1)];

    loop {
        let Some(buffer_size) = pids
            .len()
            .checked_mul(pid_size)
            .and_then(|size| libc::c_int::try_from(size).ok())
        else {
            return Vec::new();
        };

        let pid_count = list(pids.as_mut_ptr().cast(), buffer_size);
        if pid_count <= 0 {
            return Vec::new();
        }

        let count = pid_count as usize;
        if count < pids.len() {
            return pids
                .into_iter()
                .take(count)
                .filter(|pid| *pid > 0)
                .filter_map(|pid| u32::try_from(pid).ok())
                .collect();
        }

        let Some(new_len) = pids.len().checked_mul(2) else {
            return Vec::new();
        };
        pids.resize(new_len, 0);
    }
}

#[cfg(target_os = "macos")]
fn macos_argv_for_pid(pid: u32) -> Vec<String> {
    let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid as libc::c_int];
    let mut buffer = vec![0_u8; macos_arg_buffer_len()];
    let mut buffer_len = buffer.len();

    let status = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            mib.len() as libc::c_uint,
            buffer.as_mut_ptr().cast(),
            &mut buffer_len,
            std::ptr::null_mut(),
            0,
        )
    };

    if status != 0 {
        return Vec::new();
    }

    buffer.truncate(buffer_len);
    macos_argv_from_procargs2_buffer(&buffer)
}

#[cfg(target_os = "macos")]
fn macos_arg_buffer_len() -> usize {
    let arg_max = unsafe { libc::sysconf(libc::_SC_ARG_MAX) };
    usize::try_from(arg_max)
        .ok()
        .filter(|size| *size >= 4096)
        .unwrap_or(262_144)
}

#[cfg(target_os = "macos")]
fn macos_argv_from_procargs2_buffer(buffer: &[u8]) -> Vec<String> {
    let argc_size = std::mem::size_of::<libc::c_int>();
    if buffer.len() < argc_size {
        return Vec::new();
    }

    let mut argc_bytes = [0_u8; std::mem::size_of::<libc::c_int>()];
    argc_bytes.copy_from_slice(&buffer[..argc_size]);
    let argc = libc::c_int::from_ne_bytes(argc_bytes);
    let Ok(argc) = usize::try_from(argc) else {
        return Vec::new();
    };

    let mut offset = argc_size;
    offset = skip_until_nul(buffer, offset);
    offset = skip_nuls(buffer, offset);

    let mut argv = Vec::with_capacity(argc.min(64));
    while offset < buffer.len() && argv.len() < argc {
        let Some(end) = buffer[offset..].iter().position(|byte| *byte == 0) else {
            break;
        };

        if end > 0
            && let Ok(arg) = std::str::from_utf8(&buffer[offset..offset + end])
        {
            argv.push(arg.to_string());
        }
        offset += end + 1;
    }

    argv
}

#[cfg(target_os = "macos")]
fn skip_until_nul(buffer: &[u8], offset: usize) -> usize {
    buffer[offset..]
        .iter()
        .position(|byte| *byte == 0)
        .map_or(buffer.len(), |position| offset + position + 1)
}

#[cfg(target_os = "macos")]
fn skip_nuls(buffer: &[u8], mut offset: usize) -> usize {
    while offset < buffer.len() && buffer[offset] == 0 {
        offset += 1;
    }
    offset
}

#[cfg(target_os = "macos")]
fn macos_tty_device_id(pane_tty: &str) -> Option<u32> {
    use std::os::unix::fs::MetadataExt;

    let tty = pane_tty.trim();
    if tty.is_empty() || tty.eq_ignore_ascii_case("not a tty") {
        return None;
    }

    let path = tty.strip_prefix("/dev/").map_or_else(
        || std::path::PathBuf::from("/dev").join(tty),
        |_| std::path::PathBuf::from(tty),
    );
    let metadata = fs::metadata(path).ok()?;
    u32::try_from(metadata.rdev()).ok()
}

#[cfg(target_os = "macos")]
fn c_char_array_to_string(bytes: &[libc::c_char]) -> String {
    let end = bytes
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(bytes.len());
    let bytes = bytes[..end]
        .iter()
        .map(|byte| *byte as u8)
        .collect::<Vec<_>>();

    String::from_utf8_lossy(&bytes).trim().to_string()
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

    #[cfg(target_os = "linux")]
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
    fn macos_pid_list_treats_libproc_return_as_pid_count() {
        let pids = macos_pid_list(16, |buffer, _bytes| {
            let output = unsafe { std::slice::from_raw_parts_mut(buffer.cast::<libc::pid_t>(), 3) };
            output.copy_from_slice(&[11, 12, 13]);
            3
        });

        assert_eq!(pids, vec![11, 12, 13]);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_pid_list_grows_past_initially_full_large_buffers() {
        let pids = macos_pid_list(4096, |buffer, bytes| {
            let pid_capacity = bytes as usize / std::mem::size_of::<libc::pid_t>();
            let pid_count = if pid_capacity == 4096 { 4096 } else { 4097 };
            let output =
                unsafe { std::slice::from_raw_parts_mut(buffer.cast::<libc::pid_t>(), pid_count) };
            for (index, pid) in output.iter_mut().enumerate() {
                *pid = libc::pid_t::try_from(index + 1).expect("test pid should fit");
            }
            libc::c_int::try_from(pid_count).expect("test pid count should fit")
        });

        assert_eq!(pids.len(), 4097);
        assert_eq!(pids.last(), Some(&4097));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_argv_parser_reads_procargs2_arguments_only() {
        let mut buffer = Vec::new();
        buffer.extend_from_slice(&(2 as libc::c_int).to_ne_bytes());
        buffer.extend_from_slice(b"/bin/zsh\0\0\0");
        buffer.extend_from_slice(b"zsh\0-l\0SHELL=/bin/zsh\0");

        assert_eq!(
            macos_argv_from_procargs2_buffer(&buffer),
            vec!["zsh".to_string(), "-l".to_string()]
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_foreground_match_requires_tty_and_foreground_group() {
        let process = MacProcessInfo {
            pid: 123,
            process_group_id: 42,
            tty_device: 7,
            tty_process_group_id: 42,
            command: "zsh".to_string(),
        };

        assert!(process.is_foreground_on_tty(7));

        let background = MacProcessInfo {
            process_group_id: 41,
            ..process.clone()
        };
        assert!(!background.is_foreground_on_tty(7));

        let other_tty = MacProcessInfo {
            tty_device: 8,
            ..process
        };
        assert!(!other_tty.is_foreground_on_tty(7));
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
