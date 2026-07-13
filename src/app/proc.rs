use super::*;

/// Builds a per-scan [`ProcessSnapshot`]. Enumerating the process table is done
/// once via [`ProcessInspector::snapshot`]; the returned snapshot answers every
/// pane's foreground/descendant query from the prebuilt index rather than
/// re-scanning all PIDs per pane.
pub(crate) trait ProcessInspector {
    type Snapshot<'a>: ProcessSnapshot
    where
        Self: 'a;

    fn snapshot(&self) -> Self::Snapshot<'_>;
}

/// A single enumeration of the process table, indexed so per-pane foreground and
/// descendant lookups need no further table-wide scans. Expensive per-PID detail
/// (argv/env via `KERN_PROCARGS2` on macOS, `/proc/<pid>/{cmdline,environ}` on
/// Linux) stays lazy: it is fetched only for the PIDs a query actually matches.
pub(crate) trait ProcessSnapshot {
    fn descendant_processes(&self, root_pid: u32) -> Result<Vec<ProcessEvidence>>;

    fn foreground_processes(&self, pane_tty: &str) -> Result<Vec<ProcessEvidence>> {
        let _ = pane_tty;
        Ok(Vec::new())
    }
}

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

#[derive(Default)]
pub(crate) struct ProcProcessInspector;

impl ProcessInspector for ProcProcessInspector {
    type Snapshot<'a> = ProcTableSnapshot;

    fn snapshot(&self) -> ProcTableSnapshot {
        ProcTableSnapshot::capture()
    }
}

/// Prebuilt indexes over one process-table enumeration:
/// - `foreground_by_tty`: tty device id -> PIDs whose process group is that
///   tty's foreground group (the per-pane foreground lookup).
/// - `children_by_ppid`: parent PID -> child PIDs (the descendant walk).
#[derive(Default)]
pub(crate) struct ProcTableSnapshot {
    foreground_by_tty: std::collections::HashMap<u64, Vec<u32>>,
    children_by_ppid: std::collections::HashMap<u32, Vec<u32>>,
}

impl ProcessSnapshot for ProcTableSnapshot {
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    fn descendant_processes(&self, root_pid: u32) -> Result<Vec<ProcessEvidence>> {
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

            if let Some(children) = self.children_by_ppid.get(&pid) {
                queue.extend(children.iter().copied());
            }
        }

        Ok(processes)
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    fn descendant_processes(&self, _root_pid: u32) -> Result<Vec<ProcessEvidence>> {
        Ok(Vec::new())
    }

    #[cfg(target_os = "linux")]
    fn foreground_processes(&self, pane_tty: &str) -> Result<Vec<ProcessEvidence>> {
        let Some(tty_device) = linux_tty_device_id(pane_tty) else {
            return Ok(Vec::new());
        };
        Ok(self.evidence_for_foreground_tty(tty_device))
    }

    #[cfg(target_os = "macos")]
    fn foreground_processes(&self, pane_tty: &str) -> Result<Vec<ProcessEvidence>> {
        let Some(tty_device) = macos_tty_device_id(pane_tty) else {
            return Ok(Vec::new());
        };
        Ok(self.evidence_for_foreground_tty(u64::from(tty_device)))
    }
}

impl ProcTableSnapshot {
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    fn evidence_for_foreground_tty(&self, tty_device: u64) -> Vec<ProcessEvidence> {
        self.foreground_by_tty
            .get(&tty_device)
            .into_iter()
            .flatten()
            .filter_map(|pid| process_evidence_for_pid(*pid))
            .collect()
    }

    #[cfg(target_os = "linux")]
    fn capture() -> Self {
        let mut snapshot = Self::default();
        for stat in linux_list_all_pids()
            .into_iter()
            .filter_map(linux_process_stat_for_pid)
        {
            snapshot
                .children_by_ppid
                .entry(stat.parent_pid)
                .or_default()
                .push(stat.pid);
            if let Ok(tty_device) = u64::try_from(stat.tty_device)
                && stat.is_foreground_on_tty(tty_device)
            {
                snapshot
                    .foreground_by_tty
                    .entry(tty_device)
                    .or_default()
                    .push(stat.pid);
            }
        }
        snapshot
    }

