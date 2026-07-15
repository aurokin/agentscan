use super::super::*;
use std::collections::HashMap;
use std::time::{Duration, Instant};

pub(crate) fn apply_pane_output_status_fallbacks(
    panes: &mut [PaneRecord],
) -> PaneOutputCaptureStats {
    let mut capture = TmuxPaneOutputCapture;
    apply_pane_output_status_fallbacks_with_capture(panes, &mut capture)
}

pub(crate) fn apply_pane_output_status_fallbacks_with_cache(
    panes: &mut [PaneRecord],
    cache: &mut PaneOutputStatusCache,
    now: Instant,
) {
    let mut capture = TmuxPaneOutputCapture;
    cache.apply(panes, &mut capture, now);
}

pub(crate) trait PaneOutputCapture {
    fn capture_screen(&mut self, pane_id: &str) -> Result<Option<String>>;
}

struct TmuxPaneOutputCapture;

impl PaneOutputCapture for TmuxPaneOutputCapture {
    fn capture_screen(&mut self, pane_id: &str) -> Result<Option<String>> {
        tmux::tmux_capture_pane_screen(pane_id)
    }
}

fn apply_pane_output_status_fallbacks_with_capture(
    panes: &mut [PaneRecord],
    capture: &mut impl PaneOutputCapture,
) -> PaneOutputCaptureStats {
    let mut stats = PaneOutputCaptureStats::default();
    for pane in panes {
        if !classify::pane_output_status_fallback_candidate(pane) {
            continue;
        }

        stats.attempt_count = stats.attempt_count.saturating_add(1);
        // Mirror the cached path: a transient capture failure is recorded as an
        // error rather than silently swallowed, so it stays distinguishable from
        // a successful capture that simply produced no status.
        match capture.capture_screen(&pane.pane_id) {
            Ok(Some(output)) => classify::apply_pane_output_status_fallback(pane, &output),
            Ok(None) => {}
            Err(_) => {
                stats.error_count = stats.error_count.saturating_add(1);
            }
        }
    }
    stats
}

/// Cumulative `capture-pane` accounting for the daemon's pane-output status path.
/// Lets us measure how heavily a session leans on the (relatively expensive)
/// capture-pane fallback and how effective the TTL cache is at suppressing it.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct PaneOutputCaptureStats {
    pub(crate) attempt_count: u64,
    pub(crate) hit_count: u64,
    pub(crate) error_count: u64,
}

pub(crate) struct PaneOutputStatusCache {
    ttl: Duration,
    entries: HashMap<String, PaneOutputStatusCacheEntry>,
    stats: PaneOutputCaptureStats,
}

impl PaneOutputStatusCache {
    pub(crate) fn new(ttl: Duration) -> Self {
        Self {
            ttl,
            entries: HashMap::new(),
            stats: PaneOutputCaptureStats::default(),
        }
    }

    pub(crate) fn capture_stats(&self) -> PaneOutputCaptureStats {
        self.stats
    }

    pub(crate) fn apply(
        &mut self,
        panes: &mut [PaneRecord],
        capture: &mut impl PaneOutputCapture,
        now: Instant,
    ) {
        self.prune_expired(now);

        for pane in panes {
            if !classify::pane_output_status_fallback_candidate(pane) {
                continue;
            }

            // `fallback_candidate` above implies a provider, but let the type
            // system enforce it: a providerless pane simply is not cacheable.
            let Some(key) = PaneOutputStatusCacheKey::from_pane(pane) else {
                continue;
            };
            let cacheable = classify::pane_output_status_candidate_cacheable(pane);
            if cacheable && let Some(entry) = self.fresh_entry(&pane.pane_id, &key, now) {
                let status_kind = entry.status_kind;
                self.stats.hit_count = self.stats.hit_count.saturating_add(1);
                if let Some(kind) = status_kind {
                    pane.status = PaneStatus::pane_output(kind);
                }
                continue;
            }

            self.stats.attempt_count = self.stats.attempt_count.saturating_add(1);
            let status_kind = match capture.capture_screen(&pane.pane_id) {
                Ok(output) => {
                    if let Some(output) = output {
                        classify::apply_pane_output_status_fallback(pane, &output);
                    }

                    (pane.status.source == StatusSource::PaneOutput).then_some(pane.status.kind)
                }
                Err(_) => {
                    self.stats.error_count = self.stats.error_count.saturating_add(1);
                    continue;
                }
            };

            if cacheable {
                self.entries.insert(
                    pane.pane_id.clone(),
                    PaneOutputStatusCacheEntry {
                        key,
                        status_kind,
                        checked_at: now,
                    },
                );
            }
        }
    }

