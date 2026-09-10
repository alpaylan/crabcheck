use std::collections::{
    HashMap,
    HashSet,
};

use {
    rand::{
        RngCore,
        SeedableRng,
    },
    rand_chacha::ChaCha8Rng,
};

#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash, Ord, PartialOrd)]
pub struct Id(pub usize);

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Operand {
    Const(usize),
    FromId(Id),
}

impl From<usize> for Operand {
    fn from(value: usize) -> Self {
        Self::Const(value)
    }
}

impl Operand {
    fn referenced_id(&self) -> Option<Id> {
        match self {
            Self::Const(_) => None,
            Self::FromId(id) => Some(*id),
        }
    }

    fn resolve(&self, values: &HashMap<Id, usize>) -> Option<usize> {
        match self {
            Self::Const(value) => Some(*value),
            Self::FromId(id) => values.get(id).copied(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ChooseDecision {
    pub id: Id,
    pub lo: Operand,
    pub hi: Operand,
    pub picked: usize,
    pub extra_deps: Vec<Id>,
}

impl ChooseDecision {
    pub fn new(id: Id, lo: Operand, hi: Operand, picked: usize, extra_deps: Vec<Id>) -> Self {
        Self { id, lo, hi, picked, extra_deps }
    }

    fn dependencies(&self) -> Vec<Id> {
        let mut deps = Vec::new();
        if let Some(id) = self.lo.referenced_id() {
            deps.push(id);
        }
        if let Some(id) = self.hi.referenced_id() {
            deps.push(id);
        }
        deps.extend(self.extra_deps.iter().copied());
        deps.sort_unstable();
        deps.dedup();
        deps
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ChoiceTrace {
    decisions: Vec<ChooseDecision>,
}

impl ChoiceTrace {
    pub fn new(decisions: Vec<ChooseDecision>) -> Self {
        Self { decisions }
    }

    pub fn decisions(&self) -> &[ChooseDecision] {
        &self.decisions
    }

    pub fn len(&self) -> usize {
        self.decisions.len()
    }

    pub fn is_empty(&self) -> bool {
        self.decisions.is_empty()
    }

    pub fn is_well_formed(&self) -> bool {
        self.evaluated_ranges().is_some()
    }

    pub fn remove_with_dependents(&self, root: Id) -> Self {
        let mut removed = HashSet::from([root]);
        loop {
            let mut changed = false;
            for decision in &self.decisions {
                if removed.contains(&decision.id) {
                    continue;
                }
                if decision.dependencies().iter().any(|dep| removed.contains(dep)) {
                    removed.insert(decision.id);
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }

        let decisions = self
            .decisions
            .iter()
            .filter(|decision| !removed.contains(&decision.id))
            .cloned()
            .collect();
        Self { decisions }
    }

    fn evaluated_ranges(&self) -> Option<Vec<(usize, usize)>> {
        let mut seen = HashSet::new();
        let mut values = HashMap::new();
        let mut ranges = Vec::with_capacity(self.decisions.len());

        for decision in &self.decisions {
            if !seen.insert(decision.id) {
                return None;
            }

            for dep in &decision.extra_deps {
                if !values.contains_key(dep) {
                    return None;
                }
            }

            let lo = decision.lo.resolve(&values)?;
            let hi = decision.hi.resolve(&values)?;
            if lo > hi {
                return None;
            }
            if decision.picked < lo || decision.picked > hi {
                return None;
            }

            values.insert(decision.id, decision.picked);
            ranges.push((lo, hi));
        }

        Some(ranges)
    }

    fn range_at(&self, index: usize) -> Option<(usize, usize)> {
        self.evaluated_ranges().and_then(|ranges| ranges.get(index).copied())
    }
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct TracedUsize {
    pub id: Id,
    pub value: usize,
}

impl TracedUsize {
    pub fn as_operand(self) -> Operand {
        Operand::FromId(self.id)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TraceError {
    MissingDependency { id: Id, dependency: Id },
    MissingPlannedDecision { id: Id },
    InvalidRange { id: Id, lo: usize, hi: usize },
    PickOutOfRange { id: Id, picked: usize, lo: usize, hi: usize },
    DuplicateDecisionId { id: Id },
}

enum TraceMode {
    Recording,
    Replay { by_id: HashMap<Id, ChooseDecision> },
}

pub struct TraceRunner {
    mode: TraceMode,
    rng: ChaCha8Rng,
    values: HashMap<Id, usize>,
    emitted: Vec<ChooseDecision>,
    next_id: usize,
}

impl TraceRunner {
    pub fn recording(seed: u64) -> Self {
        Self {
            mode: TraceMode::Recording,
            rng: ChaCha8Rng::seed_from_u64(seed),
            values: HashMap::new(),
            emitted: Vec::new(),
            next_id: 1,
        }
    }

    pub fn replay(seed: u64, plan: &ChoiceTrace) -> Self {
        let by_id =
            plan.decisions.iter().cloned().map(|decision| (decision.id, decision)).collect();

        Self {
            mode: TraceMode::Replay { by_id },
            rng: ChaCha8Rng::seed_from_u64(seed),
            values: HashMap::new(),
            emitted: Vec::new(),
            next_id: 1,
        }
    }

    pub fn choose_usize(
        &mut self,
        lo: Operand,
        hi: Operand,
        extra_deps: &[Id],
    ) -> Result<TracedUsize, TraceError> {
        let id = Id(self.next_id);
        self.next_id += 1;
        self.choose_usize_with_id(id, lo, hi, extra_deps)
    }

    pub fn choose_usize_with_id(
        &mut self,
        id: Id,
        lo: Operand,
        hi: Operand,
        extra_deps: &[Id],
    ) -> Result<TracedUsize, TraceError> {
        // Always consume one RNG draw so replay stays deterministic even
        // when a decision is taken from the planned trace.
        let draw = self.rng.next_u64();

        let planned_pick = match &self.mode {
            TraceMode::Recording => None,
            TraceMode::Replay { by_id } => by_id.get(&id).map(|decision| decision.picked),
        };
        let is_planned = planned_pick.is_some();
        let mut decision = ChooseDecision::new(id, lo, hi, 0, extra_deps.to_vec());

        if self.values.contains_key(&id) {
            return Err(TraceError::DuplicateDecisionId { id });
        }

        for dep in &decision.extra_deps {
            if !self.values.contains_key(dep) {
                return Err(TraceError::MissingDependency { id, dependency: *dep });
            }
        }

        let resolved_lo = decision.lo.resolve(&self.values).ok_or_else(|| {
            TraceError::MissingDependency {
                id,
                dependency: decision.lo.referenced_id().expect("referenced id must exist"),
            }
        })?;
        let resolved_hi = decision.hi.resolve(&self.values).ok_or_else(|| {
            TraceError::MissingDependency {
                id,
                dependency: decision.hi.referenced_id().expect("referenced id must exist"),
            }
        })?;

        if resolved_lo > resolved_hi {
            return Err(TraceError::InvalidRange { id, lo: resolved_lo, hi: resolved_hi });
        }

        let picked = if is_planned {
            let picked = planned_pick.expect("planned pick should exist when is_planned=true");
            if picked < resolved_lo || picked > resolved_hi {
                return Err(TraceError::PickOutOfRange {
                    id,
                    picked,
                    lo: resolved_lo,
                    hi: resolved_hi,
                });
            }
            picked
        } else {
            match &self.mode {
                TraceMode::Recording => sample_from_draw(draw, resolved_lo, resolved_hi),
                TraceMode::Replay { .. } => return Err(TraceError::MissingPlannedDecision { id }),
            }
        };

        decision.picked = picked;
        self.values.insert(id, picked);
        self.emitted.push(decision);

        Ok(TracedUsize { id, value: picked })
    }

    pub fn finish(self) -> ChoiceTrace {
        ChoiceTrace::new(self.emitted)
    }
}

pub fn shrink_trace<F>(initial: ChoiceTrace, mut fails: F) -> ChoiceTrace
where
    F: FnMut(&ChoiceTrace) -> bool,
{
    if !fails(&initial) {
        return initial;
    }

    let mut current = initial;
    loop {
        let mut changed = false;

        let ids = current.decisions.iter().map(|decision| decision.id).collect::<Vec<_>>();
        for id in ids {
            let candidate = current.remove_with_dependents(id);
            if candidate.len() >= current.len() {
                continue;
            }
            if !candidate.is_well_formed() {
                continue;
            }
            if fails(&candidate) {
                current = candidate;
                changed = true;
                break;
            }
        }

        if changed {
            continue;
        }

        for index in 0..current.decisions.len() {
            if try_shrink_decision_at(&mut current, index, &mut fails) {
                changed = true;
                break;
            }
        }

        if !changed {
            return current;
        }
    }
}

fn try_shrink_decision_at<F>(trace: &mut ChoiceTrace, index: usize, fails: &mut F) -> bool
where
    F: FnMut(&ChoiceTrace) -> bool,
{
    let (resolved_lo, _) = match trace.range_at(index) {
        Some(range) => range,
        None => return false,
    };

    let picked = trace.decisions[index].picked;
    for new_picked in resolved_lo..picked {
        let mut candidate = trace.clone();
        candidate.decisions[index].picked = new_picked;
        if !candidate.is_well_formed() {
            continue;
        }
        if fails(&candidate) {
            *trace = candidate;
            return true;
        }
    }

    if let Operand::Const(hi_const) = trace.decisions[index].hi {
        let min_hi = resolved_lo.max(trace.decisions[index].picked);
        for new_hi in min_hi..hi_const {
            let mut candidate = trace.clone();
            candidate.decisions[index].hi = Operand::Const(new_hi);
            if !candidate.is_well_formed() {
                continue;
            }
            if fails(&candidate) {
                *trace = candidate;
                return true;
            }
        }
    }

    if let Operand::Const(lo_const) = trace.decisions[index].lo {
        for new_lo in 0..lo_const {
            let mut candidate = trace.clone();
            candidate.decisions[index].lo = Operand::Const(new_lo);
            if !candidate.is_well_formed() {
                continue;
            }
            if fails(&candidate) {
                *trace = candidate;
                return true;
            }
        }
    }

    false
}

fn sample_from_draw(draw: u64, lo: usize, hi: usize) -> usize {
    let span = (hi as u128) - (lo as u128) + 1;
    let offset = (draw as u128) % span;
    (lo as u128 + offset) as usize
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Tree<T> {
    Leaf(T),
    Node(T, Box<Tree<T>>, Box<Tree<T>>),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TrTree<T> {
    TrLeaf(Id, T),
    TrNode(Id, T, Box<TrTree<T>>, Box<TrTree<T>>),
}

impl TrTree<TracedUsize> {
    pub fn lift_back(self) -> Tree<usize> {
        match self {
            Self::TrLeaf(_, value) => Tree::Leaf(value.value),
            Self::TrNode(_, value, left, right) => {
                Tree::Node(value.value, Box::new(left.lift_back()), Box::new(right.lift_back()))
            },
        }
    }

    pub fn id(&self) -> Id {
        match self {
            Self::TrLeaf(id, _) | Self::TrNode(id, _, _, _) => *id,
        }
    }
}

impl TracedUsize {
    pub fn arbitrary_sized(
        runner: &mut TraceRunner,
        size: TracedUsize,
    ) -> Result<TracedUsize, TraceError> {
        runner.choose_usize(0.into(), size.as_operand(), &[size.id])
    }
}

impl TrTree<TracedUsize> {
    pub fn arbitrary_sized(
        runner: &mut TraceRunner,
        depth: usize,
        size: TracedUsize,
    ) -> Result<Self, TraceError> {
        let value = TracedUsize::arbitrary_sized(runner, size)?;
        let max_branch = if depth == 0 { 0 } else { 1 };
        let branch = runner.choose_usize(0.into(), max_branch.into(), &[size.id, value.id])?;

        if branch.value == 0 {
            Ok(Self::TrLeaf(branch.id, value))
        } else {
            let left_size =
                runner.choose_usize(0.into(), size.as_operand(), &[branch.id, size.id])?;
            let right_size =
                runner.choose_usize(0.into(), size.as_operand(), &[branch.id, size.id])?;
            let left = Self::arbitrary_sized(runner, depth - 1, left_size)?;
            let right = Self::arbitrary_sized(runner, depth - 1, right_size)?;
            Ok(Self::TrNode(branch.id, value, Box::new(left), Box::new(right)))
        }
    }
}

pub fn generate_traced_tree(
    seed: u64,
    max_size: usize,
    max_depth: usize,
) -> Result<(TrTree<TracedUsize>, ChoiceTrace), TraceError> {
    let mut runner = TraceRunner::recording(seed);
    let root_size = runner.choose_usize(0.into(), max_size.into(), &[])?;
    let tree = TrTree::<TracedUsize>::arbitrary_sized(&mut runner, max_depth, root_size)?;
    Ok((tree, runner.finish()))
}

pub fn replay_traced_tree(
    seed: u64,
    max_size: usize,
    max_depth: usize,
    trace: &ChoiceTrace,
) -> Result<(TrTree<TracedUsize>, ChoiceTrace), TraceError> {
    let mut runner = TraceRunner::replay(seed, trace);
    let root_size = runner.choose_usize(0.into(), max_size.into(), &[])?;
    let tree = TrTree::<TracedUsize>::arbitrary_sized(&mut runner, max_depth, root_size)?;
    Ok((tree, runner.finish()))
}

pub fn shrink_traced_tree<F>(
    seed: u64,
    max_size: usize,
    max_depth: usize,
    initial: ChoiceTrace,
    mut fails_tree: F,
) -> ChoiceTrace
where
    F: FnMut(&Tree<usize>) -> bool,
{
    shrink_trace(initial, |candidate| {
        let replayed = replay_traced_tree(seed, max_size, max_depth, candidate);
        match replayed {
            Ok((tree, _trace)) => fails_tree(&tree.lift_back()),
            Err(_) => false,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run_program(
        seed: u64,
        plan: Option<&ChoiceTrace>,
    ) -> Result<(usize, ChoiceTrace), TraceError> {
        let mut runner = match plan {
            Some(trace) => TraceRunner::replay(seed, trace),
            None => TraceRunner::recording(seed),
        };

        let size = runner.choose_usize(0.into(), 10.into(), &[])?;
        let branch = runner.choose_usize(0.into(), 1.into(), &[size.id])?;
        let child = if branch.value == 1 {
            runner.choose_usize(0.into(), size.as_operand(), &[branch.id])?.value
        } else {
            0
        };

        Ok((size.value + child, runner.finish()))
    }

    #[test]
    fn explicit_duplicate_id_is_rejected() {
        let mut runner = TraceRunner::recording(1);
        let first = runner
            .choose_usize_with_id(Id(42), 0.into(), 10.into(), &[])
            .expect("first explicit id should succeed");
        assert_eq!(first.id, Id(42));

        let err = runner
            .choose_usize_with_id(Id(42), 0.into(), 10.into(), &[])
            .expect_err("duplicate explicit id should fail");
        assert_eq!(err, TraceError::DuplicateDecisionId { id: Id(42) });
    }

    #[test]
    fn replay_with_removed_explicit_id_fails_strictly() {
        let mut recorder = TraceRunner::recording(9);
        let root = recorder
            .choose_usize_with_id(Id(1), 1.into(), 6.into(), &[])
            .expect("root choice should be recorded");
        let maybe_removed = recorder
            .choose_usize_with_id(Id(2), 0.into(), root.as_operand(), &[root.id])
            .expect("middle choice should be recorded");
        let leaf = recorder
            .choose_usize_with_id(Id(3), maybe_removed.as_operand(), 10.into(), &[maybe_removed.id])
            .expect("leaf choice should be recorded");
        let recorded = recorder.finish();
        assert_eq!(recorded.len(), 3);

        let shrunk_plan = recorded.remove_with_dependents(Id(2));
        assert_eq!(shrunk_plan.decisions().iter().map(|d| d.id).collect::<Vec<_>>(), vec![Id(1)]);

        let mut replay = TraceRunner::replay(9, &shrunk_plan);
        let replay_root = replay
            .choose_usize_with_id(Id(1), 1.into(), 6.into(), &[])
            .expect("root should come from plan");
        assert_eq!(replay_root.id, root.id);
        assert_eq!(replay_root.value, root.value);

        let err = replay
            .choose_usize_with_id(Id(2), 0.into(), replay_root.as_operand(), &[replay_root.id])
            .expect_err("missing explicit id should fail in strict replay");
        assert_eq!(err, TraceError::MissingPlannedDecision { id: maybe_removed.id });
        let replayed_trace = replay.finish();
        assert_eq!(replayed_trace.len(), 1);
        assert_eq!(leaf.id, Id(3));
    }

    #[test]
    fn remove_with_dependents_cascades() {
        let trace = ChoiceTrace::new(vec![
            ChooseDecision::new(Id(1), 0.into(), 10.into(), 8, vec![]),
            ChooseDecision::new(Id(2), 0.into(), 1.into(), 1, vec![Id(1)]),
            ChooseDecision::new(Id(3), 0.into(), Operand::FromId(Id(1)), 4, vec![Id(2)]),
        ]);

        let candidate = trace.remove_with_dependents(Id(2));
        let ids = candidate.decisions().iter().map(|decision| decision.id).collect::<Vec<_>>();
        assert_eq!(ids, vec![Id(1)]);
        assert!(candidate.is_well_formed());
    }

    #[test]
    fn replay_is_deterministic_with_seed_and_trace() {
        let (original_output, original_trace) =
            run_program(7, None).expect("recording should succeed");
        let (replayed_output, replayed_trace) =
            run_program(7, Some(&original_trace)).expect("replay should succeed");

        assert_eq!(original_output, replayed_output);
        assert_eq!(original_trace, replayed_trace);
    }

    #[test]
    fn shrink_trace_removes_choices_and_minimizes_values() {
        let initial = ChoiceTrace::new(vec![
            ChooseDecision::new(Id(1), 0.into(), 20.into(), 15, vec![]),
            ChooseDecision::new(Id(2), 0.into(), 1.into(), 1, vec![Id(1)]),
            ChooseDecision::new(Id(3), 0.into(), Operand::FromId(Id(1)), 10, vec![Id(2)]),
        ]);

        let shrunk = shrink_trace(initial, |trace| {
            if !trace.is_well_formed() {
                return false;
            }

            let root = trace
                .decisions()
                .iter()
                .find(|decision| decision.id == Id(1))
                .map(|decision| decision.picked)
                .unwrap_or(0);

            root >= 4
        });

        assert_eq!(shrunk.len(), 1);
        let root = &shrunk.decisions()[0];
        assert_eq!(root.id, Id(1));
        assert_eq!(root.picked, 4);
        assert_eq!(root.hi, Operand::Const(4));
    }

    #[test]
    fn traced_tree_record_and_replay_match() {
        let (tree, trace) = generate_traced_tree(17, 12, 3).expect("recording should succeed");
        let (replayed_tree, replayed_trace) =
            replay_traced_tree(17, 12, 3, &trace).expect("replay should succeed");

        assert_eq!(tree, replayed_tree);
        assert_eq!(trace, replayed_trace);
    }

    #[test]
    fn traced_tree_shrinking_reduces_choices() {
        let (_tree, trace) = generate_traced_tree(9, 20, 4).expect("recording should succeed");
        let shrunk = shrink_traced_tree(9, 20, 4, trace.clone(), |_candidate| true);

        assert!(shrunk.len() <= trace.len());
        let (shrunk_tree, _shrunk_trace) =
            replay_traced_tree(9, 20, 4, &shrunk).expect("replaying shrunk trace should succeed");
        let _shrunk_plain = shrunk_tree.lift_back();
    }
}
