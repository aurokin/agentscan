use super::*;

pub(crate) trait ProcessInspector {
    fn descendant_commands(&self, root_pid: u32) -> Result<Vec<String>>;
}

#[derive(Default)]
pub(crate) struct ProcProcessInspector;

impl ProcessInspector for ProcProcessInspector {
    fn descendant_commands(&self, root_pid: u32) -> Result<Vec<String>> {
        descendant_commands(root_pid)
    }
}

#[cfg(target_os = "linux")]
fn descendant_commands(root_pid: u32) -> Result<Vec<String>> {
    const MAX_PROCESSES: usize = 64;

    let mut commands = Vec::new();
    let mut queue = children_for_pid(root_pid)?;
    let mut visited = std::collections::HashSet::new();

    while let Some(pid) = queue.pop() {
        if !visited.insert(pid) || visited.len() > MAX_PROCESSES {
            continue;
        }

        if let Some(command) = command_for_pid(pid) {
            commands.push(command);
        }

        queue.extend(children_for_pid(pid)?);
    }

    Ok(commands)
}

#[cfg(not(target_os = "linux"))]
fn descendant_commands(_root_pid: u32) -> Result<Vec<String>> {
    Ok(Vec::new())
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
fn command_for_pid(pid: u32) -> Option<String> {
    command_from_cmdline(pid).or_else(|| command_from_comm(pid))
}

#[cfg(target_os = "linux")]
fn command_from_cmdline(pid: u32) -> Option<String> {
    let path = format!("/proc/{pid}/cmdline");
    let contents = fs::read(path).ok()?;
    let first = contents
        .split(|byte| *byte == 0)
        .find(|part| !part.is_empty())?;
    let raw = std::str::from_utf8(first).ok()?.trim();
    command_basename(raw)
}

#[cfg(target_os = "linux")]
fn command_from_comm(pid: u32) -> Option<String> {
    let path = format!("/proc/{pid}/comm");
    let raw = fs::read_to_string(path).ok()?;
    command_basename(raw.trim())
}

#[cfg(target_os = "linux")]
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
