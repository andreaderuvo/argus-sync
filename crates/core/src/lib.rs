//! argus-sync's engine: a folder on this computer kept in two-way sync with a folder on an Argus
//! server, through a sync key that opens that folder and nothing else.

pub mod engine;
pub mod local;
pub mod names;
pub mod pair;
pub mod plan;
pub mod remote;
pub mod state;
