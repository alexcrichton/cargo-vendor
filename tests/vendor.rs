use std::env;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering, ATOMIC_USIZE_INIT};

fn vendor(dir: &Path) -> Command {
    let mut me = env::current_exe().unwrap();
    me.pop();
    if me.ends_with("deps") {
        me.pop();
    }
    me.push("cargo-vendor");
    let mut cmd = Command::new(&me);
    cmd.arg("vendor");
    cmd.current_dir(dir);
    return cmd
}

static CNT: AtomicUsize = ATOMIC_USIZE_INIT;

fn dir() -> PathBuf {
    let i = CNT.fetch_add(1, Ordering::SeqCst);
    let mut dir = env::current_exe().unwrap();
    dir.pop();
    if dir.ends_with("deps") {
        dir.pop();
    }
    dir.pop();
    dir.push("tmp");
    drop(fs::create_dir(&dir));
    dir.push(&format!("test{}", i));
    drop(fs::remove_dir_all(&dir));
    fs::create_dir(&dir).unwrap();
    return dir
}

fn file(dir: &Path, path: &str, contents: &str) {
    let path = dir.join(path);
    println!("writing {:?}", path);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    File::create(path).unwrap().write_all(contents.as_bytes()).unwrap();
}

fn read(path: &Path) -> String {
    let mut contents = String::new();
    File::open(path).unwrap().read_to_string(&mut contents).unwrap();
	contents
}

fn run(cmd: &mut Command) -> (String, String) {
    println!("running {:?}", cmd);
    let output = cmd.output().unwrap();
    if !output.status.success() {
        println!("stdout: ----------\n{}", String::from_utf8_lossy(&output.stdout));
        println!("stderr: ----------\n{}", String::from_utf8_lossy(&output.stderr));
        panic!("not successful: {}", output.status);
    }

    (String::from_utf8(output.stdout).unwrap(),
     String::from_utf8(output.stderr).unwrap())
}

#[test]
fn vendor_simple() {
    let dir = dir();

    file(&dir, "Cargo.toml", r#"
        [package]
        name = "foo"
        version = "0.1.0"

        [dependencies]
        log = "=0.3.5"
    "#);
    file(&dir, "src/lib.rs", "");

    run(Command::new("cargo").arg("generate-lockfile").current_dir(&dir));
    run(&mut vendor(&dir));

    let lock = read(&dir.join("vendor/log/Cargo.toml"));
    assert!(lock.contains("version = \"0.3.5\""));

    assert_vendor_works(&dir);
}

fn assert_vendor_works(dir: &Path) {
    file(&dir, ".cargo/config", r#"
        [source.crates-io]
        replace-with = 'vendor'

        [source.vendor]
        directory = 'vendor'
    "#);
    run(Command::new("cargo").arg("build").current_dir(&dir));
}

#[test]
fn two_versions() {
    let dir = dir();

    file(&dir, "Cargo.toml", r#"
        [package]
        name = "foo"
        version = "0.1.0"

        [dependencies]
        bitflags = "=0.8.0"
        bar = { path = "bar" }
    "#);
    file(&dir, "src/lib.rs", "");
    file(&dir, "bar/Cargo.toml", r#"
        [package]
        name = "bar"
        version = "0.1.0"

        [dependencies]
        bitflags = "=0.7.0"
    "#);
    file(&dir, "bar/src/lib.rs", "");

    run(Command::new("cargo").arg("generate-lockfile").current_dir(&dir));
    run(&mut vendor(&dir));

    let lock = read(&dir.join("vendor/bitflags/Cargo.toml"));
    assert!(lock.contains("version = \"0.8.0\""));
    let lock = read(&dir.join("vendor/bitflags-0.7.0/Cargo.toml"));
    assert!(lock.contains("version = \"0.7.0\""));

    assert_vendor_works(&dir);
}

#[test]
fn help() {
    run(vendor(Path::new(".")).arg("--help"));
}

#[test]
fn update_versions() {
    let dir = dir();

    file(&dir, "Cargo.toml", r#"
        [package]
        name = "foo"
        version = "0.1.0"

        [dependencies]
        bitflags = "=0.7.0"
    "#);
    file(&dir, "src/lib.rs", "");

    run(Command::new("cargo").arg("generate-lockfile").current_dir(&dir));
    run(&mut vendor(&dir));

    let lock = read(&dir.join("vendor/bitflags/Cargo.toml"));
    assert!(lock.contains("version = \"0.7.0\""));

    file(&dir, "Cargo.toml", r#"
        [package]
        name = "foo"
        version = "0.1.0"

        [dependencies]
        bitflags = "=0.8.0"
    "#);
    run(Command::new("cargo").arg("generate-lockfile").current_dir(&dir));
    run(&mut vendor(&dir));

    let lock = read(&dir.join("vendor/bitflags/Cargo.toml"));
    assert!(lock.contains("version = \"0.8.0\""));
}
