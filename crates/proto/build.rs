//! Compiles the media and jobs protos with the pure-Rust `protox`, so no system `protoc` is needed.

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let protos = ["../../proto/goodfolk/media/v1/media.proto", "../../proto/goodfolk/jobs/v1/jobs.proto"];
    let fds = protox::compile(protos, ["../../proto"])?;
    tonic_prost_build::configure().compile_fds(fds)?;
    for proto in protos {
        println!("cargo:rerun-if-changed={proto}");
    }
    Ok(())
}
