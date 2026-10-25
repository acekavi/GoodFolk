//! Video and PDF finishing: locates the external tools (ffmpeg, ffprobe, qpdf, PDFium) the pipeline runs.

pub mod probe;
pub mod tools;

pub use probe::{Probe, Refusal, VideoError, accept, probe};
