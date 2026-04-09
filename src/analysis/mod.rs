//! Analysis pipeline (DESIGN.md §11, §12, §16).
//!
//! Contains the cache/TLB boundary detection algorithms (hybrid X-Ray +
//! Calibrator plateau/jump) and the bandwidth/MLP analysis.

pub mod cache;

// Future sub-modules:
// pub mod tlb;
// pub mod bandwidth;
// pub mod mlp;
