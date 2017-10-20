extern crate cargo;
extern crate env_logger;
#[macro_use]
extern crate serde_derive;
#[macro_use]
extern crate serde_json;
extern crate toml;

use std::collections::{BTreeMap, HashMap, BTreeSet};
use std::env;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{PathBuf, Path};

use cargo::core::{Workspace, GitReference};
use cargo::CliResult;
use cargo::util::{Config, CargoResult, CargoResultExt};
use cargo::util::Sha256;

#[derive(Deserialize)]
struct Options {
    arg_path: Option<String>,
    flag_no_delete: Option<bool>,
    flag_version: bool,
    flag_sync: Option<Vec<String>>,
    flag_verbose: u32,
    flag_quiet: Option<bool>,
    flag_explicit_version: Option<bool>,
    flag_color: Option<String>,
    flag_frozen: bool,
    flag_locked: bool,
    flag_disallow_duplicates: bool,
}

#[derive(Serialize)]
struct VendorConfig {
    source: BTreeMap<String, VendorSource>,
}

#[derive(Serialize)]
#[serde(rename_all = "lowercase", untagged)]
enum VendorSource {
    Directory {
        directory: PathBuf,
    },
    Registry {
        registry: Option<String>,
        #[serde(rename = "replace-with")]
        replace_with: String,
    },
    Git {
        git: String,
        branch: Option<String>,
        tag: Option<String>,
        rev: Option<String>,
        #[serde(rename = "replace-with")]
        replace_with: String,
    },
}

fn main() {
    env_logger::init().unwrap();

    // We're doing the vendoring operation outselves, so we don't actually want
    // to respect any of the `source` configuration in Cargo itself. That's
    // intended for other consumers of Cargo, but we want to go straight to the
    // source, e.g. crates.io, to fetch crates.
    let config = {
        let config_orig = Config::default().unwrap();
        let mut values = config_orig.values().unwrap().clone();
        values.remove("source");
        let config = Config::default().unwrap();
        config.set_values(values).unwrap();
        config
    };

    let args = env::args().collect::<Vec<_>>();
    let result = cargo::call_main_without_stdin(real_main, &config, r#"
Vendor all dependencies for a project locally

Usage:
    cargo vendor [options] [<path>]

Options:
    -h, --help               Print this message
    -V, --version            Print version information
    -s, --sync TOML ...      Sync the `Cargo.toml` or `Cargo.lock` specified
    -v, --verbose ...        Use verbose output
    -q, --quiet              No output printed to stdout
    -x, --explicit-version   Always include version in subdir name
    --disallow-duplicates    Disallow two versions of one crate
    --no-delete              Don't delete older crates in the vendor directory
    --frozen                 Require Cargo.lock and cache are up to date
    --locked                 Require Cargo.lock is up to date
    --color WHEN             Coloring: auto, always, never

This cargo subcommand will vendor all crates.io dependencies for a project into
the specified directory at `<path>`. The `cargo vendor` command is intended to
be run in the same directory as `Cargo.toml`, but the manifest can also be
specified via the `--sync` flag. After this command completes the vendor
directory specified by `<path>` will contain all sources from crates.io
necessary to build the manifests specified.

The `cargo vendor` command will also print out the configuration necessary
to use the vendored sources, which when needed is then encoded into
`.cargo/config`.
"#, &args, false);

    if let Err(e) = result {
        cargo::exit_with_error(e, &mut *config.shell());
    }
}

fn real_main(options: Options, config: &Config) -> CliResult {
    if options.flag_version {
        println!("{} {}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"));
        return Ok(());
    }

    try!(config.configure(options.flag_verbose,
                          options.flag_quiet,
                          &options.flag_color,
                          options.flag_frozen,
                          options.flag_locked));

    let default = "vendor".to_string();
    let path = Path::new(options.arg_path.as_ref().unwrap_or(&default));

    try!(fs::create_dir_all(&path).chain_err(|| {
        format!("failed to create: `{}`", path.display())
    }));

    let workspaces = match options.flag_sync {
        Some(list) => {
            list.iter().map(|path| {
                let path = Path::new(path);
                let manifest = if path.ends_with("Cargo.lock") {
                    config.cwd().join(path.with_file_name("Cargo.toml"))
                } else {
                    config.cwd().join(path)
                };
                Workspace::new(&manifest, config)
            }).collect::<CargoResult<Vec<_>>>()?
        }
        None => {
            let manifest = config.cwd().join("Cargo.toml");
            vec![Workspace::new(&manifest, config)?]
        }
    };

    let vendor_config = try!(sync(
        &workspaces,
        &path,
        config,
        options.flag_explicit_version.unwrap_or(false),
        options.flag_no_delete.unwrap_or(false),
        options.flag_disallow_duplicates,
    ).chain_err(|| {
        "failed to sync"
    }));

    if !options.flag_quiet.unwrap_or(false) {
        eprint!("To use vendored sources, add this to your .cargo/config for this project:\n\n");
        print!("{}", &toml::to_string(&vendor_config).unwrap());
    }

    Ok(())
}