    #[cfg(target_os = "macos")]
    fn capture() -> Self {
        let mut snapshot = Self::default();
        for info in macos_list_all_pids()
            .into_iter()
            .filter_map(macos_process_info_for_pid)
        {
            snapshot
                .children_by_ppid
                .entry(info.parent_pid)
                .or_default()
                .push(info.pid);
            if info.is_foreground_on_tty(info.tty_device) {
                snapshot
                    .foreground_by_tty
                    .entry(u64::from(info.tty_device))
                    .or_default()
                    .push(info.pid);
            }
        }
        snapshot
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    fn capture() -> Self {
        Self::default()
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

fn tty_path_from_pane_tty(pane_tty: &str) -> Option<PathBuf> {
    let tty = pane_tty.trim();
    if tty.is_empty() || tty.eq_ignore_ascii_case("not a tty") {
        return None;
    }

    Some(
        tty.strip_prefix("/dev/")
            .map_or_else(|| PathBuf::from("/dev").join(tty), |_| PathBuf::from(tty)),
    )
}

fn process_is_foreground_on_tty(
    pane_tty_device: u64,
    process_tty_device: u64,
    process_group_id: i64,
    tty_process_group_id: i64,
) -> bool {
    process_tty_device == pane_tty_device
        && tty_process_group_id > 0
        && process_group_id == tty_process_group_id
}

fn strings_from_nul_separated_bytes(bytes: &[u8]) -> Vec<String> {
    bytes
        .split(|byte| *byte == 0)
        .filter(|part| !part.is_empty())
        .filter_map(|part| std::str::from_utf8(part).ok())
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(str::to_string)
        .collect()
}

fn selected_env_from_nul_separated_bytes(bytes: &[u8]) -> Vec<(String, String)> {
    strings_from_nul_separated_bytes(bytes)
        .into_iter()
        .filter_map(|entry| {
            let (key, value) = entry.split_once('=')?;
            SELECTED_ENV_KEYS
                .contains(&key)
                .then(|| (key.to_string(), value.to_string()))
        })
        .collect()
}

#[cfg(target_os = "linux")]
fn linux_tty_device_id(pane_tty: &str) -> Option<u64> {
    use std::os::unix::fs::MetadataExt;

    let path = tty_path_from_pane_tty(pane_tty)?;
    fs::metadata(path).ok().map(|metadata| metadata.rdev())
}

#[cfg(target_os = "linux")]
#[derive(Clone, Debug, Eq, PartialEq)]
struct LinuxProcessStat {
    pid: u32,
    parent_pid: u32,
    process_group_id: i64,
    tty_device: i64,
    tty_process_group_id: i64,
}

#[cfg(target_os = "linux")]
impl LinuxProcessStat {
    fn is_foreground_on_tty(&self, tty_device: u64) -> bool {
        u64::try_from(self.tty_device)
            .ok()
            .is_some_and(|process_tty| {
                process_is_foreground_on_tty(
                    tty_device,
                    process_tty,
                    self.process_group_id,
                    self.tty_process_group_id,
                )
            })
    }
}

#[cfg(target_os = "linux")]
fn linux_list_all_pids() -> Vec<u32> {
    let Ok(entries) = fs::read_dir("/proc") else {
        return Vec::new();
    };

    entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            entry
                .file_name()
                .to_str()
                .and_then(|name| name.parse().ok())
        })
        .collect()
}

#[cfg(target_os = "linux")]
fn linux_process_stat_for_pid(pid: u32) -> Option<LinuxProcessStat> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    linux_process_stat_from_line(&stat)
}

#[cfg(target_os = "linux")]
fn linux_process_stat_from_line(line: &str) -> Option<LinuxProcessStat> {
    let open = line.find('(')?;
    let close = line.rfind(") ")?;
    let pid = line[..open].trim().parse().ok()?;
    let fields = line[close + 2..].split_whitespace().collect::<Vec<_>>();

    Some(LinuxProcessStat {
        pid,
        parent_pid: fields.get(1)?.parse().ok()?,
        process_group_id: fields.get(2)?.parse().ok()?,
        tty_device: fields.get(4)?.parse().ok()?,
        tty_process_group_id: fields.get(5)?.parse().ok()?,
    })
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

    strings_from_nul_separated_bytes(&contents)
}

#[cfg(target_os = "linux")]
fn selected_env_for_pid(pid: u32) -> Vec<(String, String)> {
    let path = format!("/proc/{pid}/environ");
    let Ok(contents) = fs::read(path) else {
        return Vec::new();
    };

    selected_env_from_nul_separated_bytes(&contents)
}

#[cfg(target_os = "linux")]
fn command_from_comm(pid: u32) -> Option<String> {
    let path = format!("/proc/{pid}/comm");
    let raw = fs::read_to_string(path).ok()?;
    command_basename(raw.trim())
}

#[cfg(target_os = "macos")]
fn process_evidence_for_pid(pid: u32) -> Option<ProcessEvidence> {
    let process = macos_process_info_for_pid(pid)?;
    process_evidence_from_macos_info(process)
}

#[cfg(target_os = "macos")]
#[derive(Clone, Debug)]
struct MacProcessInfo {
    pid: u32,
    parent_pid: u32,
    process_group_id: u32,
    tty_device: u32,
    tty_process_group_id: u32,
    command: String,
}

#[cfg(target_os = "macos")]
impl MacProcessInfo {
    fn is_foreground_on_tty(&self, tty_device: u32) -> bool {
        process_is_foreground_on_tty(
            u64::from(tty_device),
            u64::from(self.tty_device),
            i64::from(self.process_group_id),
            i64::from(self.tty_process_group_id),
        )
    }
}

