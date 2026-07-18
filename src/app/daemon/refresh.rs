use super::*;
use std::collections::{BTreeSet, HashMap, HashSet};

pub(super) fn apply_control_event_batch(
    snapshot: &mut SnapshotEnvelope,
    tmux_reads: &mut impl TmuxReadProvider,
    batch: &ControlEventBatch,
    pane_output_cache: &mut scanner::PaneOutputStatusCache,
    disable_proc_fallback: bool,
) -> Result<ControlEventOutcome> {
    let mut refresh_context =
        RefreshContext::new(tmux_reads, pane_output_cache, disable_proc_fallback);
    let pane_scopes_before_refresh = pane_scopes_by_id(snapshot, batch);
    let mut changed = false;
    let mut fallback_to_full = false;
    let mut full_snapshot_refresh = false;
    let mut targeted_title_updates = 0_u64;
    let mut targeted_pane_refreshes = 0_u64;
    let mut targeted_scope_refreshes = 0_u64;

    if batch.resnapshot_sequence.is_some() {
        let tmux_version = snapshot.source.tmux_version.clone();
        refresh_context.reconcile_full_snapshot(snapshot, tmux_version.as_deref())?;
        changed = true;
        full_snapshot_refresh = true;
    }

    for (session_id, sequence) in &batch.sessions {
        if batch
            .resnapshot_sequence
            .is_some_and(|resnapshot_sequence| *sequence <= resnapshot_sequence)
        {
            continue;
        }
        changed = true;
        targeted_scope_refreshes = targeted_scope_refreshes.saturating_add(1);
        if let Err(error) = refresh_context.refresh_session(snapshot, session_id) {
            refresh_context.fallback_to_full_resnapshot(
                snapshot,
                &format!("session:{session_id}"),
                error,
            )?;
            fallback_to_full = true;
            full_snapshot_refresh = true;
        }
    }

    for (window_id, sequence) in &batch.windows {
        if batch
            .resnapshot_sequence
            .is_some_and(|resnapshot_sequence| *sequence <= resnapshot_sequence)
        {
            continue;
        }
        changed = true;
        targeted_scope_refreshes = targeted_scope_refreshes.saturating_add(1);
        if let Err(error) = refresh_context.refresh_window(snapshot, window_id) {
            refresh_context.fallback_to_full_resnapshot(
                snapshot,
                &format!("window:{window_id}"),
                error,
            )?;
            fallback_to_full = true;
            full_snapshot_refresh = true;
        }
    }

    let pane_scopes_after_scope_refresh = pane_scopes_by_id(snapshot, batch);
    let activity_panes = batch
        .activities
        .keys()
        .filter(|pane_id| {
            snapshot
                .panes
                .iter()
                .find(|pane| pane.pane_id == pane_id.as_str())
                .is_some_and(classify::pane_output_status_activity_candidate)
        })
        .cloned()
        .collect::<HashSet<_>>();
    let mut pane_ids = batch.panes.keys().cloned().collect::<BTreeSet<_>>();
    pane_ids.extend(activity_panes.iter().cloned());
    for pane_id in &pane_ids {
        let title_override = title_override_after_latest_refresh(
            batch,
            &pane_scopes_before_refresh,
            &pane_scopes_after_scope_refresh,
            pane_id,
            activity_panes.contains(pane_id),
        );
        let has_title_override = title_override.is_some();
        if refresh_context.refresh_pane_with_title(snapshot, pane_id, title_override)? {
            changed = true;
            targeted_pane_refreshes = targeted_pane_refreshes.saturating_add(1);
            if has_title_override {
                targeted_title_updates = targeted_title_updates.saturating_add(1);
            }
        }
    }

    for pane_id in batch.titles.keys() {
        let Some(title) = title_override_after_latest_refresh(
            batch,
            &pane_scopes_before_refresh,
            &pane_scopes_after_scope_refresh,
            pane_id,
            activity_panes.contains(pane_id),
        ) else {
            continue;
        };
        if batch.panes.contains_key(pane_id) {
            continue;
        }
        if activity_panes.contains(pane_id) {
            continue;
        }
        if refresh_context.refresh_pane_with_title(snapshot, pane_id, Some(title))? {
            changed = true;
            targeted_pane_refreshes = targeted_pane_refreshes.saturating_add(1);
            targeted_title_updates = targeted_title_updates.saturating_add(1);
        }
    }

    // Sort/mark exactly once for the whole batch: every scope and pane refresh above ran
    // its no-finalize variant, so a K-pane batch pays one full sort + mark instead of K.
    if changed {
        finalize_snapshot(snapshot)?;
    }

    Ok(ControlEventOutcome {
        changed,
        fallback_to_full,
        full_snapshot_refresh,
        targeted_title_updates,
        targeted_pane_refreshes,
        targeted_scope_refreshes,
    })
}

// Map only the panes that carry a title event in this batch to their (session, window)
// scope. `title_override_after_latest_refresh` short-circuits on panes without a title
// event and never consults the map for them, so cloning every pane's scope Strings is
// wasted work; when the batch carries no titles this returns an empty map with no clones.
fn pane_scopes_by_id(
    snapshot: &SnapshotEnvelope,
    batch: &ControlEventBatch,
) -> HashMap<String, (Option<String>, Option<String>)> {
    if batch.titles.is_empty() {
        return HashMap::new();
    }
    snapshot
        .panes
        .iter()
        .filter(|pane| batch.titles.contains_key(pane.pane_id.as_str()))
        .map(|pane| {
            (
                pane.pane_id.clone(),
                (pane.tmux.session_id.clone(), pane.tmux.window_id.clone()),
            )
        })
        .collect()
}

fn title_override_after_latest_refresh<'a>(
    batch: &'a ControlEventBatch,
    pane_scopes_before_refresh: &HashMap<String, (Option<String>, Option<String>)>,
    pane_scopes_after_scope_refresh: &HashMap<String, (Option<String>, Option<String>)>,
    pane_id: &str,
    activity_refresh: bool,
) -> Option<&'a str> {
    let title = batch.titles.get(pane_id)?;
    let mut latest_refresh_sequence = batch
        .resnapshot_sequence
        .into_iter()
        .chain(batch.panes.get(pane_id).copied())
        .chain(
            activity_refresh
                .then(|| batch.activities.get(pane_id).copied())
                .flatten(),
        )
        .max();

    for pane_scopes in [
        pane_scopes_before_refresh.get(pane_id),
        pane_scopes_after_scope_refresh.get(pane_id),
    ]
    .into_iter()
    .flatten()
    {
        latest_refresh_sequence =
            latest_refresh_sequence_for_scopes(batch, pane_scopes, latest_refresh_sequence);
    }

    latest_refresh_sequence
        .is_none_or(|latest_refresh_sequence| title.sequence > latest_refresh_sequence)
        .then_some(title.title.as_str())
}

