#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DispatchSequenceItem<Candidate> {
    pub candidate_index: u32,
    pub retry_index: u32,
    pub candidate: Candidate,
    pub mark: DispatchSequenceMark,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum DispatchSequenceMark {
    Pending,
    Failed,
    Succeeded,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DispatchSequence<Candidate> {
    items: Vec<DispatchSequenceItem<Candidate>>,
    cursor: usize,
}

impl<Candidate> DispatchSequence<Candidate> {
    pub fn new(items: Vec<DispatchSequenceItem<Candidate>>) -> Self {
        Self { items, cursor: 0 }
    }

    pub fn from_candidates(candidates: Vec<Candidate>) -> Self {
        let items = candidates
            .into_iter()
            .enumerate()
            .map(|(index, candidate)| DispatchSequenceItem {
                candidate_index: u32::try_from(index).unwrap_or(u32::MAX),
                retry_index: 0,
                candidate,
                mark: DispatchSequenceMark::Pending,
            })
            .collect();
        Self::new(items)
    }

    pub fn peek_current(&self) -> Option<&DispatchSequenceItem<Candidate>> {
        self.items.get(self.cursor)
    }

    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> Option<&DispatchSequenceItem<Candidate>> {
        while self
            .items
            .get(self.cursor)
            .is_some_and(|item| item.mark != DispatchSequenceMark::Pending)
        {
            self.cursor = self.cursor.saturating_add(1);
        }
        self.items.get(self.cursor)
    }

    pub fn mark_failed(&mut self) -> Option<&DispatchSequenceItem<Candidate>> {
        self.mark_current(DispatchSequenceMark::Failed)
    }

    pub fn mark_succeeded(&mut self) -> Option<&DispatchSequenceItem<Candidate>> {
        self.mark_current(DispatchSequenceMark::Succeeded)
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn items(&self) -> &[DispatchSequenceItem<Candidate>] {
        &self.items
    }

    fn mark_current(
        &mut self,
        mark: DispatchSequenceMark,
    ) -> Option<&DispatchSequenceItem<Candidate>> {
        // Sequences can be restored with terminal attempts before the cursor.
        // Never let a later completion overwrite that durable outcome.
        while self
            .items
            .get(self.cursor)
            .is_some_and(|item| item.mark != DispatchSequenceMark::Pending)
        {
            self.cursor = self.cursor.saturating_add(1);
        }
        let item = self.items.get_mut(self.cursor)?;
        item.mark = mark;
        self.cursor = self.cursor.saturating_add(1);
        while self
            .items
            .get(self.cursor)
            .is_some_and(|item| item.mark != DispatchSequenceMark::Pending)
        {
            self.cursor = self.cursor.saturating_add(1);
        }
        self.items.get(self.cursor)
    }
}

#[cfg(test)]
mod tests {
    use super::{DispatchSequence, DispatchSequenceMark};

    #[test]
    fn mark_failed_advances_without_reordering() {
        let mut sequence = DispatchSequence::from_candidates(vec!["a", "b", "c"]);

        assert_eq!(sequence.next().map(|item| item.candidate), Some("a"));
        assert_eq!(sequence.mark_failed().map(|item| item.candidate), Some("b"));
        assert_eq!(sequence.next().map(|item| item.candidate), Some("b"));
        assert_eq!(sequence.mark_failed().map(|item| item.candidate), Some("c"));
        assert_eq!(sequence.next().map(|item| item.candidate), Some("c"));

        assert_eq!(sequence.items()[0].mark, DispatchSequenceMark::Failed);
        assert_eq!(sequence.items()[1].mark, DispatchSequenceMark::Failed);
        assert_eq!(sequence.items()[2].mark, DispatchSequenceMark::Pending);
    }

    #[test]
    fn marking_a_restored_sequence_never_overwrites_a_terminal_attempt() {
        let mut sequence = DispatchSequence::new(vec![
            super::DispatchSequenceItem {
                candidate_index: 0,
                retry_index: 0,
                candidate: "already-succeeded",
                mark: DispatchSequenceMark::Succeeded,
            },
            super::DispatchSequenceItem {
                candidate_index: 1,
                retry_index: 0,
                candidate: "pending",
                mark: DispatchSequenceMark::Pending,
            },
        ]);

        assert_eq!(sequence.mark_failed(), None);
        assert_eq!(sequence.cursor(), 2);
        assert_eq!(sequence.items()[0].mark, DispatchSequenceMark::Succeeded);
        assert_eq!(sequence.items()[1].mark, DispatchSequenceMark::Failed);
    }

    #[test]
    fn marking_an_exhausted_sequence_preserves_all_terminal_attempts() {
        let mut sequence = DispatchSequence::new(vec![super::DispatchSequenceItem {
            candidate_index: 0,
            retry_index: 0,
            candidate: "already-succeeded",
            mark: DispatchSequenceMark::Succeeded,
        }]);

        assert_eq!(sequence.mark_failed(), None);
        assert_eq!(sequence.cursor(), 1);
        assert_eq!(sequence.items()[0].mark, DispatchSequenceMark::Succeeded);
    }

    #[test]
    fn marking_a_restored_sequence_returns_the_next_pending_attempt() {
        let mut sequence = DispatchSequence::new(vec![
            super::DispatchSequenceItem {
                candidate_index: 0,
                retry_index: 0,
                candidate: "already-succeeded",
                mark: DispatchSequenceMark::Succeeded,
            },
            super::DispatchSequenceItem {
                candidate_index: 1,
                retry_index: 0,
                candidate: "pending-a",
                mark: DispatchSequenceMark::Pending,
            },
            super::DispatchSequenceItem {
                candidate_index: 2,
                retry_index: 0,
                candidate: "already-failed",
                mark: DispatchSequenceMark::Failed,
            },
            super::DispatchSequenceItem {
                candidate_index: 3,
                retry_index: 0,
                candidate: "pending-b",
                mark: DispatchSequenceMark::Pending,
            },
        ]);

        assert_eq!(
            sequence.mark_failed().map(|item| item.candidate),
            Some("pending-b")
        );
        assert_eq!(sequence.cursor(), 3);
        assert_eq!(sequence.items()[1].mark, DispatchSequenceMark::Failed);
        assert_eq!(sequence.items()[2].mark, DispatchSequenceMark::Failed);
    }
}
