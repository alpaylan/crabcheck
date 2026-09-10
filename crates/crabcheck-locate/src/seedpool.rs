#[derive(Clone, Debug)]
pub struct Seed<T: Clone, F: Clone + Ord> {
    pub input: T,
    pub feedback: F,
    pub energy: usize,
}

pub struct SeedPool<T: Clone, F: Clone + Ord> {
    pub seeds: Vec<Seed<T, F>>,
    pub best_of_all_time: Option<Seed<T, F>>,
}

impl<T: Clone, F: Clone + Ord> SeedPool<T, F> {
    pub fn new() -> SeedPool<T, F> {
        SeedPool::default()
    }

    pub fn add_seed(&mut self, seed: Seed<T, F>) {
        if let Some(best) = &self.best_of_all_time {
            if seed.feedback > best.feedback {
                self.best_of_all_time = Some(seed.clone());
            }
        } else {
            self.best_of_all_time = Some(seed.clone());
        }

        self.seeds.push(seed);
    }

    pub fn best(&self) -> &Seed<T, F> {
        self.seeds.iter().max_by_key(|s| &s.feedback).unwrap()
    }

    pub fn worst(&self) -> &Seed<T, F> {
        self.seeds.iter().min_by_key(|s| &s.feedback).unwrap()
    }

    pub fn pop(&mut self) -> Option<Seed<T, F>> {
        if let Some((max_index, _)) = self.seeds.iter().enumerate().max_by_key(|(_, s)| &s.feedback)
        {
            let seed = self.seeds[max_index].clone();

            if seed.energy > 0 {
                self.seeds[max_index].energy -= 1;
            } else {
                self.seeds.remove(max_index);
            }
            Some(seed)
        } else {
            None
        }
    }

    pub fn size(&self) -> usize {
        self.seeds.len()
    }

    pub fn is_empty(&self) -> bool {
        self.seeds.is_empty()
    }
}

impl<T: Clone, F: Clone + Ord> Default for SeedPool<T, F> {
    fn default() -> SeedPool<T, F> {
        SeedPool { seeds: vec![], best_of_all_time: None }
    }
}