fn sync(workspaces: &[Workspace],
        local_dst: &Path,
        config: &Config,
        explicit_version: bool,
        no_delete: bool,
        disallow_duplicates: bool) -> CargoResult<VendorConfig> {
    let mut ids = BTreeMap::new();
    for ws in workspaces {
        let (packages, resolve) = try!(cargo::ops::resolve_ws(&ws).chain_err(|| {
            "failed to load pkg lockfile"
        }));

        for pkg in resolve.iter() {
            if pkg.source_id().is_path() {
                continue
            }
            ids.insert(pkg.clone(), packages.get(pkg).chain_err(|| {
                "failed to fetch package"
            })?.clone());
        }
    }

    // https://github.com/rust-lang/cargo/blob/373c5d8ce43691f90929a74b047d7eababd04379/src/cargo/sources/registry/mod.rs#L248

    let mut versions = HashMap::new();
    for id in ids.keys() {
        let map = versions.entry(id.name()).or_insert_with(BTreeMap::default);

        if let Some(prev) = map.get(&id.version()) {
            let err = format!("found duplicate version of package `{} v{}` \
                               vendored from two sources:\n\
                               \n\
                               \tsource 1: {}\n\
                               \tsource 2: {}",
                              id.name(),
                              id.version(),
                              prev,
                              id.source_id());
            return Err(err.into())
        }
        map.insert(id.version(), id.source_id());
    }

    let existing_crates = local_dst.read_dir().map(|iter| {
        iter.filter_map(|e| e.ok())
            .filter(|e| e.path().join("Cargo.toml").exists())
            .map(|e| e.path())
            .collect::<Vec<_>>()
    }).unwrap_or(Vec::new());

    let mut added_crates = Vec::new();
    let mut sources = BTreeSet::new();
    for (id, pkg) in ids.iter() {
        // Next up, copy it to the vendor directory
        let src = pkg.manifest_path().parent().expect("manifest_path should point to a file");
        let max_version = *versions[id.name()].iter().rev().next().unwrap().0;
        let dir_has_version_suffix = explicit_version || id.version() != max_version;
        let dst_name = if dir_has_version_suffix {
            if !explicit_version && disallow_duplicates {
                return Err(format!("found duplicate versions of package `{}` \
                                    at {} and {}, but this was disallowed via \
                                    --disallow-duplicates",
                                   pkg.name(),
                                   id.version(),
                                   max_version).into())
            }
            // Eg vendor/futures-0.1.13
            format!("{}-{}", id.name(), id.version())
        } else {
            // Eg vendor/futures
            id.name().to_string()
        };
        let dst = local_dst.join(&dst_name);
        added_crates.push(dst.clone());
        sources.insert(id.source_id());

        let cksum = dst.join(".cargo-checksum.json");
        if dir_has_version_suffix && cksum.exists() {
            // Always re-copy directory without version suffix in case the version changed
            continue
        }

        config.shell().status("Vendoring",
                              &format!("{} ({}) to {}", id, src.to_string_lossy(), dst.display()))?;

        let _ = fs::remove_dir_all(&dst);
        let mut map = BTreeMap::new();
        try!(cp_r(&src, &dst, &dst, &mut map).chain_err(|| {
            format!("failed to copy over vendored sources for: {}", id)
        }));

        // Finally, emit the metadata about this package
        let json = json!({
            "package": pkg.summary().checksum(),
            "files": map,
        });

        try!(try!(File::create(&cksum)).write_all(json.to_string().as_bytes()));
    }

    if !no_delete {
        for path in existing_crates {
            if !added_crates.contains(&path) {
                try!(fs::remove_dir_all(&path));
            }
        }
    }

    // add our vendored source
    let dir = config.cwd().join(local_dst);
    let mut config = BTreeMap::new();
    config.insert("vendored-sources".to_string(), VendorSource::Directory {
        directory: dir.to_path_buf(),
    });

    // replace original sources with vendor
    for source_id in sources {
        let name = if source_id.is_default_registry() {
            "crates-io".to_string()
        } else {
            source_id.url().to_string()
        };

        let source = if source_id.is_default_registry() {
            VendorSource::Registry {
                registry: None,
                replace_with: "vendored-sources".to_string(),
            }
        } else if source_id.is_git() {
            let mut branch = None;
            let mut tag = None;
            let mut rev = None;
            if let Some(reference) = source_id.git_reference() {
                match *reference {
                    GitReference::Branch(ref b) => branch = Some(b.clone()),
                    GitReference::Tag(ref t) => tag = Some(t.clone()),
                    GitReference::Rev(ref r) => rev = Some(r.clone()),
                }
            }
            VendorSource::Git {
                git: source_id.url().to_string(),
                branch,
                tag,
                rev,
                replace_with: "vendored-sources".to_string(),
            }
        } else {
            panic!()
        };
        config.insert(name, source);
    }

    Ok(VendorConfig { source: config })
}