fn latest_refresh_sequence_for_scopes(
    batch: &ControlEventBatch,
    pane_scopes: &(Option<String>, Option<String>),
    latest_refresh_sequence: Option<u64>,
) -> Option<u64> {
    let mut latest_refresh_sequence = latest_refresh_sequence;
    if let Some(sequence) = pane_scopes
        .0
        .as_deref()
        .and_then(|session_id| batch.sessions.get(session_id))
    {
        latest_refresh_sequence = Some(
            latest_refresh_sequence
                .map(|latest| latest.max(*sequence))
                .unwrap_or(*sequence),
        );
    }
    if let Some(sequence) = pane_scopes
        .1
        .as_deref()
        .and_then(|window_id| batch.windows.get(window_id))
    {
        latest_refresh_sequence = Some(
            latest_refresh_sequence
                .map(|latest| latest.max(*sequence))
                .unwrap_or(*sequence),
        );
    }
    latest_refresh_sequence
}

// Shared borrow target for per-pass lazy process snapshots; the inspector is a
// stateless unit, so a `'static` borrow keeps `RefreshContext` lifetime-free.
static PROC_INSPECTOR: proc::ProcProcessInspector = proc::ProcProcessInspector;

struct RefreshContext<'a, TmuxReads> {
    tmux_reads: &'a mut TmuxReads,
    pane_output_cache: &'a mut scanner::PaneOutputStatusCache,
    disable_proc_fallback: bool,
    // One lazily-captured process table shared by every targeted refresh in
    // the batch: K unresolved candidate panes cost one capture, not K.
    proc_snapshot: proc::LazyProcessSnapshot<'static, proc::ProcProcessInspector>,
}

impl<'a, TmuxReads: TmuxReadProvider> RefreshContext<'a, TmuxReads> {
    fn new(
        tmux_reads: &'a mut TmuxReads,
        pane_output_cache: &'a mut scanner::PaneOutputStatusCache,
        disable_proc_fallback: bool,
    ) -> Self {
        Self {
            tmux_reads,
            pane_output_cache,
            disable_proc_fallback,
            proc_snapshot: proc::LazyProcessSnapshot::new(&PROC_INSPECTOR),
        }
    }

    fn refresh_pane_with_title(
        &mut self,
        snapshot: &mut SnapshotEnvelope,
        pane_id: &str,
        title_override: Option<&str>,
    ) -> Result<bool> {
        refresh_snapshot_pane_with_title_no_finalize(
            snapshot,
            self.tmux_reads,
            pane_id,
            title_override,
            self.pane_output_cache,
            &self.proc_snapshot,
            self.disable_proc_fallback,
        )
    }

    fn refresh_window(&mut self, snapshot: &mut SnapshotEnvelope, window_id: &str) -> Result<()> {
        refresh_snapshot_scope_no_finalize(
            snapshot,
            self.tmux_reads,
            TargetScope::Window,
            window_id,
            self.pane_output_cache,
            &self.proc_snapshot,
            self.disable_proc_fallback,
        )
    }

    fn refresh_session(&mut self, snapshot: &mut SnapshotEnvelope, session_id: &str) -> Result<()> {
        refresh_snapshot_scope_no_finalize(
            snapshot,
            self.tmux_reads,
            TargetScope::Session,
            session_id,
            self.pane_output_cache,
            &self.proc_snapshot,
            self.disable_proc_fallback,
        )
    }

    fn fallback_to_full_resnapshot(
        &mut self,
        snapshot: &mut SnapshotEnvelope,
        event_context: &str,
        error: anyhow::Error,
    ) -> Result<()> {
        eprintln!(
            "agentscan: targeted refresh failed for control-mode event {event_context:?}: {error:#}"
        );
        let tmux_version = snapshot.source.tmux_version.clone();
        self.reconcile_full_snapshot(snapshot, tmux_version.as_deref())
    }

    fn reconcile_full_snapshot(
        &mut self,
        snapshot: &mut SnapshotEnvelope,
        tmux_version: Option<&str>,
    ) -> Result<()> {
        reconcile_full_snapshot(
            snapshot,
            self.tmux_reads,
            tmux_version,
            self.pane_output_cache,
            self.disable_proc_fallback,
        )
    }
}

pub(super) fn refresh_snapshot_pane_with_title(
    snapshot: &mut SnapshotEnvelope,
    tmux_reads: &mut impl TmuxReadProvider,
    pane_id: &str,
    title_override: Option<&str>,
    pane_output_cache: &mut scanner::PaneOutputStatusCache,
    proc_snapshot: &impl proc::ProcessSnapshot,
    disable_proc_fallback: bool,
) -> Result<bool> {
    let changed = refresh_snapshot_pane_with_title_no_finalize(
        snapshot,
        tmux_reads,
        pane_id,
        title_override,
        pane_output_cache,
        proc_snapshot,
        disable_proc_fallback,
    )?;
    if changed {
        finalize_snapshot(snapshot)?;
    }
    Ok(changed)
}

// Apply a single targeted pane refresh without sorting/marking the snapshot. Callers
// that touch several panes in one pass (control-event batch, settle recapture) finalize
// once at the end via `finalize_snapshot` instead of paying a full sort + mark per pane.
pub(super) fn refresh_snapshot_pane_with_title_no_finalize(
    snapshot: &mut SnapshotEnvelope,
    tmux_reads: &mut impl TmuxReadProvider,
    pane_id: &str,
    title_override: Option<&str>,
    pane_output_cache: &mut scanner::PaneOutputStatusCache,
    proc_snapshot: &impl proc::ProcessSnapshot,
    disable_proc_fallback: bool,
) -> Result<bool> {
    let previous = snapshot
        .panes
        .iter()
        .find(|existing| existing.pane_id == pane_id)
        .cloned();
    let allow_title_change_for_identity = title_override.is_some();
    let pane = tmux_reads.list_pane(pane_id)?.map(|mut row| {
        if let Some(title) = title_override {
            row.pane_title_raw = title.to_string();
        }
        let mut pane = pane_from_targeted_row_preserving_provider_identity(
            row,
            previous.as_ref(),
            allow_title_change_for_identity,
            proc_snapshot,
            disable_proc_fallback,
        );
        scanner::apply_pane_output_status_fallbacks_with_cache(
            std::slice::from_mut(&mut pane),
            pane_output_cache,
            Instant::now(),
        );
        pane.diagnostics.cache_origin = "daemon_update".to_string();
        pane
    });

    if let Some(index) = snapshot
        .panes
        .iter()
        .position(|existing| existing.pane_id == pane_id)
    {
        if let Some(pane) = pane {
            snapshot.panes[index] = pane;
        } else {
            snapshot.panes.remove(index);
        }
    } else if pane.is_none() {
        return Ok(false);
    } else if let Some(pane) = pane {
        snapshot.panes.push(pane);
    }

    Ok(true)
}

