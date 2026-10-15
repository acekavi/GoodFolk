//! Compiles the media proto with the pure-Rust `protox`, so no system `protoc` is needed.

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let proto = "../../proto/goodfolk/media/v1/media.proto";
    let fds = protox::compile([proto], ["../../proto"])?;
    tonic_prost_build::configure().compile_fds(fds)?;
    println!("cargo:rerun-if-changed={proto}");
    Ok(())
}
