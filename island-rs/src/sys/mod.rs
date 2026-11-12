//! Event sources. Each one reports to the window thread; none of them polls.

pub mod audio;
pub mod bluetooth;
pub mod clipboard;
pub mod drives;
pub mod keys;
pub mod network;
pub mod power;
pub mod privacy;
pub mod radio;
pub mod telemetry;
pub mod media;
pub mod weather;
