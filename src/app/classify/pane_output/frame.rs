pub(super) struct PaneOutputFrame<'a> {
    lines: Vec<&'a str>,
}

impl<'a> PaneOutputFrame<'a> {
    pub(super) fn new(output: &'a str) -> Self {
        Self {
            lines: output.lines().collect(),
        }
    }

    pub(super) fn rposition(&self, predicate: impl Fn(&str) -> bool) -> Option<usize> {
        self.lines.iter().rposition(|line| predicate(line))
    }

    pub(super) fn line(&self, index: usize) -> Option<&'a str> {
        self.lines.get(index).copied()
    }

    pub(super) fn lines_before(&self, index: usize) -> Option<&[&'a str]> {
        self.contains_index(index).then(|| &self.lines[..index])
    }

    pub(super) fn lines_from(&self, index: usize) -> Option<&[&'a str]> {
        self.contains_index(index).then(|| &self.lines[index..])
    }

    pub(super) fn range(&self, start: usize, end: usize) -> Option<&[&'a str]> {
        (start <= end && end <= self.lines.len()).then(|| &self.lines[start..end])
    }

    pub(super) fn window_before(&self, index: usize, max_before: usize) -> Option<&[&'a str]> {
        self.contains_index(index)
            .then(|| &self.lines[index.saturating_sub(max_before)..index])
    }

    pub(super) fn window_ending_at(&self, index: usize, max_before: usize) -> Option<&[&'a str]> {
        self.contains_index(index)
            .then(|| &self.lines[index.saturating_sub(max_before)..=index])
    }

    pub(super) fn rposition_before(
        &self,
        index: usize,
        predicate: impl Fn(&str) -> bool,
    ) -> Option<usize> {
        self.lines_before(index)?
            .iter()
            .rposition(|line| predicate(line))
    }

    pub(super) fn previous_nonblank_before(&self, index: usize) -> Option<&'a str> {
        self.lines_before(index)?
            .iter()
            .rev()
            .copied()
            .find(|line| !line.trim().is_empty())
    }

    pub(super) fn last_nonblank(&self) -> Option<&'a str> {
        self.lines
            .iter()
            .rev()
            .copied()
            .find(|line| !line.trim().is_empty())
    }

    pub(super) fn is_within_tail(&self, index: usize, max_tail_len: usize) -> bool {
        self.contains_index(index) && self.lines.len() - index <= max_tail_len
    }

    pub(super) fn tail_contains(
        &self,
        index: usize,
        max_tail_len: usize,
        predicate: impl Fn(&str) -> bool,
    ) -> bool {
        self.is_within_tail(index, max_tail_len)
            && self.lines[index..].iter().any(|line| predicate(line))
    }

    pub(super) fn forward_gap_before_is_within(
        &self,
        before_index: usize,
        after_index: usize,
        max_gap: usize,
        predicate: impl Fn(&str) -> bool,
    ) -> bool {
        self.contains_index(before_index)
            && self.contains_index(after_index)
            && before_index <= after_index
            && after_index.saturating_sub(before_index) <= max_gap
            && self.lines[before_index + 1..after_index]
                .iter()
                .all(|line| predicate(line))
    }

    pub(super) fn forward_gap_before_all(
        &self,
        before_index: usize,
        after_index: usize,
        predicate: impl Fn(&str) -> bool,
    ) -> bool {
        self.contains_index(before_index)
            && self.contains_index(after_index)
            && before_index <= after_index
            && self.lines[before_index + 1..after_index]
                .iter()
                .all(|line| predicate(line))
    }

    pub(super) fn trailing_lines_after_are(
        &self,
        index: usize,
        mut predicate: impl FnMut(usize, &str, bool) -> bool,
    ) -> bool {
        if !self.contains_index(index) {
            return false;
        }

        let last_index = self.lines.len().saturating_sub(1);
        self.lines
            .iter()
            .enumerate()
            .skip(index + 1)
            .all(|(line_index, line)| predicate(line_index, line, line_index == last_index))
    }

    pub(super) fn trailing_lines_after_any(
        &self,
        index: usize,
        mut predicate: impl FnMut(&str) -> bool,
    ) -> bool {
        self.contains_index(index) && self.lines[index + 1..].iter().any(|line| predicate(line))
    }

    pub(super) fn gap_between_is_within(
        &self,
        first_index: usize,
        second_index: usize,
        max_gap: usize,
        predicate: impl Fn(&str) -> bool,
    ) -> bool {
        self.contains_index(first_index)
            && self.contains_index(second_index)
            && first_index.abs_diff(second_index) <= max_gap
            && self
                .lines_between(first_index, second_index)
                .iter()
                .all(|line| predicate(line))
    }

    fn contains_index(&self, index: usize) -> bool {
        index < self.lines.len()
    }

    fn lines_between(&self, first_index: usize, second_index: usize) -> &[&'a str] {
        let start = first_index.min(second_index) + 1;
        let end = first_index.max(second_index);
        &self.lines[start..end]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tail_helpers_anchor_to_line_count_from_index() {
        let frame = PaneOutputFrame::new("one\ntwo\nthree\n");

        assert!(frame.is_within_tail(1, 2));
        assert!(!frame.is_within_tail(0, 2));
        assert!(!frame.is_within_tail(3, 1));
        assert!(frame.tail_contains(1, 2, |line| line == "three"));
        assert!(!frame.tail_contains(3, 1, |_| true));
    }

    #[test]
    fn gap_helpers_check_exclusive_lines_between_anchors() {
        let frame = PaneOutputFrame::new("busy\n\nok\nprompt\nfooter\n");

        assert!(frame.forward_gap_before_is_within(0, 3, 4, |line| {
            line.trim().is_empty() || line == "ok"
        }));
        assert!(
            frame.gap_between_is_within(3, 0, 4, |line| { line.trim().is_empty() || line == "ok" })
        );
        assert!(!frame.forward_gap_before_is_within(3, 0, 4, |_| true));
        assert!(!frame.forward_gap_before_is_within(0, 5, 10, |_| true));
        assert!(!frame.gap_between_is_within(0, 5, 10, |_| true));
        assert!(
            frame.forward_gap_before_all(0, 3, |line| { line.trim().is_empty() || line == "ok" })
        );
        assert!(!frame.forward_gap_before_all(3, 0, |_| true));
    }

    #[test]
    fn window_and_trailing_helpers_reject_invalid_indexes() {
        let frame = PaneOutputFrame::new("one\n\ntwo\nthree");

        assert_eq!(frame.line(2), Some("two"));
        assert_eq!(frame.range(1, 3), Some(&["", "two"][..]));
        assert_eq!(frame.previous_nonblank_before(2), Some("one"));
        assert_eq!(frame.last_nonblank(), Some("three"));
        assert_eq!(frame.window_before(2, 4), Some(&["one", ""][..]));
        assert_eq!(frame.window_ending_at(2, 1), Some(&["", "two"][..]));
        assert!(
            frame.trailing_lines_after_are(2, |_, line, is_last| { is_last && line == "three" })
        );
        assert!(frame.trailing_lines_after_any(1, |line| line == "two"));

        assert_eq!(frame.line(4), None);
        assert_eq!(frame.range(3, 5), None);
        assert_eq!(frame.lines_before(4), None);
        assert_eq!(frame.lines_from(4), None);
        assert_eq!(frame.window_before(4, 1), None);
        assert_eq!(frame.window_ending_at(4, 1), None);
        assert!(!frame.trailing_lines_after_are(4, |_, _, _| true));
        assert!(!frame.trailing_lines_after_any(4, |_| true));
    }
}
