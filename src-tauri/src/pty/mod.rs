//! Agent processes in ConPTYs: `manager` (spawn/write/resize/kill), `job`
//! (Job Object per agent), `batcher` (coalesced output), `ring` (scrollback),
//! `modes` (terminal modes restored on replay).

pub mod batcher;
pub mod job;
pub mod manager;
pub mod modes;
pub mod ring;

pub use manager::{PtyManager, SpawnRequest};
