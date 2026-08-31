#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SchedulerRequestSnapshot {
    generation: u64,
    ranking_seed: u64,
}

impl SchedulerRequestSnapshot {
    pub const fn new(generation: u64, ranking_seed: u64) -> Self {
        Self {
            generation,
            ranking_seed,
        }
    }

    pub const fn generation(self) -> u64 {
        self.generation
    }

    pub const fn ranking_seed(self) -> u64 {
        self.ranking_seed
    }

    pub const fn page_id(self, ordinal: u32) -> SchedulerPageId {
        // The seed is request-local ordering input; page identity remains
        // reusable by the generation-scoped resolved-page cache.
        SchedulerPageId {
            generation: self.generation,
            ordinal,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SchedulerPageId {
    generation: u64,
    ordinal: u32,
}

impl SchedulerPageId {
    pub const fn generation(self) -> u64 {
        self.generation
    }

    pub const fn ordinal(self) -> u32 {
        self.ordinal
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_snapshot_produces_stable_generation_scoped_page_ids() {
        let snapshot = SchedulerRequestSnapshot::new(17, 29);

        assert_eq!(snapshot.generation(), 17);
        assert_eq!(snapshot.ranking_seed(), 29);
        assert_eq!(snapshot.page_id(3), snapshot.page_id(3));
        assert_ne!(snapshot.page_id(3), snapshot.page_id(4));
        assert_ne!(
            snapshot.page_id(3),
            SchedulerRequestSnapshot::new(18, 29).page_id(3)
        );
        assert_eq!(
            snapshot.page_id(3),
            SchedulerRequestSnapshot::new(17, 30).page_id(3)
        );
    }
}