fn finalize_snapshot(snapshot: &mut SnapshotEnvelope) -> Result<()> {
    snapshot::sort_snapshot_panes(snapshot);
    snapshot::mark_snapshot_as_daemon(snapshot)
}

// Carry the previous pane's identity (provider, how it was classified, and the
// proc-fallback diagnostics) onto the freshly built pane. Status and display are
// deliberately left as the fresh row computed them, so a preserved agent still
// reflects its new title (e.g. idle -> busy) while keeping its provider.
fn preserve_provider_identity_for_targeted_update(pane: &mut PaneRecord, previous: &PaneRecord) {
    pane.provider = previous.provider;
    pane.classification = previous.classification.clone();
    pane.diagnostics.proc_fallback = previous.diagnostics.proc_fallback.clone();
}

fn pane_from_targeted_row_preserving_provider_identity(
    row: TmuxPaneRow,
    previous: Option<&PaneRecord>,
    allow_title_change_for_identity: bool,
    proc_snapshot: &impl proc::ProcessSnapshot,
    disable_proc_fallback: bool,
) -> PaneRecord {
    let fresh_agent_metadata = classify::trusted_agent_metadata(&row, Some(proc_snapshot));
    let should_preserve = previous.is_some_and(|previous| {
        should_preserve_provider_identity_for_targeted_update(
            previous,
            &row,
            &fresh_agent_metadata,
            allow_title_change_for_identity,
        )
    });
    let mut classification_metadata = fresh_agent_metadata.clone();
    if should_preserve {
        classification_metadata.provider = previous
            .and_then(|previous| previous.provider)
            .map(|provider| provider.to_string());
    }

    let mut pane = classify::pane_from_row_with_agent_metadata(row, classification_metadata);
    if should_preserve && let Some(previous) = previous {
        pane.agent_metadata = fresh_agent_metadata;
        preserve_provider_identity_for_targeted_update(&mut pane, previous);
    }
    recover_targeted_pane_provider(&mut pane, proc_snapshot, disable_proc_fallback);
    pane
}

// The targeted (event) path normally avoids process inspection, but some agents
// cannot be identified from tmux metadata at all — notably Claude Code, whose
// `pane_current_command` is its version string and whose title is the current task
// rather than "Claude Code". Such a pane, when freshly built here, has no provider
// and (because nothing carried one forward) would stay invisible until the next
// full snapshot — up to the self-heal interval away under the default
// `disable_reconcile = true`. Run the bounded single-pane proc fallback for exactly
// these cases to recover identity from the process tree, which finds `claude` even
// when the foreground briefly flips to a tool subprocess.
//
// This is self-limiting: `apply_proc_fallback_with_options` only inspects panes that
// `is_proc_fallback_candidate` accepts (no provider yet, and an agent-shaped command
// or title — version-like command, spinner/idle glyph, or shell/launcher), so plain
// panes cost nothing, and once a pane resolves it is no longer a candidate. It does
// revisit the "targeted refreshes avoid process inspection" stance, but only for the
// ambiguous-agent panes that the metadata-only path cannot otherwise see.
fn recover_targeted_pane_provider(
    pane: &mut PaneRecord,
    proc_snapshot: &impl proc::ProcessSnapshot,
    disable_proc_fallback: bool,
) {
    // Only ambiguous panes that the metadata path could not identify. A pane that
    // already has a provider (fresh classification or a carried-forward identity)
    // is left untouched; `apply_proc_fallback_with_options` further self-gates to
    // agent-shaped candidates, so plain panes never trigger process inspection.
    if pane.provider.is_some() {
        return;
    }
    // The caller threads one lazily-captured process snapshot through the whole
    // refresh pass: non-candidate panes and disabled fallback never pay for a
    // capture, and every candidate in the pass shares a single one.
    classify::apply_proc_fallback_with_options(pane, proc_snapshot, disable_proc_fallback);
}

// Decide whether a targeted (title/pane) refresh should keep the pane's existing
// provider instead of the freshly classified one. A control-mode title update only
// changes the pane's title text; when a previously *process-tree-confirmed* agent's
// title changes (e.g. Claude Code's title becoming the current task), we keep its
// identity rather than re-running process inspection on every title tick.
//
// Restricted to `ProcFallbackOutcome::Resolved` identities on purpose: a provider
// that came only from the old title (or a stable wrapper command) must NOT be made
// sticky here, or a non-agent pane — or an agent that exited under a stable shell —
// would keep a stale provider after its title changes away from the provider name.
// Those panes instead fall through to `recover_targeted_pane_provider`,
// which consults the process tree and clears or corrects the match. The process
// tree is the source of truth; this preservation is only the cheap fast path for
// identities the process tree already confirmed.
//
// Preserve when: the previous identity was process-resolved and did not come from
// agent metadata/hooks (fresh metadata should win), the fresh row carries no agent
// metadata and resolves to no *different* provider, and the row still matches the
// previous tmux process identity (same pane_pid, command, path, and tty — only the
// title may differ). A genuine change fails these guards and fresh classification
// (then proc recovery) wins.
fn should_preserve_provider_identity_for_targeted_update(
    previous: &PaneRecord,
    row: &TmuxPaneRow,
    fresh_agent_metadata: &AgentMetadata,
    allow_title_change: bool,
) -> bool {
    previous.diagnostics.proc_fallback.outcome == ProcFallbackOutcome::Resolved
        && previous.provider.is_some()
        && previous.agent_metadata.provider.is_none()
        && fresh_agent_metadata.provider.is_none()
        && previous.tmux.pane_pid == row.pane_pid
        && {
            // Provider-only classification (no row clone, no full PaneRecord) instead
            // of `pane_from_row(row.clone()).provider`; the real classification still
            // runs once in `pane_from_targeted_row_preserving_provider_identity`.
            let fresh_provider = classify::provider_from_row(row, fresh_agent_metadata);
            fresh_provider == previous.provider
                || (fresh_provider.is_none()
                    && row_matches_previous_tmux_identity(previous, row, allow_title_change))
        }
}