    fn fresh_entry(
        &self,
        pane_id: &str,
        key: &PaneOutputStatusCacheKey,
        now: Instant,
    ) -> Option<&PaneOutputStatusCacheEntry> {
        let entry = self.entries.get(pane_id)?;
        if &entry.key != key {
            return None;
        }

        now.checked_duration_since(entry.checked_at)
            .is_some_and(|age| age <= self.ttl)
            .then_some(entry)
    }

    fn prune_expired(&mut self, now: Instant) {
        let ttl = self.ttl;
        self.entries.retain(|_, entry| {
            now.checked_duration_since(entry.checked_at)
                .is_some_and(|age| age <= ttl)
        });
    }

    /// Drop the cached status for a pane so the next `apply` re-captures it unconditionally.
    ///
    /// The settle re-capture uses this: a pane already classified `Busy` from pane output is
    /// not a fallback candidate, and an idle transition emits no further tmux activity, so the
    /// only way to re-confirm it is to force a fresh capture rather than wait out the TTL.
    pub(crate) fn invalidate(&mut self, pane_id: &str) {
        self.entries.remove(pane_id);
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct PaneOutputStatusCacheEntry {
    key: PaneOutputStatusCacheKey,
    status_kind: Option<StatusKind>,
    checked_at: Instant,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct PaneOutputStatusCacheKey {
    provider: Provider,
    pane_current_command: String,
    pane_title_raw: String,
    session_id: Option<String>,
    window_id: Option<String>,
}

impl PaneOutputStatusCacheKey {
    fn from_pane(pane: &PaneRecord) -> Option<Self> {
        Some(Self {
            provider: pane.provider?,
            pane_current_command: pane.tmux.pane_current_command.clone(),
            pane_title_raw: pane.tmux.pane_title_raw.clone(),
            session_id: pane.tmux.session_id.clone(),
            window_id: pane.tmux.window_id.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct FakePaneOutputCapture {
        outputs: HashMap<String, Option<String>>,
        errors: std::collections::HashSet<String>,
        calls: Vec<String>,
    }

    impl FakePaneOutputCapture {
        fn with_output(mut self, pane_id: &str, output: impl Into<String>) -> Self {
            self.outputs
                .insert(pane_id.to_string(), Some(output.into()));
            self
        }

        fn with_error(mut self, pane_id: &str) -> Self {
            self.errors.insert(pane_id.to_string());
            self
        }

        fn call_count(&self) -> usize {
            self.calls.len()
        }
    }

    impl PaneOutputCapture for FakePaneOutputCapture {
        fn capture_screen(&mut self, pane_id: &str) -> Result<Option<String>> {
            self.calls.push(pane_id.to_string());
            if self.errors.contains(pane_id) {
                anyhow::bail!("simulated capture failure for {pane_id}");
            }
            Ok(self.outputs.get(pane_id).cloned().unwrap_or(None))
        }
    }

    fn pane(pane_id: &str, provider: Option<Provider>, status: PaneStatus) -> PaneRecord {
        PaneRecord {
            pane_id: pane_id.to_string(),
            location: PaneLocation {
                session_name: "s".to_string(),
                window_index: 0,
                pane_index: 0,
                window_name: "w".to_string(),
            },
            tmux: TmuxPaneMetadata {
                pane_pid: 100,
                pane_tty: "/dev/ttys001".to_string(),
                pane_current_path: "/tmp".to_string(),
                pane_current_command: "node".to_string(),
                pane_title_raw: "GitHub Copilot".to_string(),
                session_id: Some("$1".to_string()),
                window_id: Some("@1".to_string()),
                pane_active: false,
                window_active: false,
            },
            display: DisplayMetadata {
                label: "GitHub Copilot".to_string(),
                activity_label: None,
            },
            provider,
            status,
            classification: PaneClassification {
                matched_by: None,
                confidence: None,
                reasons: Vec::new(),
            },
            agent_metadata: AgentMetadata::default(),
            diagnostics: PaneDiagnostics {
                cache_origin: "test".to_string(),
                proc_fallback: ProcFallbackDiagnostics::default(),
            },
            last_focus_seq: None,
        }
    }

    fn copilot_busy_output() -> &'static str {
        "❯ Review patch\n\n\
         ● Thinking (Esc to cancel · 616 B)\n\
         /tmp/probe [main]\n\
         ────────────────────\n\
         ❯\n\
         ────────────────────\n\
         / commands · ? help\n"
    }

    fn codex_idle_output() -> &'static str {
        "› Ask Codex to do anything\n\
         \n\
           tab to queue message                                       100% context left\n"
    }

    fn claude_idle_output() -> &'static str {
        "╭────────────────────────────────────────╮\n\
         ❯ Try \"fix the failing test\"\n\
         ╰────────────────────────────────────────╯\n\
         ? for shortcuts\n"
    }

    fn gemini_idle_output() -> &'static str {
        ">   Type your message or @path/to/file\n\
         Workspace   Sandbox    Model\n\
         ~/code/app  no sandbox gemini-2.5-pro\n"
    }

    fn gemini_auth_output() -> &'static str {
        "╭──────────────────────────────────────────────────────────────────────────────╮\n\
         │  Opening authentication page in your browser.                                │\n\
         │  Do you want to continue?                                                    │\n\
         │  Enter to select · ↑/↓ to navigate · Esc to cancel                           │\n\
         ╰──────────────────────────────────────────────────────────────────────────────╯\n"
    }

    fn opencode_idle_output() -> &'static str {
        "╹  Ask anything... \"Fix a TODO in the codebase\"\n\
         Build · gpt-5.1 OpenAI\n\
         ~/code/app                                                    /status\n"
    }

