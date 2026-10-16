//! Writes the gallery renditions of one image, as media-svc would make them, for the web app's gallery paint
//! gate: `cargo run --release -p imaging --example renditions -- <image> <out dir>` writes `{size}.{format}` for
//! each rendition and prints `size format width height` lines.

use std::{error::Error, fs, path::Path};

use imaging::{Profile, SourceType, process, sniff};

fn main() -> Result<(), Box<dyn Error>> {
    let mut args = std::env::args().skip(1);
    let (Some(source), Some(out)) = (args.next(), args.next()) else {
        return Err("usage: renditions <image> <out dir>".into());
    };
    let bytes = fs::read(&source)?;
    let ty: SourceType = sniff(&bytes).ok_or("not a supported image")?;
    let processed = process(&bytes, ty, Profile::Gallery)?;
    fs::create_dir_all(&out)?;
    for r in &processed.renditions {
        fs::write(Path::new(&out).join(format!("{}.{}", r.size.as_str(), r.format.as_str())), &r.bytes)?;
        println!("{} {} {} {}", r.size.as_str(), r.format.as_str(), r.width, r.height);
    }
    Ok(())
}
