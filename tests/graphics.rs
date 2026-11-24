use std::{fs, process::Command};
fn run(name: &str) {
    let root = std::env::temp_dir().join(format!("luauxtx-gfx-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&root).unwrap();
    image::RgbaImage::from_pixel(2, 2, image::Rgba([255, 255, 255, 255]))
        .save(root.join("pixel.png"))
        .unwrap();
    fs::write(root.join("bad.png"), "not an image").unwrap();
    let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/gfx")
        .join(name);
    let mut child = Command::new(env!("CARGO_BIN_EXE_luauxtx"))
        .arg(script)
        .current_dir(&root)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let start = std::time::Instant::now();
    loop {
        if child.try_wait().unwrap().is_some() {
            break;
        }
        if start.elapsed().as_secs() > 30 {
            child.kill().unwrap();
            let output = child.wait_with_output().unwrap();
            fs::remove_dir_all(&root).unwrap();
            panic!(
                "graphics CLI timed out\nstdout: {}\nstderr: {}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let output = child.wait_with_output().unwrap();
    fs::remove_dir_all(root).unwrap();
    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
#[test]
fn graphics_resources_and_validation_work_without_a_display() {
    run("validation.luau");
}
#[test]
fn desktop_startup_errors_are_catchable() {
    run("headless.luau");
}
#[test]
#[ignore = "requires an interactive desktop or Xvfb and a GPU/software adapter"]
fn native_windows_resize_frames_and_cleanup() {
    run("live.luau");
}
#[test]
#[ignore = "requires an interactive desktop or Xvfb and a GPU/software adapter"]
fn callback_failure_closes_native_resources() {
    run("failure.luau");
}