    fn pi_idle_output() -> &'static str {
        "────────────────────────────────\n\
                                        \n\
         ────────────────────────────────\n\
         ~/code/app\n\
         0.0%/200k                                      claude-sonnet\n"
    }

    #[test]
    fn pane_output_capture_runs_only_for_supported_unknown_status_candidates() {
        let mut panes = vec![
            pane("%1", None, PaneStatus::not_checked()),
            pane("%2", Some(Provider::Codex), PaneStatus::not_checked()),
            pane(
                "%3",
                Some(Provider::Copilot),
                PaneStatus::title(StatusKind::Busy),
            ),
            pane("%4", Some(Provider::Copilot), PaneStatus::not_checked()),
            pane(
                "%5",
                Some(Provider::Gemini),
                PaneStatus::title(StatusKind::Idle),
            ),
            pane("%6", Some(Provider::Gemini), PaneStatus::not_checked()),
            pane("%7", Some(Provider::Opencode), PaneStatus::not_checked()),
            pane("%8", Some(Provider::Pi), PaneStatus::not_checked()),
            pane("%9", Some(Provider::Claude), PaneStatus::not_checked()),
            pane(
                "%10",
                Some(Provider::Claude),
                PaneStatus::title(StatusKind::Busy),
            ),
            pane("%11", Some(Provider::Aider), PaneStatus::not_checked()),
        ];
        let mut capture = FakePaneOutputCapture::default()
            .with_output("%2", codex_idle_output())
            .with_output("%4", copilot_busy_output())
            .with_output("%5", gemini_auth_output())
            .with_output("%6", gemini_idle_output())
            .with_output("%7", opencode_idle_output())
            .with_output("%8", pi_idle_output())
            .with_output("%9", claude_idle_output());

        apply_pane_output_status_fallbacks_with_capture(&mut panes, &mut capture);

        assert_eq!(
            capture.calls,
            vec!["%2", "%4", "%5", "%6", "%7", "%8", "%9"]
        );
        assert_eq!(panes[1].status, PaneStatus::pane_output(StatusKind::Idle));
        assert_eq!(panes[3].status, PaneStatus::pane_output(StatusKind::Busy));
        assert_eq!(panes[4].status, PaneStatus::pane_output(StatusKind::Busy));
        assert_eq!(panes[5].status, PaneStatus::pane_output(StatusKind::Idle));
        assert_eq!(panes[6].status, PaneStatus::pane_output(StatusKind::Idle));
        assert_eq!(panes[7].status, PaneStatus::pane_output(StatusKind::Idle));
        assert_eq!(panes[8].status, PaneStatus::pane_output(StatusKind::Idle));
        assert_eq!(panes[10].status, PaneStatus::not_checked());
    }

    #[test]
    fn pane_output_fallbacks_record_capture_errors() {
        // `%1` captures a status successfully; `%2`'s capture fails transiently.
        // The failure must surface as a recorded error, distinguishable from a
        // successful capture that yielded no status.
        let mut panes = vec![
            pane("%1", Some(Provider::Copilot), PaneStatus::not_checked()),
            pane("%2", Some(Provider::Codex), PaneStatus::not_checked()),
        ];
        let mut capture = FakePaneOutputCapture::default()
            .with_output("%1", copilot_busy_output())
            .with_error("%2");

        let stats = apply_pane_output_status_fallbacks_with_capture(&mut panes, &mut capture);

        assert_eq!(stats.attempt_count, 2);
        assert_eq!(stats.error_count, 1);
        assert_eq!(panes[0].status, PaneStatus::pane_output(StatusKind::Busy));
        assert_eq!(panes[1].status, PaneStatus::not_checked());
    }

    #[test]
    fn pane_output_cache_reuses_recent_status_for_matching_key() {
        let now = Instant::now();
        let mut cache = PaneOutputStatusCache::new(Duration::from_secs(2));
        let mut capture = FakePaneOutputCapture::default().with_output("%1", copilot_busy_output());
        let mut first = vec![pane(
            "%1",
            Some(Provider::Copilot),
            PaneStatus::not_checked(),
        )];
        cache.apply(&mut first, &mut capture, now);

        let mut second = vec![pane(
            "%1",
            Some(Provider::Copilot),
            PaneStatus::not_checked(),
        )];
        cache.apply(&mut second, &mut capture, now + Duration::from_millis(500));

        assert_eq!(capture.call_count(), 1);
        assert_eq!(second[0].status, PaneStatus::pane_output(StatusKind::Busy));
    }

    #[test]
    fn pane_output_cache_does_not_cache_gemini_title_idle_refinement_candidates() {
        let now = Instant::now();
        let mut cache = PaneOutputStatusCache::new(Duration::from_secs(2));
        let mut capture = FakePaneOutputCapture::default().with_output("%1", gemini_idle_output());
        let mut first = vec![pane(
            "%1",
            Some(Provider::Gemini),
            PaneStatus::title(StatusKind::Idle),
        )];
        cache.apply(&mut first, &mut capture, now);

        capture
            .outputs
            .insert("%1".to_string(), Some(gemini_auth_output().to_string()));
        let mut second = vec![pane(
            "%1",
            Some(Provider::Gemini),
            PaneStatus::title(StatusKind::Idle),
        )];
        cache.apply(&mut second, &mut capture, now + Duration::from_millis(500));

        assert_eq!(capture.call_count(), 2);
        assert_eq!(first[0].status, PaneStatus::title(StatusKind::Idle));
        assert_eq!(second[0].status, PaneStatus::pane_output(StatusKind::Busy));
    }

    #[test]
    fn pane_output_cache_does_not_reuse_gemini_unknown_status_entries() {
        let now = Instant::now();
        let mut cache = PaneOutputStatusCache::new(Duration::from_secs(2));
        let mut capture = FakePaneOutputCapture::default().with_output("%1", gemini_idle_output());
        let mut first = vec![pane(
            "%1",
            Some(Provider::Gemini),
            PaneStatus::not_checked(),
        )];
        cache.apply(&mut first, &mut capture, now);

        capture
            .outputs
            .insert("%1".to_string(), Some(gemini_auth_output().to_string()));
        let mut second = vec![pane(
            "%1",
            Some(Provider::Gemini),
            PaneStatus::not_checked(),
        )];
        cache.apply(&mut second, &mut capture, now + Duration::from_millis(500));

        assert_eq!(capture.call_count(), 2);
        assert_eq!(first[0].status, PaneStatus::pane_output(StatusKind::Idle));
        assert_eq!(second[0].status, PaneStatus::pane_output(StatusKind::Busy));
    }

    #[test]
    fn pane_output_cache_tracks_capture_attempts_hits_and_errors() {
        let now = Instant::now();
        let mut cache = PaneOutputStatusCache::new(Duration::from_secs(2));
        // `%1` captures successfully; `%2` has no fake output configured, so the
        // capture returns `Ok(None)` (an attempt that resolves nothing, not an error).
        let mut capture = FakePaneOutputCapture::default().with_output("%1", copilot_busy_output());

        let mut first = vec![
            pane("%1", Some(Provider::Copilot), PaneStatus::not_checked()),
            pane("%2", Some(Provider::Codex), PaneStatus::not_checked()),
        ];
        cache.apply(&mut first, &mut capture, now);

        // Second pass within TTL: `%1` reuses the cached status (a hit, no capture).
        let mut second = vec![pane(
            "%1",
            Some(Provider::Copilot),
            PaneStatus::not_checked(),
        )];
        cache.apply(&mut second, &mut capture, now + Duration::from_millis(500));

        let stats = cache.capture_stats();
        assert_eq!(stats.attempt_count, 2);
        assert_eq!(stats.hit_count, 1);
        assert_eq!(stats.error_count, 0);
    }

    #[test]
    fn pane_output_cache_invalidates_when_key_changes() {
        let now = Instant::now();
        let mut cache = PaneOutputStatusCache::new(Duration::from_secs(2));
        let mut capture = FakePaneOutputCapture::default().with_output("%1", copilot_busy_output());
        let mut first = vec![pane(
            "%1",
            Some(Provider::Copilot),
            PaneStatus::not_checked(),
        )];
        cache.apply(&mut first, &mut capture, now);

        let mut second = vec![pane(
            "%1",
            Some(Provider::Copilot),
            PaneStatus::not_checked(),
        )];
        second[0].tmux.pane_title_raw = "GitHub Copilot | Query".to_string();
        cache.apply(&mut second, &mut capture, now + Duration::from_millis(500));

        assert_eq!(capture.call_count(), 2);
    }

    #[test]
    fn pane_output_cache_expires() {
        let now = Instant::now();
        let mut cache = PaneOutputStatusCache::new(Duration::from_secs(2));
        let mut capture = FakePaneOutputCapture::default().with_output("%1", copilot_busy_output());
        let mut first = vec![pane(
            "%1",
            Some(Provider::Copilot),
            PaneStatus::not_checked(),
        )];
        cache.apply(&mut first, &mut capture, now);

        let mut second = vec![pane(
            "%1",
            Some(Provider::Copilot),
            PaneStatus::not_checked(),
        )];
        cache.apply(&mut second, &mut capture, now + Duration::from_secs(3));

        assert_eq!(capture.call_count(), 2);
    }

    #[test]
    fn pane_output_cache_reuses_recent_no_match() {
        let now = Instant::now();
        let mut cache = PaneOutputStatusCache::new(Duration::from_secs(2));
        let mut capture = FakePaneOutputCapture::default().with_output("%1", "plain output");
        let mut first = vec![pane(
            "%1",
            Some(Provider::Copilot),
            PaneStatus::not_checked(),
        )];
        cache.apply(&mut first, &mut capture, now);

        let mut second = vec![pane(
            "%1",
            Some(Provider::Copilot),
            PaneStatus::not_checked(),
        )];
        cache.apply(&mut second, &mut capture, now + Duration::from_millis(500));

        assert_eq!(capture.call_count(), 1);
        assert_eq!(second[0].status, PaneStatus::not_checked());
    }

    #[test]
    fn pane_output_cache_prunes_expired_entries() {
        let now = Instant::now();
        let mut cache = PaneOutputStatusCache::new(Duration::from_secs(2));
        let mut capture = FakePaneOutputCapture::default().with_output("%1", copilot_busy_output());
        let mut first = vec![pane(
            "%1",
            Some(Provider::Copilot),
            PaneStatus::not_checked(),
        )];
        cache.apply(&mut first, &mut capture, now);

        let mut empty = Vec::new();
        cache.apply(&mut empty, &mut capture, now + Duration::from_secs(3));

        assert!(cache.entries.is_empty());
    }
}
