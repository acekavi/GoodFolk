//! Video finishing: locates the ffmpeg and ffprobe the pipeline runs, probes and transcodes video uploads, and scores
//! renditions with VMAF.

pub mod probe;
mod run;
pub mod tools;
mod transcode;
mod vmaf;

pub use probe::{Probe, Refusal, VideoError, accept, probe};
pub use transcode::{Encoded, Options, poster_png, transcode};
pub use vmaf::{VmafScore, vmaf};
