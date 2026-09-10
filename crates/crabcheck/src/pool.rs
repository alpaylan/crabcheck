//! The seed pool of the targeted phase: the best-scoring inputs seen so
//! far, each with an energy budget of mutations to spend on it.

use rand_core::RngCore;

use crate::source::uniform;

/// An input worth mutating.
#[derive(Clone, Debug)]
pub(crate) struct Seed<T> {
    pub value: T,
    pub score: f64,
    pub energy: u32,
}

/// A bounded collection of seeds ordered by score.
#[derive(Debug)]
pub(crate) struct Pool<T> {
    seeds: Vec<Seed<T>>,
    capacity: usize,
    energy: u32,
    /// The best-scoring input ever added, kept even after its energy is spent.
    pub best: Option<(f64, T)>,
    pub improvements: u64,
}

impl<T: Clone> Pool<T> {
    pub fn new(capacity: usize, energy: u32) -> Self {
        Pool {
            seeds: Vec::new(),
            capacity: capacity.max(1),
            energy: energy.max(1),
            best: None,
            improvements: 0,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.seeds.is_empty()
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.seeds.len()
    }

    /// The lowest score currently in the pool, if it is full.
    fn threshold(&self) -> Option<f64> {
        (self.seeds.len() >= self.capacity)
            .then(|| self.seeds.iter().map(|s| s.score).fold(f64::INFINITY, f64::min))
    }

    /// Whether an input with `score` would be worth adding.
    pub fn improves(&self, score: f64) -> bool {
        match self.threshold() {
            Some(worst) => score > worst,
            None => true,
        }
    }

    /// Add an input; evicts the worst seed when the pool is full. Returns
    /// whether it set a new overall best.
    pub fn add(&mut self, value: T, score: f64) -> bool {
        if !self.improves(score) {
            return false;
        }
        if self.seeds.len() >= self.capacity
            && let Some(worst) = self
                .seeds
                .iter()
                .enumerate()
                .min_by(|a, b| a.1.score.total_cmp(&b.1.score))
                .map(|(i, _)| i)
        {
            self.seeds.swap_remove(worst);
        }
        let new_best = self.best.as_ref().is_none_or(|(best, _)| score > *best);
        if new_best {
            self.best = Some((score, value.clone()));
            self.improvements += 1;
        }
        self.seeds.push(Seed { value, score, energy: self.energy });
        new_best
    }

    /// Add an input regardless of its score (coverage-guided search keeps
    /// inputs for the code they reach, not for their score). Evicts the
    /// worst seed when full.
    pub fn add_forced(&mut self, value: T, score: f64) {
        if self.seeds.len() >= self.capacity
            && let Some(worst) = self
                .seeds
                .iter()
                .enumerate()
                .min_by(|a, b| a.1.score.total_cmp(&b.1.score))
                .map(|(i, _)| i)
        {
            self.seeds.swap_remove(worst);
        }
        if self.best.as_ref().is_none_or(|(best, _)| score > *best) {
            self.best = Some((score, value.clone()));
            self.improvements += 1;
        }
        self.seeds.push(Seed { value, score, energy: self.energy });
    }

    /// Pick a seed to mutate, preferring high scores, and spend one unit of
    /// its energy. Seeds whose energy is spent are dropped; when every seed
    /// is spent the energies are refilled so the search keeps going.
    pub fn pick(&mut self, rng: &mut impl RngCore) -> Option<Seed<T>> {
        if self.seeds.is_empty() {
            return None;
        }
        if self.seeds.iter().all(|s| s.energy == 0) {
            for seed in &mut self.seeds {
                seed.energy = self.energy;
            }
        }
        let mut ranked: Vec<usize> =
            (0..self.seeds.len()).filter(|&i| self.seeds[i].energy > 0).collect();
        ranked.sort_by(|&a, &b| self.seeds[b].score.total_cmp(&self.seeds[a].score));
        let n = ranked.len() as u64;
        let total = n * (n + 1) / 2;
        let mut roll = uniform(rng, total - 1);
        let mut chosen = ranked[0];
        for (rank, &index) in ranked.iter().enumerate() {
            let weight = n - rank as u64;
            if roll < weight {
                chosen = index;
                break;
            }
            roll -= weight;
        }
        let seed = &mut self.seeds[chosen];
        seed.energy -= 1;
        let picked = seed.clone();
        if seed.energy == 0 && self.seeds.len() > 1 {
            self.seeds.swap_remove(chosen);
        }
        Some(picked)
    }

    /// Take every seed out of the pool (used to merge worker pools).
    pub fn drain(&mut self) -> Vec<Seed<T>> {
        std::mem::take(&mut self.seeds)
    }
}

#[cfg(test)]
mod tests {
    use {
        super::*,
        crate::Source,
    };

    #[test]
    fn keeps_the_best_and_evicts_the_worst() {
        let mut pool = Pool::new(2, 3);
        assert!(pool.add(1u8, 1.0));
        assert!(!pool.add(2, 0.5));
        assert!(pool.add(3, 2.0));
        assert!(!pool.improves(0.7), "pool is full of better seeds");
        assert!(!pool.add(4, 1.5), "added, but not a new overall best");
        assert_eq!(pool.len(), 2);
        assert!(!pool.improves(1.4) && pool.improves(1.6));
        assert_eq!(pool.best.as_ref().unwrap().0, 2.0);
        assert_eq!(pool.improvements, 2);
    }

    #[test]
    fn energy_is_spent_and_refilled() {
        let mut pool = Pool::new(4, 2);
        pool.add(1u8, 1.0);
        pool.add(2, 5.0);
        let mut rng = Source::new(1, 1);
        for _ in 0..20 {
            assert!(pool.pick(&mut rng).is_some(), "the pool never runs dry");
        }
        assert!(!pool.is_empty());
        let drained = pool.drain();
        assert!(!drained.is_empty());
        assert!(pool.is_empty());
    }
}