fn row_matches_previous_tmux_identity(
    previous: &PaneRecord,
    row: &TmuxPaneRow,
    allow_title_change: bool,
) -> bool {
    previous.tmux.pane_current_command == row.pane_current_command
        && (allow_title_change || previous.tmux.pane_title_raw == row.pane_title_raw)
        && previous.tmux.pane_current_path == row.pane_current_path
        && previous.tmux.pane_tty == row.pane_tty
}

#[cfg(test)]
pub(super) fn refresh_snapshot_window(
    snapshot: &mut SnapshotEnvelope,
    tmux_reads: &mut impl TmuxReadProvider,
    window_id: &str,
    pane_output_cache: &mut scanner::PaneOutputStatusCache,
    disable_proc_fallback: bool,
) -> Result<()> {
    let proc_snapshot = proc::LazyProcessSnapshot::new(&PROC_INSPECTOR);
    refresh_snapshot_scope_no_finalize(
        snapshot,
        tmux_reads,
        TargetScope::Window,
        window_id,
        pane_output_cache,
        &proc_snapshot,
        disable_proc_fallback,
    )?;
    finalize_snapshot(snapshot)
}

pub(super) fn refresh_snapshot_session(
    snapshot: &mut SnapshotEnvelope,
    tmux_reads: &mut impl TmuxReadProvider,
    session_id: &str,
    pane_output_cache: &mut scanner::PaneOutputStatusCache,
    disable_proc_fallback: bool,
) -> Result<()> {
    let proc_snapshot = proc::LazyProcessSnapshot::new(&PROC_INSPECTOR);
    refresh_snapshot_scope_no_finalize(
        snapshot,
        tmux_reads,
        TargetScope::Session,
        session_id,
        pane_output_cache,
        &proc_snapshot,
        disable_proc_fallback,
    )?;
    finalize_snapshot(snapshot)
}

// A focus change flips `pane_active`/`window_active` within the focused pane's
// session only (each session tracks its own active window and per-window active
// pane, independent of other sessions), and it can move focus across windows in
// that session. Refreshing the whole session is the narrowest scope that keeps
// every affected active flag correct — including the previously-focused pane —
// instead of a full list-panes over every session on each rapid focus event. If
// the focused pane is not in the snapshot, fall back to a full reconcile.
pub(super) fn refresh_snapshot_for_focused_pane(
    snapshot: &mut SnapshotEnvelope,
    tmux_reads: &mut impl TmuxReadProvider,
    pane_id: &str,
    tmux_version: Option<&str>,
    pane_output_cache: &mut scanner::PaneOutputStatusCache,
    disable_proc_fallback: bool,
) -> Result<()> {
    let session_id = snapshot
        .panes
        .iter()
        .find(|pane| pane.pane_id == pane_id)
        .and_then(|pane| pane.tmux.session_id.clone());
    let Some(session_id) = session_id.as_deref() else {
        return reconcile_full_snapshot(
            snapshot,
            tmux_reads,
            tmux_version,
            pane_output_cache,
            disable_proc_fallback,
        );
    };
    refresh_snapshot_session(
        snapshot,
        tmux_reads,
        session_id,
        pane_output_cache,
        disable_proc_fallback,
    )?;
    // The session id came from our (possibly stale) snapshot. If the focused
    // pane moved to another session before this event was applied, the
    // old-session refresh just dropped it without re-adding it anywhere; only
    // a full reconcile rediscovers it.
    let focused_pane_missing = !snapshot.panes.iter().any(|pane| pane.pane_id == pane_id);
    if focused_pane_missing {
        reconcile_full_snapshot(
            snapshot,
            tmux_reads,
            tmux_version,
            pane_output_cache,
            disable_proc_fallback,
        )?;
    }
    Ok(())
}

// Refresh every pane in a session/window scope without sorting/marking the snapshot; the
// control-event batch finalizes once after all scope and pane refreshes are applied.
fn refresh_snapshot_scope_no_finalize(
    snapshot: &mut SnapshotEnvelope,
    tmux_reads: &mut impl TmuxReadProvider,
    scope: TargetScope,
    target_id: &str,
    pane_output_cache: &mut scanner::PaneOutputStatusCache,
    proc_snapshot: &impl proc::ProcessSnapshot,
    disable_proc_fallback: bool,
) -> Result<()> {
    let rows = tmux_reads.list_target_panes(scope.pane_list_scope(), target_id)?;
    let refreshed_pane_ids = rows
        .as_ref()
        .map(|rows| {
            rows.iter()
                .map(|row| row.pane_id.clone())
                .collect::<HashSet<_>>()
        })
        .unwrap_or_default();
    // Only the panes being rebuilt need their previous identity carried forward, so clone
    // just those rather than every pane in the snapshot (a one-window refresh otherwise
    // deep-clones the whole pane list).
    let previous_by_pane_id = snapshot
        .panes
        .iter()
        .filter(|pane| refreshed_pane_ids.contains(pane.pane_id.as_str()))
        .map(|pane| (pane.pane_id.clone(), pane.clone()))
        .collect::<HashMap<_, _>>();

    snapshot.panes.retain(|pane| {
        !scope.matches(pane, target_id) && !refreshed_pane_ids.contains(&pane.pane_id)
    });

    if let Some(rows) = rows {
        let mut panes = rows
            .into_iter()
            .map(|row| {
                let previous = previous_by_pane_id.get(&row.pane_id);
                pane_from_targeted_row_preserving_provider_identity(
                    row,
                    previous,
                    false,
                    proc_snapshot,
                    disable_proc_fallback,
                )
            })
            .collect::<Vec<_>>();
        scanner::apply_pane_output_status_fallbacks_with_cache(
            &mut panes,
            pane_output_cache,
            Instant::now(),
        );
        snapshot.panes.extend(panes.into_iter().map(|mut pane| {
            pane.diagnostics.cache_origin = "daemon_update".to_string();
            pane
        }));
    }

    Ok(())
}

pub(super) fn reconcile_full_snapshot(
    snapshot: &mut SnapshotEnvelope,
    tmux_reads: &mut impl TmuxReadProvider,
    tmux_version: Option<&str>,
    pane_output_cache: &mut scanner::PaneOutputStatusCache,
    disable_proc_fallback: bool,
) -> Result<()> {
    *snapshot = daemon_snapshot_from_tmux_with_provider(
        tmux_reads,
        tmux_version,
        pane_output_cache,
        Instant::now(),
        disable_proc_fallback,
    )?;
    Ok(())
}

