use std::path::Path;

pub(crate) fn command_basename(raw: &str) -> Option<String> {
    Path::new(raw.trim())
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.trim().is_empty())
        .map(str::to_string)
}
