#[test]
fn ffmpeg_is_built_without_gpl() {
    let out = std::process::Command::new(video::tools::ffmpeg()).arg("-buildconf").output().unwrap();
    let conf = String::from_utf8_lossy(&out.stdout);
    assert!(!conf.contains("--enable-gpl"), "{conf}");
    assert!(!conf.contains("--enable-nonfree"), "{conf}");
    for lib in [
        "--enable-libsvtav1",
        "--enable-libopenh264",
        "--enable-libopus",
        "--enable-libdav1d",
        "--enable-libvmaf",
        "--enable-libzimg",
    ] {
        assert!(conf.contains(lib), "missing {lib}: {conf}");
    }
}

#[test]
fn ffprobe_runs() {
    assert!(std::process::Command::new(video::tools::ffprobe()).arg("-version").status().unwrap().success());
}