pub(super) fn reconcile_refresh_outcome(
    previous: &SnapshotEnvelope,
    current: &SnapshotEnvelope,
    publish_context: SnapshotPublishContext,
) -> RefreshOutcome {
    if snapshots_are_materially_equal(previous, current) {
        RefreshOutcome::no_publish_and_reset_reconcile_timer()
    } else {
        RefreshOutcome::publish_and_reset_reconcile_timer(publish_context)
    }
}

/// Field-wise equivalent of cloning both envelopes, blanking the volatile
/// fields (`generated_at`, `source.daemon_generated_at`, and each pane's
/// `diagnostics.cache_origin`), and comparing with `==` — but without any
/// allocation. It runs on nearly every daemon tick, so avoiding the deep clone
/// of every `PaneRecord` matters for steady-state churn.
///
/// Each struct is destructured exhaustively so that adding a field forces a
/// compile error here instead of silently escaping the comparison.
pub(super) fn snapshots_are_materially_equal(
    left: &SnapshotEnvelope,
    right: &SnapshotEnvelope,
) -> bool {
    let SnapshotEnvelope {
        schema_version: left_schema_version,
        // Volatile: cleared by the legacy normalize step before comparison.
        generated_at: _,
        source: left_source,
        panes: left_panes,
    } = left;
    let SnapshotEnvelope {
        schema_version: right_schema_version,
        generated_at: _,
        source: right_source,
        panes: right_panes,
    } = right;

    left_schema_version == right_schema_version
        && source_is_materially_equal(left_source, right_source)
        && left_panes.len() == right_panes.len()
        && left_panes
            .iter()
            .zip(right_panes.iter())
            .all(|(left_pane, right_pane)| pane_is_materially_equal(left_pane, right_pane))
}

fn source_is_materially_equal(left: &SnapshotSource, right: &SnapshotSource) -> bool {
    let SnapshotSource {
        kind: left_kind,
        tmux_version: left_tmux_version,
        // Volatile: cleared by the legacy normalize step before comparison.
        daemon_generated_at: _,
    } = left;
    let SnapshotSource {
        kind: right_kind,
        tmux_version: right_tmux_version,
        daemon_generated_at: _,
    } = right;

    left_kind == right_kind && left_tmux_version == right_tmux_version
}

fn pane_is_materially_equal(left: &PaneRecord, right: &PaneRecord) -> bool {
    let PaneRecord {
        pane_id: left_pane_id,
        location: left_location,
        tmux: left_tmux,
        display: left_display,
        provider: left_provider,
        status: left_status,
        classification: left_classification,
        agent_metadata: left_agent_metadata,
        diagnostics: left_diagnostics,
        last_focus_seq: left_last_focus_seq,
    } = left;
    let PaneRecord {
        pane_id: right_pane_id,
        location: right_location,
        tmux: right_tmux,
        display: right_display,
        provider: right_provider,
        status: right_status,
        classification: right_classification,
        agent_metadata: right_agent_metadata,
        diagnostics: right_diagnostics,
        last_focus_seq: right_last_focus_seq,
    } = right;

    left_pane_id == right_pane_id
        && left_location == right_location
        && left_tmux == right_tmux
        && left_display == right_display
        && left_provider == right_provider
        && left_status == right_status
        && left_classification == right_classification
        && left_agent_metadata == right_agent_metadata
        && diagnostics_are_materially_equal(left_diagnostics, right_diagnostics)
        // Material: a recency change on a published clone must ship that
        // pane in the wire diff so subscriber-reconstructed snapshots keep
        // the field. Runtime snapshots always hold None on both sides, so
        // this leg can never defeat no-op publication suppression.
        && left_last_focus_seq == right_last_focus_seq
}

fn diagnostics_are_materially_equal(left: &PaneDiagnostics, right: &PaneDiagnostics) -> bool {
    let PaneDiagnostics {
        // Volatile: cleared by the legacy normalize step before comparison.
        cache_origin: _,
        proc_fallback: left_proc_fallback,
    } = left;
    let PaneDiagnostics {
        cache_origin: _,
        proc_fallback: right_proc_fallback,
    } = right;

    left_proc_fallback == right_proc_fallback
}

pub(super) fn snapshot_diff(
    left: &SnapshotEnvelope,
    right: &SnapshotEnvelope,
) -> ipc::SnapshotDiffFrame {
    const MAX_DIFF_ITEMS: usize = 24;
    let left_by_id = left
        .panes
        .iter()
        .map(|pane| (pane.pane_id.as_str(), pane))
        .collect::<HashMap<_, _>>();
    let right_by_id = right
        .panes
        .iter()
        .map(|pane| (pane.pane_id.as_str(), pane))
        .collect::<HashMap<_, _>>();
    let mut diff = ipc::SnapshotDiffFrame::default();

    for pane_id in left_by_id.keys() {
        if !right_by_id.contains_key(pane_id) {
            push_bounded(
                &mut diff.removed_pane_ids,
                (*pane_id).to_string(),
                &mut diff.truncated,
            );
        }
    }
    for pane_id in right_by_id.keys() {
        if !left_by_id.contains_key(pane_id) {
            push_bounded(
                &mut diff.added_pane_ids,
                (*pane_id).to_string(),
                &mut diff.truncated,
            );
        }
    }
    for (pane_id, left_pane) in &left_by_id {
        let Some(right_pane) = right_by_id.get(pane_id) else {
            continue;
        };
        let fields = pane_diff_fields(left_pane, right_pane);
        if fields.is_empty() {
            continue;
        }
        if diff.changed_panes.len() >= MAX_DIFF_ITEMS {
            diff.truncated = true;
            continue;
        }
        diff.changed_panes.push(ipc::SnapshotPaneDiffFrame {
            pane_id: (*pane_id).to_string(),
            fields,
        });
    }

    diff
}

/// The exact pane delta for a `snapshot_diff` wire frame, plus the byte growth
/// of panes the delta intentionally omits (needed to keep the snapshot store's
/// full-frame size bound sound).
pub(super) struct SnapshotWireDiff {
    pub(super) changed_panes: Vec<PaneRecord>,
    pub(super) removed_pane_ids: Vec<String>,
    /// Total growth, in encoded-JSON bytes, of volatile fields on panes that
    /// were omitted from `changed_panes` as materially equal. A full frame
    /// still serializes those fields, so omitted growth must count toward the
    /// full-frame size bound even though it never reaches the wire diff.
    pub(super) omitted_pane_growth: usize,
}