#[cfg(target_os = "macos")]
fn process_evidence_from_macos_info(process: MacProcessInfo) -> Option<ProcessEvidence> {
    let procargs = macos_procargs2_for_pid(process.pid);
    let command = command_basename(&process.command).or_else(|| {
        procargs
            .argv
            .first()
            .and_then(|argv0| command_basename(argv0))
    })?;

    Some(ProcessEvidence {
        pid: process.pid,
        command,
        argv: procargs.argv,
        env: procargs.env,
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
        parent_pid: bsd.pbi_ppid,
        process_group_id: bsd.pbi_pgid,
        tty_device: bsd.e_tdev,
        tty_process_group_id: bsd.e_tpgid,
        command: c_char_array_to_string(&bsd.pbi_comm),
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
#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct MacProcArgs {
    argv: Vec<String>,
    env: Vec<(String, String)>,
}

#[cfg(target_os = "macos")]
fn macos_procargs2_for_pid(pid: u32) -> MacProcArgs {
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
        return MacProcArgs::default();
    }

    buffer.truncate(buffer_len);
    macos_procargs2_from_buffer(&buffer)
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
fn macos_procargs2_from_buffer(buffer: &[u8]) -> MacProcArgs {
    let argc_size = std::mem::size_of::<libc::c_int>();
    if buffer.len() < argc_size {
        return MacProcArgs::default();
    }

    let mut argc_bytes = [0_u8; std::mem::size_of::<libc::c_int>()];
    argc_bytes.copy_from_slice(&buffer[..argc_size]);
    let argc = libc::c_int::from_ne_bytes(argc_bytes);
    let Ok(argc) = usize::try_from(argc) else {
        return MacProcArgs::default();
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

    MacProcArgs {
        argv,
        env: selected_env_from_nul_separated_bytes(&buffer[offset..]),
    }
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

    let path = tty_path_from_pane_tty(pane_tty)?;
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

        let processes = ProcTableSnapshot::capture()
            .descendant_processes(child.id())
            .expect("collect process evidence");

        let _ = child.kill();
        let _ = child.wait();

        assert!(
            processes.iter().any(|process| process.pid == child.id()),
            "expected root process evidence, got {processes:?}"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_stat_parser_reads_foreground_tty_fields() {
        let stat = linux_process_stat_from_line(
            "1234 (agent) S 1 42 42 34820 42 4194304 0 0 0 0 0 0 0 0 20 0 1 0 1",
        )
        .expect("parse proc stat");

        assert_eq!(
            stat,
            LinuxProcessStat {
                pid: 1234,
                parent_pid: 1,
                process_group_id: 42,
                tty_device: 34820,
                tty_process_group_id: 42,
            }
        );
        assert!(stat.is_foreground_on_tty(34820));

        let background = LinuxProcessStat {
            process_group_id: 41,
            ..stat.clone()
        };
        assert!(!background.is_foreground_on_tty(34820));

        let other_tty = LinuxProcessStat {
            tty_device: 34821,
            ..stat
        };
        assert!(!other_tty.is_foreground_on_tty(34820));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_stat_parser_handles_command_names_with_parentheses() {
        let stat = linux_process_stat_from_line(
            "1234 (agent) worker) S 1 42 42 34820 42 4194304 0 0 0 0 0 0 0 0 20 0 1 0 1",
        )
        .expect("parse proc stat");

        assert_eq!(stat.pid, 1234);
        assert_eq!(stat.process_group_id, 42);
        assert_eq!(stat.tty_device, 34820);
        assert_eq!(stat.tty_process_group_id, 42);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_tty_device_id_rejects_missing_tty() {
        assert_eq!(linux_tty_device_id("not a tty"), None);
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
    fn macos_procargs2_parser_reads_arguments_and_selected_env() {
        let mut buffer = Vec::new();
        buffer.extend_from_slice(&(2 as libc::c_int).to_ne_bytes());
        buffer.extend_from_slice(b"/bin/zsh\0\0\0");
        buffer.extend_from_slice(
            b"zsh\0-l\0SHELL=/bin/zsh\0PI_CODING_AGENT=true\0OPENCODE_PID=123\0",
        );

        assert_eq!(
            macos_procargs2_from_buffer(&buffer),
            MacProcArgs {
                argv: vec!["zsh".to_string(), "-l".to_string()],
                env: vec![
                    ("PI_CODING_AGENT".to_string(), "true".to_string()),
                    ("OPENCODE_PID".to_string(), "123".to_string()),
                ],
            }
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_foreground_match_requires_tty_and_foreground_group() {
        let process = MacProcessInfo {
            pid: 123,
            parent_pid: 1,
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
    fn macos_process_evidence_does_not_require_env_visibility() {
        let mut child = Command::new("sleep")
            .arg("5")
            .env("PI_CODING_AGENT", "true")
            .spawn()
            .expect("spawn sleep process with env");

        let evidence = process_evidence_for_pid(child.id()).expect("process evidence");

        let _ = child.kill();
        let _ = child.wait();

        assert_eq!(evidence.pid, child.id());
        assert!(!evidence.command.trim().is_empty());
    }
}