fn cp_r(src: &Path,
        dst: &Path,
        root: &Path,
        cksums: &mut BTreeMap<String, String>) -> io::Result<()> {
    try!(fs::create_dir(dst));
    for entry in try!(src.read_dir()) {
        let entry = try!(entry);

        match entry.file_name().to_str() {
            // Skip git config files as they're not relevant to builds most of
            // the time and if we respect them (e.g.  in git) then it'll
            // probably mess with the checksums when a vendor dir is checked
            // into someone else's source control
            Some(".gitattributes") |
            Some(".gitignore") |
            Some(".git") => continue,

            // Temporary Cargo files
            Some(".cargo-ok") => continue,

            // Skip patch-style orig/rej files. Published crates on crates.io
            // have `Cargo.toml.orig` which we don't want to use here and
            // otherwise these are rarely used as part of the build process.
            Some(filename) => {
                if filename.ends_with(".orig") || filename.ends_with(".rej") {
                    continue;
                }
            }
            _ => ()
        }

        let src = entry.path();
        let dst = dst.join(entry.file_name());
        if try!(entry.file_type()).is_dir() {
            try!(cp_r(&src, &dst, root, cksums));
        } else {
            try!(fs::copy(&src, &dst));
            let rel = dst.strip_prefix(root).unwrap().to_str().unwrap();
            cksums.insert(rel.replace("\\", "/"), try!(sha256(&dst)));
        }
    }
    Ok(())
}

fn sha256(p: &Path) -> io::Result<String> {
    let mut file = try!(File::open(p));
    let mut sha = Sha256::new();
    let mut buf = [0; 2048];
    loop {
        let n = try!(file.read(&mut buf));
        if n == 0 {
            break
        }
        sha.update(&buf[..n]);
    }
    Ok(hex(&sha.finish()))
}

fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        s.push(hex((byte >> 4) & 0xf));
        s.push(hex((byte >> 0) & 0xf));
    }

    return s;

    fn hex(b: u8) -> char {
        if b < 10 {
            (b'0' + b) as char
        } else {
            (b'a' + b - 10) as char
        }
    }
}