/// Builds the exact pane delta for a `snapshot_diff` wire frame: full
/// `PaneRecord`s for every added-or-materially-changed pane, plus the ids of
/// panes present in `previous` but gone from `current`.
///
/// Unlike [`snapshot_diff`] (a bounded, field-name summary for observability),
/// this delta must be lossless — a subscriber upserts `changed` and drops
/// `removed` to reconstruct `current`, so it is neither truncated nor
/// field-filtered. Panes that differ only in volatile fields
/// (`diagnostics.cache_origin`) are intentionally omitted via
/// [`pane_is_materially_equal`]: the reconstructed snapshot stays *materially*
/// equal to a fresh query, which is the daemon's equality contract, while the
/// wire payload shrinks to genuinely-changed panes. Their byte growth is still
/// reported via [`SnapshotWireDiff::omitted_pane_growth`].
pub(super) fn snapshot_wire_diff(
    previous: &SnapshotEnvelope,
    current: &SnapshotEnvelope,
) -> SnapshotWireDiff {
    let previous_by_id = previous
        .panes
        .iter()
        .map(|pane| (pane.pane_id.as_str(), pane))
        .collect::<HashMap<_, _>>();

    let mut changed_panes = Vec::new();
    let mut omitted_pane_growth = 0_usize;
    for pane in &current.panes {
        match previous_by_id.get(pane.pane_id.as_str()) {
            Some(previous_pane) if pane_is_materially_equal(previous_pane, pane) => {
                omitted_pane_growth = omitted_pane_growth.saturating_add(
                    json_string_len(&pane.diagnostics.cache_origin)
                        .saturating_sub(json_string_len(&previous_pane.diagnostics.cache_origin)),
                );
            }
            _ => changed_panes.push(pane.clone()),
        }
    }

    let current_ids = current
        .panes
        .iter()
        .map(|pane| pane.pane_id.as_str())
        .collect::<HashSet<_>>();
    let removed_pane_ids = previous
        .panes
        .iter()
        .filter(|pane| !current_ids.contains(pane.pane_id.as_str()))
        .map(|pane| pane.pane_id.clone())
        .collect();

    SnapshotWireDiff {
        changed_panes,
        removed_pane_ids,
        omitted_pane_growth,
    }
}

/// Encoded length of `value` as a JSON string, including quotes and escapes.
/// (`to_string` cannot fail for a `&str`; the fallback only exists to avoid
/// unwrapping and assumes an unescaped value.)
fn json_string_len(value: &str) -> usize {
    serde_json::to_string(value).map_or_else(|_| value.len().saturating_add(2), |s| s.len())
}

fn push_bounded(items: &mut Vec<String>, item: String, truncated: &mut bool) {
    const MAX_DIFF_ITEMS: usize = 24;
    if items.len() >= MAX_DIFF_ITEMS {
        *truncated = true;
    } else {
        items.push(item);
    }
}

fn pane_diff_fields(left: &PaneRecord, right: &PaneRecord) -> Vec<String> {
    let mut fields = Vec::new();
    if left.provider != right.provider {
        fields.push("provider".to_string());
    }
    if left.status != right.status {
        fields.push("status".to_string());
    }
    if left.tmux.pane_title_raw != right.tmux.pane_title_raw {
        fields.push("title".to_string());
    }
    if left.location != right.location {
        fields.push("location".to_string());
    }
    if left.agent_metadata != right.agent_metadata {
        fields.push("metadata".to_string());
    }
    if left.display != right.display {
        fields.push("display".to_string());
    }
    if left.classification != right.classification {
        fields.push("classification".to_string());
    }
    fields
}

pub(super) fn daemon_snapshot_from_tmux_with_provider(
    tmux_reads: &mut impl TmuxReadProvider,
    tmux_version: Option<&str>,
    pane_output_cache: &mut scanner::PaneOutputStatusCache,
    now: Instant,
    disable_proc_fallback: bool,
) -> Result<SnapshotEnvelope> {
    let rows = tmux_reads.list_all_panes()?;
    let proc_inspector = proc::ProcProcessInspector;
    let mut panes = classify::panes_from_rows_with_proc_fallback_options(
        rows,
        &proc_inspector,
        disable_proc_fallback,
    );
    scanner::apply_pane_output_status_fallbacks_with_cache(&mut panes, pane_output_cache, now);

    let mut snapshot = SnapshotEnvelope {
        schema_version: CACHE_SCHEMA_VERSION,
        generated_at: snapshot::now_rfc3339()?,
        source: SnapshotSource {
            kind: SourceKind::Snapshot,
            tmux_version: tmux_version.map(str::to_string),
            daemon_generated_at: None,
        },
        panes,
    };
    snapshot::sort_snapshot_panes(&mut snapshot);
    for pane in &mut snapshot.panes {
        pane.diagnostics.cache_origin = "daemon_snapshot".to_string();
    }
    snapshot::mark_snapshot_as_daemon(&mut snapshot)?;
    Ok(snapshot)
}

#[cfg(test)]
pub(crate) fn test_refresh_snapshot_pane_with_provider(
    snapshot: &mut SnapshotEnvelope,
    tmux_reads: &mut impl TmuxReadProvider,
    pane_id: &str,
) -> Result<()> {
    let mut pane_output_cache = scanner::PaneOutputStatusCache::new(PANE_OUTPUT_STATUS_CACHE_TTL);
    let proc_snapshot = proc::LazyProcessSnapshot::new(&PROC_INSPECTOR);
    refresh_snapshot_pane_with_title(
        snapshot,
        tmux_reads,
        pane_id,
        None,
        &mut pane_output_cache,
        &proc_snapshot,
        false,
    )
    .map(|_| ())
}

#[cfg(test)]
pub(crate) fn test_apply_resnapshot_control_event_with_provider(
    snapshot: &mut SnapshotEnvelope,
    tmux_reads: &mut impl TmuxReadProvider,
) -> Result<(bool, bool)> {
    let mut pane_output_cache = scanner::PaneOutputStatusCache::new(PANE_OUTPUT_STATUS_CACHE_TTL);
    let mut batch = ControlEventBatch::default();
    batch.push(ControlEvent::Resnapshot);
    let outcome =
        apply_control_event_batch(snapshot, tmux_reads, &batch, &mut pane_output_cache, false)?;
    Ok((outcome.changed, outcome.full_snapshot_refresh))
}

#[cfg(test)]
pub(crate) fn test_apply_control_event_lines_with_provider(
    snapshot: &mut SnapshotEnvelope,
    tmux_reads: &mut impl TmuxReadProvider,
    lines: &[String],
) -> Result<(bool, bool, bool)> {
    let mut pane_output_cache = scanner::PaneOutputStatusCache::new(PANE_OUTPUT_STATUS_CACHE_TTL);
    let batch = ControlEventBatch::from_lines(lines);
    let outcome =
        apply_control_event_batch(snapshot, tmux_reads, &batch, &mut pane_output_cache, false)?;
    Ok((
        outcome.changed,
        outcome.full_snapshot_refresh,
        outcome.fallback_to_full,
    ))
}

