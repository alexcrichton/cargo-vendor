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
    println!("status: {}", output.status);
    println!("stdout: ----------\n{}", String::from_utf8_lossy(&output.stdout));
    println!("stderr: ----------\n{}", String::from_utf8_lossy(&output.stderr));
    if !output.status.success() {
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

    run(&mut vendor(&dir));

    let lock = read(&dir.join("vendor/log/Cargo.toml"));
    assert!(lock.contains("version = \"0.3.5\""));

    assert_vendor_works(&dir);
}

fn add_vendor_config(dir: &Path) {
    file(&dir, ".cargo/config", r#"
        [source.crates-io]
        replace-with = 'vendor'

        [source.vendor]
        directory = 'vendor'
    "#);
}

fn assert_vendor_works(dir: &Path) {
    add_vendor_config(dir);
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
    run(&mut vendor(&dir));

    let lock = read(&dir.join("vendor/bitflags/Cargo.toml"));
    assert!(lock.contains("version = \"0.8.0\""));
}

#[test]
fn two_lockfiles() {
    let dir = dir();

    file(&dir, "foo/Cargo.toml", r#"
        [package]
        name = "foo"
        version = "0.1.0"

        [dependencies]
        bitflags = "=0.7.0"
    "#);
    file(&dir, "foo/src/lib.rs", "");
    file(&dir, "bar/Cargo.toml", r#"
        [package]
        name = "bar"
        version = "0.1.0"

        [dependencies]
        bitflags = "=0.8.0"
    "#);
    file(&dir, "bar/src/lib.rs", "");

    run(vendor(&dir).arg("-s").arg("foo/Cargo.toml")
                    .arg("-s").arg("bar/Cargo.toml"));

    let lock = read(&dir.join("vendor/bitflags/Cargo.toml"));
    assert!(lock.contains("version = \"0.8.0\""));
    let lock = read(&dir.join("vendor/bitflags-0.7.0/Cargo.toml"));
    assert!(lock.contains("version = \"0.7.0\""));

    add_vendor_config(&dir);
    run(Command::new("cargo").arg("build").current_dir(&dir.join("foo")));
    run(Command::new("cargo").arg("build").current_dir(&dir.join("bar")));
}

#[test]
fn revendor_with_config() {
    let dir = dir();

    file(&dir, "Cargo.toml", r#"
        [package]
        name = "foo"
        version = "0.1.0"

        [dependencies]
        bitflags = "=0.7.0"
    "#);
    file(&dir, "src/lib.rs", "");

    run(&mut vendor(&dir));
    let lock = read(&dir.join("vendor/bitflags/Cargo.toml"));
    assert!(lock.contains("version = \"0.7.0\""));

    add_vendor_config(&dir);
    file(&dir, "Cargo.toml", r#"
        [package]
        name = "foo"
        version = "0.1.0"

        [dependencies]
        bitflags = "=0.8.0"
    "#);
    run(&mut vendor(&dir));
    let lock = read(&dir.join("vendor/bitflags/Cargo.toml"));
    assert!(lock.contains("version = \"0.8.0\""));
}

#[test]
fn delete_old_crates() {
    let dir = dir();

    file(&dir, "Cargo.toml", r#"
        [package]
        name = "foo"
        version = "0.1.0"

        [dependencies]
        bitflags = "=0.7.0"
    "#);
    file(&dir, "src/lib.rs", "");

    run(&mut vendor(&dir));
    read(&dir.join("vendor/bitflags/Cargo.toml"));

    file(&dir, "Cargo.toml", r#"
        [package]
        name = "foo"
        version = "0.1.0"

        [dependencies]
        log = "=0.3.5"
    "#);

    run(&mut vendor(&dir));
    let lock = read(&dir.join("vendor/log/Cargo.toml"));
    assert!(lock.contains("version = \"0.3.5\""));
    assert!(!dir.join("vendor/bitflags/Cargo.toml").exists());
}

#[test]
fn ignore_files() {
    let dir = dir();

    file(&dir, "Cargo.toml", r#"
        [package]
        name = "foo"
        version = "0.1.0"

        [dependencies]
        url = "=1.4.1"
    "#);
    file(&dir, "src/lib.rs", "");

    run(&mut vendor(&dir));
    let csum = read(&dir.join("vendor/url/.cargo-checksum.json"));
    assert!(!csum.contains("\"Cargo.toml.orig\""));
}

#[test]
fn git_simple() {
    let dir = dir();

    file(&dir, "Cargo.toml", r#"
        [package]
        name = "foo"
        version = "0.1.0"

        [dependencies.futures]
        git = 'https://github.com/alexcrichton/futures-rs'
        rev = '03a0005cb6498e4330'
    "#);
    file(&dir, "src/lib.rs", "");

    run(&mut vendor(&dir));
    let csum = read(&dir.join("vendor/futures/.cargo-checksum.json"));
    assert!(csum.contains("\"package\":null"));
}

#[test]
fn git_duplicate() {
    let dir = dir();

    file(&dir, "Cargo.toml", r#"
        [package]
        name = "foo"
        version = "0.1.0"

        [dependencies]
        futures = "=0.1.15"

        [dependencies.futures-cpupool]
        git = 'https://github.com/alexcrichton/futures-rs'
        rev = '03a0005cb6498e4330'
    "#);
    file(&dir, "src/lib.rs", "");

    let output = vendor(&dir).output().unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("found duplicate version of package `futures v0.1.15`"));
}

#[test]
fn git_only() {
    let dir = dir();

    file(&dir, "Cargo.toml", r#"
        [package]
        name = "foo"
        version = "0.1.0"

        [dependency]
        log = "=0.3.5"

        [dependencies.futures]
        git = 'https://github.com/alexcrichton/futures-rs'
        rev = '03a0005cb6498e4330'
    "#);
    file(&dir, "src/lib.rs", "");

    run(&mut vendor(&dir));
    let output = vendor(&dir)
        .arg("--only-git")
        .output()
        .expect("failed to run cargo-vendor");
    if output.status.success() {
        panic!("expected a failure");
    }

    assert!(dir.join("vendor/futures").is_dir());
    assert!(!dir.join("vendor/log").exists());

    let csum = read(&dir.join("vendor/futures/.cargo-checksum.json"));
    assert!(csum.contains("\"package\":null"));

}

#[test]
fn two_versions_disallowed() {
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

    let output = vendor(&dir)
        .arg("--disallow-duplicates")
        .output()
        .expect("failed to run cargo-vendor");
    if output.status.success() {
        panic!("expected a failure");
    }
}

#[test]
fn depend_on_vendor_dir_not_deleted() {
    let dir = dir();

    file(&dir, "Cargo.toml", r#"
        [package]
        name = "foo"
        version = "0.1.0"

        [dependencies]
        libc = "=0.2.30"
    "#);
    file(&dir, "src/lib.rs", "");

    run(&mut vendor(&dir));

    file(&dir, "Cargo.toml", r#"
        [package]
        name = "foo"
        version = "0.1.0"

        [dependencies]
        libc = "=0.2.30"

        [patch.crates-io]
        libc = { path = 'vendor/libc' }
    "#);

    run(&mut vendor(&dir));
    assert!(dir.join("vendor/libc").is_dir());
}
