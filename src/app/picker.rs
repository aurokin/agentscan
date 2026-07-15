use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

use super::{DisplayMetadata, PaneLocation, PaneRecord, PaneStatus, Provider};

pub(crate) const DEFAULT_PICKER_SELECTION_KEYS: [char; 16] = [
    '1', '2', '3', '4', '5', 'Q', 'E', 'R', 'F', 'G', 'T', 'Z', 'X', 'C', 'V', 'B',
];

const RESERVED_PICKER_KEYS: [char; 2] = ['N', 'P'];
const WORKSPACE_CACHE_TTL: Duration = Duration::from_secs(2);

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum PickerGroupBy {
    #[default]
    Session,
    GitRepo,
    Cwd,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PickerWorkspaceSource {
    Session,
    GitRepo,
    Cwd,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct PickerWorkspace {
    pub(crate) id: String,
    pub(crate) label: String,
    pub(crate) source: PickerWorkspaceSource,
    #[serde(skip)]
    cache_root: Option<String>,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct PickerWorkspaceCacheKey {
    picker_group_by: PickerGroupBy,
    identity: String,
}

#[derive(Debug, Default)]
pub(crate) struct PickerWorkspaceCache {
    workspaces: HashMap<PickerWorkspaceCacheKey, CachedPickerWorkspace>,
}

#[derive(Debug)]
struct CachedPickerWorkspace {
    workspace: PickerWorkspace,
    expires_at: Instant,
}

impl PickerWorkspaceCache {
    pub(crate) fn workspace_for_pane(
        &mut self,
        pane: &PaneRecord,
        picker_group_by: PickerGroupBy,
    ) -> PickerWorkspace {
        let key = workspace_cache_key(pane, picker_group_by);
        let now = Instant::now();
        if let Some(cached) = self.workspaces.get(&key)
            && cached.expires_at > now
        {
            return cached.workspace.clone();
        }
        if let Some(workspace) = self.cached_git_repo_workspace(pane, picker_group_by, now) {
            return workspace;
        }

        let workspace = compute_workspace_for_pane(pane, picker_group_by);
        self.workspaces.insert(
            key.clone(),
            CachedPickerWorkspace {
                workspace: workspace.clone(),
                expires_at: now + WORKSPACE_CACHE_TTL,
            },
        );
        self.cache_git_repo_ancestors(pane, picker_group_by, &workspace, now);
        workspace
    }

    pub(crate) fn clear(&mut self) {
        self.workspaces.clear();
    }

    fn cached_git_repo_workspace(
        &self,
        pane: &PaneRecord,
        picker_group_by: PickerGroupBy,
        now: Instant,
    ) -> Option<PickerWorkspace> {
        if picker_group_by != PickerGroupBy::GitRepo {
            return None;
        }

        let cwd = effective_cwd(pane)?;
        for ancestor in Path::new(cwd).ancestors() {
            let cache_path = path_identity(ancestor);
            if let Some(workspace) = self
                .workspaces
                .get(&git_repo_ancestor_cache_key(&cache_path))
                .filter(|cached| cached.expires_at > now)
                .map(|cached| cached.workspace.clone())
            {
                return Some(workspace);
            }

            if ancestor.join(".git").exists() {
                return None;
            }
        }

        None
    }

    fn cache_git_repo_ancestors(
        &mut self,
        pane: &PaneRecord,
        picker_group_by: PickerGroupBy,
        workspace: &PickerWorkspace,
        now: Instant,
    ) {
        if picker_group_by != PickerGroupBy::GitRepo
            || workspace.source != PickerWorkspaceSource::GitRepo
        {
            return;
        }

        let Some(cwd) = effective_cwd(pane) else {
            return;
        };
        let Some(cache_root) = workspace.cache_root.as_deref() else {
            return;
        };

        for cache_path in git_repo_cache_paths(cwd, cache_root) {
            self.workspaces.insert(
                git_repo_ancestor_cache_key(&cache_path),
                CachedPickerWorkspace {
                    workspace: workspace.clone(),
                    expires_at: now + WORKSPACE_CACHE_TTL,
                },
            );
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PickerKeySet {
    keys: Vec<char>,
}

impl PickerKeySet {
    pub(crate) fn from_config_values(values: &[String]) -> Result<Self> {
        let mut keys = Vec::with_capacity(values.len());
        for value in values {
            let key = normalize_config_picker_key(value)?;
            if RESERVED_PICKER_KEYS.contains(&key) {
                bail!("picker key {value:?} is reserved for TUI paging");
            }
            if keys.contains(&key) {
                bail!("picker key {value:?} duplicates another configured key");
            }
            keys.push(key);
        }

        if keys.len() != DEFAULT_PICKER_SELECTION_KEYS.len() {
            bail!(
                "picker_keys must contain exactly {} keys",
                DEFAULT_PICKER_SELECTION_KEYS.len()
            );
        }

        Ok(Self { keys })
    }

    pub(crate) fn keys(&self) -> &[char] {
        &self.keys
    }

    pub(crate) fn len(&self) -> usize {
        self.keys.len()
    }

    fn contains(&self, key: char) -> bool {
        self.keys.contains(&key)
    }

    fn key_list(&self) -> String {
        self.keys
            .iter()
            .map(char::to_string)
            .collect::<Vec<_>>()
            .join(" ")
    }
}

impl Default for PickerKeySet {
    fn default() -> Self {
        Self {
            keys: DEFAULT_PICKER_SELECTION_KEYS.to_vec(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct PickerRow {
    pub(crate) key: char,
    pub(crate) pane_id: String,
    pub(crate) provider: Option<Provider>,
    pub(crate) status: PaneStatus,
    pub(crate) display: DisplayMetadata,
    pub(crate) display_label: String,
    pub(crate) location: PaneLocation,
    pub(crate) location_tag: String,
    pub(crate) workspace: PickerWorkspace,
    /// Whether this pane is the active pane of its window (`pane_active &&
    /// window_active`). True for one pane per session, so clients should prefer
    /// `is_focused` to mark the single live pane.
    pub(crate) is_active: bool,
    /// Whether this is the single pane the user is currently focused on: the
    /// active pane of the session the most-recently-active tmux client is viewing.
    /// At most one row is `is_focused`; all are `false` when nothing is attached.
    pub(crate) is_focused: bool,
    /// Number of clients attached to the tmux server. A server-level fact echoed
    /// on every row (the picker output is a flat array, so there is no envelope to
    /// carry it once); `>1` signals best-effort focus and a multiple-clients hint.
    pub(crate) attached_client_count: u32,
    /// Focus-recency ordinal copied from the pane (see
    /// `PaneRecord::last_focus_seq`): higher = more recently focused through
    /// an agentscan focus action, valid only within one daemon session and
    /// one source. Absent means "no signal".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) last_focus_seq: Option<u64>,
}

pub(crate) fn picker_rows(
    panes: &[PaneRecord],
    focused_session: Option<&str>,
    attached_client_count: u32,
    picker_group_by: PickerGroupBy,
    picker_keys: &PickerKeySet,
) -> Vec<PickerRow> {
    let mut workspace_cache = PickerWorkspaceCache::default();
    ordered_panes(panes, picker_group_by, &mut workspace_cache)
        .into_iter()
        .zip(picker_keys.keys().iter().copied())
        .map(|(pane, key)| {
            let is_active = pane.is_active();
            let workspace = workspace_cache.workspace_for_pane(pane, picker_group_by);
            PickerRow {
                key,
                pane_id: pane.pane_id.clone(),
                provider: pane.provider,
                status: pane.status.clone(),
                display: pane.display.clone(),
                display_label: pane.display.label.clone(),
                location: pane.location.clone(),
                location_tag: pane.location.tag(),
                workspace,
                is_active,
                // The focused pane is the active pane of the focused session, so
                // require both signals — that yields exactly one row.
                is_focused: is_active
                    && focused_session.is_some_and(|session| session == pane.location.session_name),
                attached_client_count,
                last_focus_seq: pane.last_focus_seq,
            }
        })
        .collect()
}

pub(crate) fn sort_panes_for_picker_with_cache(
    panes: &mut [PaneRecord],
    picker_group_by: PickerGroupBy,
    workspace_cache: &mut PickerWorkspaceCache,
) {
    if picker_group_by == PickerGroupBy::Session {
        return;
    }

    panes.sort_by_cached_key(|pane| picker_sort_key(pane, picker_group_by, workspace_cache));
}

#[cfg(test)]
pub(crate) fn workspace_for_pane(
    pane: &PaneRecord,
    picker_group_by: PickerGroupBy,
) -> PickerWorkspace {
    compute_workspace_for_pane(pane, picker_group_by)
}

fn compute_workspace_for_pane(
    pane: &PaneRecord,
    picker_group_by: PickerGroupBy,
) -> PickerWorkspace {
    match picker_group_by {
        PickerGroupBy::Session => session_workspace(pane),
        PickerGroupBy::Cwd => cwd_workspace(pane),
        PickerGroupBy::GitRepo => git_repo_workspace(pane),
    }
}

fn ordered_panes<'a>(
    panes: &'a [PaneRecord],
    picker_group_by: PickerGroupBy,
    workspace_cache: &mut PickerWorkspaceCache,
) -> Vec<&'a PaneRecord> {
    let mut ordered = panes.iter().collect::<Vec<_>>();
    if picker_group_by == PickerGroupBy::Session {
        return ordered;
    }

    ordered.sort_by_cached_key(|pane| picker_sort_key(pane, picker_group_by, workspace_cache));
    ordered
}

fn picker_sort_key(
    pane: &PaneRecord,
    picker_group_by: PickerGroupBy,
    workspace_cache: &mut PickerWorkspaceCache,
) -> (String, String, String, u32, u32, String) {
    let workspace = workspace_cache.workspace_for_pane(pane, picker_group_by);
    let (group_label, group_id) = match picker_group_by {
        PickerGroupBy::Session => (pane.location.session_name.clone(), workspace.id),
        PickerGroupBy::GitRepo | PickerGroupBy::Cwd => (workspace.label, workspace.id),
    };

    (
        group_label,
        group_id,
        pane.location.session_name.clone(),
        pane.location.window_index,
        pane.location.pane_index,
        pane.pane_id.clone(),
    )
}

fn workspace_cache_key(
    pane: &PaneRecord,
    picker_group_by: PickerGroupBy,
) -> PickerWorkspaceCacheKey {
    let identity = match picker_group_by {
        PickerGroupBy::Session => format!("session:{}", pane.location.session_name),
        PickerGroupBy::GitRepo | PickerGroupBy::Cwd => effective_cwd(pane)
            .map(|cwd| format!("cwd:{cwd}\nsession:{}", pane.location.session_name))
            .unwrap_or_else(|| format!("session:{}", pane.location.session_name)),
    };

    PickerWorkspaceCacheKey {
        picker_group_by,
        identity,
    }
}

fn git_repo_ancestor_cache_key(path: &str) -> PickerWorkspaceCacheKey {
    PickerWorkspaceCacheKey {
        picker_group_by: PickerGroupBy::GitRepo,
        identity: format!("git-ancestor:{path}"),
    }
}

fn git_repo_cache_paths(cwd: &str, cache_root: &str) -> Vec<String> {
    let cache_root = Path::new(cache_root);
    let mut paths = Vec::new();
    for ancestor in Path::new(cwd).ancestors() {
        paths.push(path_identity(ancestor));
        if ancestor == cache_root {
            return paths;
        }
    }

    Vec::new()
}

fn session_workspace(pane: &PaneRecord) -> PickerWorkspace {
    let label = fallback_session_label(pane);
    PickerWorkspace {
        id: workspace_id(PickerWorkspaceSource::Session, &pane.location.session_name),
        label,
        source: PickerWorkspaceSource::Session,
        cache_root: None,
    }
}

fn cwd_workspace(pane: &PaneRecord) -> PickerWorkspace {
    effective_cwd(pane)
        .and_then(cwd_workspace_for_path)
        .unwrap_or_else(|| session_workspace(pane))
}

fn git_repo_workspace(pane: &PaneRecord) -> PickerWorkspace {
    if let Some(workspace) = effective_cwd(pane).and_then(git_repo_workspace_for_path) {
        return workspace;
    }

    cwd_workspace(pane)
}

fn cwd_workspace_for_path(cwd: &str) -> Option<PickerWorkspace> {
    let path = Path::new(cwd);
    let label = basename_path(path)?;
    let identity = path_identity(path);
    Some(PickerWorkspace {
        id: workspace_id(PickerWorkspaceSource::Cwd, &identity),
        label,
        source: PickerWorkspaceSource::Cwd,
        cache_root: None,
    })
}

fn effective_cwd(pane: &PaneRecord) -> Option<&str> {
    pane.agent_metadata
        .cwd
        .as_deref()
        .map(str::trim)
        .filter(|cwd| !cwd.is_empty())
        .or_else(|| {
            let cwd = pane.tmux.pane_current_path.trim();
            (!cwd.is_empty()).then_some(cwd)
        })
}

fn git_repo_workspace_for_path(path: &str) -> Option<PickerWorkspace> {
    let mut current = Path::new(path);
    if current.as_os_str().is_empty() {
        return None;
    }

    loop {
        let git_path = current.join(".git");
        if git_path.is_dir() {
            return git_repo_workspace_from_path(current, false);
        }
        if git_path.is_file() {
            return git_common_dir_workspace(&git_path)
                .or_else(|| git_repo_workspace_from_path(current, false));
        }
        current = current.parent()?;
    }
}

fn git_common_dir_workspace(git_file: &Path) -> Option<PickerWorkspace> {
    let git_file_parent = git_file.parent()?;
    let contents = fs::read_to_string(git_file).ok()?;
    let gitdir = contents.trim().strip_prefix("gitdir:")?.trim();
    let gitdir = resolve_path(gitdir, git_file_parent);
    if !gitdir_is_linked_worktree(&gitdir) {
        return None;
    }
    let commondir = fs::read_to_string(gitdir.join("commondir")).ok()?;
    let common_dir = resolve_path(commondir.trim(), &gitdir);

    let mut workspace = if common_dir.file_name().and_then(|name| name.to_str()) == Some(".git") {
        common_dir
            .parent()
            .and_then(|repo_path| git_repo_workspace_from_path(repo_path, false))
    } else {
        git_repo_workspace_from_path(&common_dir, true)
    }?;
    workspace.cache_root = Some(path_identity(git_file_parent));
    Some(workspace)
}

fn gitdir_is_linked_worktree(gitdir: &Path) -> bool {
    gitdir
        .parent()
        .and_then(|parent| parent.file_name())
        .and_then(|name| name.to_str())
        == Some("worktrees")
}

fn git_repo_workspace_from_path(
    repo_path: &Path,
    strip_dot_git_suffix: bool,
) -> Option<PickerWorkspace> {
    let label = git_repo_label_from_path(repo_path, strip_dot_git_suffix)?;
    let identity = path_identity(repo_path);
    Some(PickerWorkspace {
        id: workspace_id(PickerWorkspaceSource::GitRepo, &identity),
        label,
        source: PickerWorkspaceSource::GitRepo,
        cache_root: Some(identity),
    })
}

fn git_repo_label_from_path(path: &Path, strip_dot_git_suffix: bool) -> Option<String> {
    let label = basename_path(path)?;
    if strip_dot_git_suffix {
        let stripped = label
            .strip_suffix(".git")
            .map(str::trim)
            .filter(|name| !name.is_empty());
        if let Some(label) = stripped {
            return Some(label.to_string());
        }
    }
    Some(label)
}

fn resolve_path(path: &str, base: &Path) -> PathBuf {
    let path = Path::new(path);
    let resolved = if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    };

    fs::canonicalize(&resolved).unwrap_or(resolved)
}

fn path_identity(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn basename_path(path: &Path) -> Option<String> {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(ToOwned::to_owned)
}

fn workspace_id(source: PickerWorkspaceSource, identity: &str) -> String {
    let prefix = match source {
        PickerWorkspaceSource::Session => "session",
        PickerWorkspaceSource::GitRepo => "git-repo",
        PickerWorkspaceSource::Cwd => "cwd",
    };
    format!("{prefix}:{identity}")
}

fn fallback_session_label(pane: &PaneRecord) -> String {
    let session = pane.location.session_name.trim();
    if session.is_empty() {
        "ungrouped".to_string()
    } else {
        session.to_string()
    }
}

pub(crate) fn normalize_picker_key(raw_key: &str, picker_keys: &PickerKeySet) -> Result<char> {
    let trimmed = raw_key.trim();
    let mut characters = trimmed.chars();
    let Some(character) = characters.next() else {
        bail!("hotkey must be one of {}", picker_key_list(picker_keys));
    };
    if characters.next().is_some() {
        bail!(
            "hotkey {raw_key:?} must be a single key from {}",
            picker_key_list(picker_keys)
        );
    }

    let normalized = character.to_ascii_uppercase();
    if !picker_keys.contains(normalized) {
        bail!(
            "hotkey {raw_key:?} is not supported; expected one of {}",
            picker_key_list(picker_keys)
        );
    }

    Ok(normalized)
}

pub(crate) fn picker_key_list(picker_keys: &PickerKeySet) -> String {
    picker_keys.key_list()
}

fn normalize_config_picker_key(raw_key: &str) -> Result<char> {
    let mut characters = raw_key.chars();
    let Some(character) = characters.next() else {
        bail!("picker_keys entries must be single ASCII letters or digits");
    };
    if characters.next().is_some() {
        bail!("picker key {raw_key:?} must be a single character");
    }
    if !character.is_ascii_alphanumeric() {
        bail!("picker key {raw_key:?} must be an ASCII letter or digit");
    }

    Ok(character.to_ascii_uppercase())
}