#[cfg(test)]
pub(crate) fn test_apply_control_event_lines_with_provider_counts(
    snapshot: &mut SnapshotEnvelope,
    tmux_reads: &mut impl TmuxReadProvider,
    lines: &[String],
) -> Result<(bool, bool, bool, u64, u64, u64)> {
    let mut pane_output_cache = scanner::PaneOutputStatusCache::new(PANE_OUTPUT_STATUS_CACHE_TTL);
    let batch = ControlEventBatch::from_lines(lines);
    let outcome =
        apply_control_event_batch(snapshot, tmux_reads, &batch, &mut pane_output_cache, false)?;
    Ok((
        outcome.changed,
        outcome.full_snapshot_refresh,
        outcome.fallback_to_full,
        outcome.targeted_title_updates,
        outcome.targeted_pane_refreshes,
        outcome.targeted_scope_refreshes,
    ))
}

#[cfg(test)]
pub(crate) fn test_recover_targeted_pane_provider_with_inspector(
    pane: &mut PaneRecord,
    inspector: &impl proc::ProcessInspector,
) {
    let proc_snapshot = proc::LazyProcessSnapshot::new(inspector);
    recover_targeted_pane_provider(pane, &proc_snapshot, false);
}

#[cfg(test)]
pub(crate) fn test_refresh_snapshot_pane_title_with_provider(
    snapshot: &mut SnapshotEnvelope,
    tmux_reads: &mut impl TmuxReadProvider,
    pane_id: &str,
    title_override: &str,
) -> Result<()> {
    let mut pane_output_cache = scanner::PaneOutputStatusCache::new(PANE_OUTPUT_STATUS_CACHE_TTL);
    let proc_snapshot = proc::LazyProcessSnapshot::new(&PROC_INSPECTOR);
    refresh_snapshot_pane_with_title(
        snapshot,
        tmux_reads,
        pane_id,
        Some(title_override),
        &mut pane_output_cache,
        &proc_snapshot,
        false,
    )
    .map(|_| ())
}

#[cfg(test)]
pub(crate) fn test_refresh_snapshot_window_with_provider(
    snapshot: &mut SnapshotEnvelope,
    tmux_reads: &mut impl TmuxReadProvider,
    window_id: &str,
) -> Result<()> {
    let mut pane_output_cache = scanner::PaneOutputStatusCache::new(PANE_OUTPUT_STATUS_CACHE_TTL);
    refresh_snapshot_window(
        snapshot,
        tmux_reads,
        window_id,
        &mut pane_output_cache,
        false,
    )
}

#[cfg(test)]
pub(crate) fn test_refresh_snapshot_for_focused_pane_with_provider(
    snapshot: &mut SnapshotEnvelope,
    tmux_reads: &mut impl TmuxReadProvider,
    pane_id: &str,
) -> Result<()> {
    let mut pane_output_cache = scanner::PaneOutputStatusCache::new(PANE_OUTPUT_STATUS_CACHE_TTL);
    refresh_snapshot_for_focused_pane(
        snapshot,
        tmux_reads,
        pane_id,
        None,
        &mut pane_output_cache,
        false,
    )
}

#[cfg(test)]
pub(crate) fn test_refresh_snapshot_session_with_inspector(
    snapshot: &mut SnapshotEnvelope,
    tmux_reads: &mut impl TmuxReadProvider,
    session_id: &str,
    inspector: &impl proc::ProcessInspector,
) -> Result<()> {
    let mut pane_output_cache = scanner::PaneOutputStatusCache::new(PANE_OUTPUT_STATUS_CACHE_TTL);
    let proc_snapshot = proc::LazyProcessSnapshot::new(inspector);
    refresh_snapshot_scope_no_finalize(
        snapshot,
        tmux_reads,
        TargetScope::Session,
        session_id,
        &mut pane_output_cache,
        &proc_snapshot,
        false,
    )?;
    finalize_snapshot(snapshot)
}

#[cfg(test)]
pub(crate) fn test_refresh_snapshot_session_with_provider(
    snapshot: &mut SnapshotEnvelope,
    tmux_reads: &mut impl TmuxReadProvider,
    session_id: &str,
) -> Result<()> {
    let mut pane_output_cache = scanner::PaneOutputStatusCache::new(PANE_OUTPUT_STATUS_CACHE_TTL);
    refresh_snapshot_session(
        snapshot,
        tmux_reads,
        session_id,
        &mut pane_output_cache,
        false,
    )
}

#[cfg(test)]
pub(crate) fn test_reconcile_full_snapshot_with_provider(
    snapshot: &mut SnapshotEnvelope,
    tmux_reads: &mut impl TmuxReadProvider,
    tmux_version: Option<&str>,
) -> Result<()> {
    let mut pane_output_cache = scanner::PaneOutputStatusCache::new(PANE_OUTPUT_STATUS_CACHE_TTL);
    reconcile_full_snapshot(
        snapshot,
        tmux_reads,
        tmux_version,
        &mut pane_output_cache,
        false,
    )
}

#[derive(Clone, Copy)]
enum TargetScope {
    Window,
    Session,
}

impl TargetScope {
    fn matches(self, pane: &PaneRecord, target_id: &str) -> bool {
        match self {
            Self::Window => pane.tmux.window_id.as_deref() == Some(target_id),
            Self::Session => pane.tmux.session_id.as_deref() == Some(target_id),
        }
    }

    fn pane_list_scope(self) -> tmux::PaneListScope {
        match self {
            Self::Window => tmux::PaneListScope::Window,
            Self::Session => tmux::PaneListScope::Session,
        }
    }
}

#[cfg(test)]
mod material_equality_tests {
    use super::*;
    use proptest::prelude::*;

    /// The original clone-then-normalize-then-`==` behavior, kept here verbatim
    /// as a ground-truth oracle for the zero-allocation replacement.
    fn oracle_snapshots_are_materially_equal(
        left: &SnapshotEnvelope,
        right: &SnapshotEnvelope,
    ) -> bool {
        fn normalize(snapshot: &mut SnapshotEnvelope) {
            snapshot.generated_at.clear();
            snapshot.source.daemon_generated_at = None;
            for pane in &mut snapshot.panes {
                pane.diagnostics.cache_origin.clear();
            }
        }
        let mut left = left.clone();
        let mut right = right.clone();
        normalize(&mut left);
        normalize(&mut right);
        left == right
    }

