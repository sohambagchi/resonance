//! Resonance — memory hierarchy characterisation library.
//!
//! This is the library root.  The binary (`main.rs`) provides the CLI
//! wrapper; everything testable lives here.

pub mod analysis;
pub mod arch;
pub mod buffer;
pub mod constants;
pub mod kernels;
pub mod oracle;
pub mod orchestrator;
pub mod platform;
pub mod results;
pub mod rng;
pub mod timer;
