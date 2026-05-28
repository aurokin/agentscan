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
    }
}