    fn test_row(pane_id: &str, title: &str, command: &str) -> TmuxPaneRow {
        TmuxPaneRow {
            session_name: "session".to_string(),
            window_index: 0,
            pane_index: 0,
            pane_id: pane_id.to_string(),
            pane_pid: 4242,
            pane_current_command: command.to_string(),
            pane_title_raw: title.to_string(),
            pane_tty: "/dev/ttys0".to_string(),
            pane_current_path: "/tmp/agentscan".to_string(),
            window_name: "window".to_string(),
            session_id: Some("$1".to_string()),
            window_id: Some("@0".to_string()),
            agent_provider: None,
            agent_label: None,
            agent_cwd: None,
            agent_state: None,
            agent_session_id: None,
            agent_pid: None,
            agent_version: None,
            agent_model: None,
            pane_active: false,
            window_active: false,
        }
    }

    fn sample_snapshot() -> SnapshotEnvelope {
        let mut pane_a = classify::pane_from_row(test_row("%1", "alpha", "codex"));
        pane_a.diagnostics.cache_origin = "daemon_snapshot".to_string();
        let mut pane_b = classify::pane_from_row(test_row("%2", "beta", "claude"));
        pane_b.diagnostics.cache_origin = "daemon_update".to_string();
        SnapshotEnvelope {
            schema_version: 1,
            generated_at: "2026-07-13T00:00:00Z".to_string(),
            source: SnapshotSource {
                kind: SourceKind::Daemon,
                tmux_version: Some("3.4".to_string()),
                daemon_generated_at: Some("2026-07-13T00:00:01Z".to_string()),
            },
            panes: vec![pane_a, pane_b],
        }
    }

    #[test]
    fn equal_except_volatile_fields_are_materially_equal() {
        let left = sample_snapshot();
        let mut right = left.clone();
        // Perturb only the volatile fields that normalize used to blank.
        right.generated_at = "2099-01-01T00:00:00Z".to_string();
        right.source.daemon_generated_at = None;
        right.panes[0].diagnostics.cache_origin = "something_else".to_string();
        right.panes[1].diagnostics.cache_origin.clear();

        assert!(snapshots_are_materially_equal(&left, &right));
        // Matches the oracle.
        assert_eq!(
            snapshots_are_materially_equal(&left, &right),
            oracle_snapshots_are_materially_equal(&left, &right),
        );
    }

    #[test]
    fn each_material_field_difference_breaks_equality() {
        let base = sample_snapshot();

        type Mutator = fn(&mut SnapshotEnvelope);
        let mutators: Vec<(&str, Mutator)> = vec![
            ("schema_version", |s| s.schema_version += 1),
            ("source.kind", |s| s.source.kind = SourceKind::Snapshot),
            ("source.tmux_version", |s| {
                s.source.tmux_version = Some("9.9".to_string())
            }),
            ("pane_id", |s| s.panes[0].pane_id = "%99".to_string()),
            ("pane.location", |s| s.panes[0].location.window_index += 1),
            ("pane.tmux.title", |s| {
                s.panes[0].tmux.pane_title_raw = "changed".to_string()
            }),
            ("pane.tmux.active", |s| {
                s.panes[0].tmux.pane_active = !s.panes[0].tmux.pane_active
            }),
            ("pane.display", |s| {
                s.panes[0].display.label = "changed".to_string()
            }),
            ("pane.provider", |s| s.panes[0].provider = None),
            ("pane.status", |s| {
                s.panes[0].status = PaneStatus::title(StatusKind::Busy)
            }),
            ("pane.classification", |s| {
                s.panes[0].classification.reasons.push("extra".to_string())
            }),
            ("pane.agent_metadata", |s| {
                s.panes[0].agent_metadata.label = Some("agent".to_string())
            }),
            ("pane.proc_fallback", |s| {
                s.panes[0].diagnostics.proc_fallback.outcome = ProcFallbackOutcome::Resolved
            }),
            ("pane.last_focus_seq", |s| {
                s.panes[0].last_focus_seq = Some(7)
            }),
            ("pane_count", |s| {
                s.panes.pop();
            }),
        ];

        for (label, mutate) in mutators {
            let mut mutated = base.clone();
            mutate(&mut mutated);
            assert!(
                !snapshots_are_materially_equal(&base, &mutated),
                "expected material difference for `{label}` to break equality",
            );
            assert_eq!(
                snapshots_are_materially_equal(&base, &mutated),
                oracle_snapshots_are_materially_equal(&base, &mutated),
                "new impl disagreed with oracle for `{label}`",
            );
        }
    }

    fn arb_pane() -> impl Strategy<Value = PaneRecord> {
        (
            prop::sample::select(vec!["%1", "%2", "%3"]),
            prop::sample::select(vec!["alpha", "beta"]),
            prop::sample::select(vec!["codex", "claude", "bash"]),
            prop::sample::select(vec!["origin_a", "origin_b"]),
            any::<bool>(),
        )
            .prop_map(|(pane_id, title, command, cache_origin, active)| {
                let mut pane = classify::pane_from_row(test_row(pane_id, title, command));
                pane.diagnostics.cache_origin = cache_origin.to_string();
                pane.tmux.pane_active = active;
                pane
            })
    }

    fn arb_snapshot() -> impl Strategy<Value = SnapshotEnvelope> {
        (
            prop::sample::select(vec![1u32, 2u32]),
            prop::sample::select(vec!["g1", "g2"]),
            prop::sample::select(vec![SourceKind::Snapshot, SourceKind::Daemon]),
            prop::option::of(prop::sample::select(vec!["3.4", "3.5"])),
            prop::option::of(prop::sample::select(vec!["d1", "d2"])),
            prop::collection::vec(arb_pane(), 0..=3),
        )
            .prop_map(
                |(schema_version, generated_at, kind, tmux_version, daemon_generated_at, panes)| {
                    SnapshotEnvelope {
                        schema_version,
                        generated_at: generated_at.to_string(),
                        source: SnapshotSource {
                            kind,
                            tmux_version: tmux_version.map(str::to_string),
                            daemon_generated_at: daemon_generated_at.map(str::to_string),
                        },
                        panes,
                    }
                },
            )
    }

    proptest! {
        #[test]
        fn materially_equal_matches_clone_normalize_oracle(
            left in arb_snapshot(),
            right in arb_snapshot(),
        ) {
            prop_assert_eq!(
                snapshots_are_materially_equal(&left, &right),
                oracle_snapshots_are_materially_equal(&left, &right),
            );
        }
    }
}
