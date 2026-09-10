//! Spectrum-Based Fault Localization metrics shared across analysis tools.
//!
//! Moved from `scripts/analysis.rs:158-208` so `crabcheck-profiling-analysis`
//! and `crabcheck-profiling-fast-analyze` use bit-identical formulas.

use serde::Serialize;

#[derive(Debug, Serialize, Clone, Copy)]
pub struct Suspiciousness {
    pub tarantula: f64,
    pub ochiai: f64,
    pub dstar: f64,
    pub jaccard: f64,
    pub op2: f64,
}

impl Suspiciousness {
    /// Compute SBFL scores from spectrum counts.
    ///
    /// - `ef`: failing snapshots where the region was hit
    /// - `ep`: passing snapshots where the region was hit
    /// - `nf`: failing snapshots where the region was NOT hit
    /// - `np`: passing snapshots where the region was NOT hit
    pub fn compute(ef: u64, ep: u64, nf: u64, np: u64) -> Self {
        let ef = ef as f64;
        let ep = ep as f64;
        let nf = nf as f64;
        let _np = np as f64;
        let f = ef + nf;
        let p = ep + _np;

        let tarantula = {
            let ef_over_f = if f > 0.0 { ef / f } else { 0.0 };
            let ep_over_p = if p > 0.0 { ep / p } else { 0.0 };
            let denom = ef_over_f + ep_over_p;
            if denom > 0.0 { ef_over_f / denom } else { 0.0 }
        };

        let ochiai = {
            let denom = (f * (ef + ep)).sqrt();
            if denom > 0.0 { ef / denom } else { 0.0 }
        };

        let dstar = {
            let denom = ep + nf;
            if denom > 0.0 {
                (ef * ef) / denom
            } else if ef > 0.0 {
                f64::MAX
            } else {
                0.0
            }
        };

        let jaccard = {
            let denom = ef + nf + ep;
            if denom > 0.0 { ef / denom } else { 0.0 }
        };

        let op2 = ef - ep / (p + 1.0);

        Suspiciousness { tarantula, ochiai, dstar, jaccard, op2 }
    }
}
