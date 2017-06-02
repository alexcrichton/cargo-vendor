extern crate cargo;
extern crate env_logger;
extern crate rustc_serialize;
#[macro_use]
extern crate serde_json;
extern crate toml;
#[macro_use]
extern crate maplit;

use std::cmp;
use std::collections::{BTreeMap, HashMap, HashSet, BTreeSet};
use std::env;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::Path;

use rustc_serialize::hex::ToHex;

use cargo::core::{SourceId, Workspace, Package, GitReference};
use cargo::CliResult;
use cargo::util::{human, ChainError, Config, CargoResult};
use cargo::util::Sha256;

#[derive(RustcDecodable)]
struct Options {
    arg_path: Option<String>,
    flag_no_delete: Option<bool>,
    flag_version: bool,
    flag_sync: Option<Vec<String>>,
    flag_host: Option<String>,
    flag_verbose: u32,
    flag_quiet: Option<bool>,
    flag_explicit_version: Option<bool>,
    flag_color: Option<String>,
    flag_frozen: bool,
    flag_locked: bool,
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
    --host HOST              Registry index to sync with
    -v, --verbose ...        Use verbose output
    -q, --quiet              No output printed to stdout
    -x, --explicit-version   Always include version in subdir name
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

    let cargo_config = try!(sync(&workspaces,
              &path,
              config,
              options.flag_explicit_version.unwrap_or(false),
              options.flag_no_delete.unwrap_or(false)).chain_err(|| {
        "failed to sync"
    }));

    if !options.flag_quiet.unwrap_or(false) {
        println!("To use vendored sources, add this to your .cargo/config for this project:

{}", indent_string(4, &cargo_config));
    }

    Ok(())
}

fn indent_string(amount: usize, data: &str) -> String {
    data.lines().map(|l| " ".repeat(amount) + l + "\n").collect()
}

fn sync(workspaces: &[Workspace],
        local_dst: &Path,
        config: &Config,
        explicit_version: bool,
        no_delete: bool) -> CargoResult<String> {
    let skip = workspaces.iter().flat_map(Workspace::members).map(Package::package_id).collect::<HashSet<_>>();

    let mut ids = BTreeMap::new();
    for ws in workspaces {
        let (packages, resolve) = try!(cargo::ops::resolve_ws(&ws).chain_error(|| {
            human("failed to load pkg lockfile")
        }));
        for id in resolve.iter() {
            if skip.contains(id) { continue }

            let pkg = packages.get(id).chain_error(|| {
                human(format!("failed to fetch package"))
            })?;

            ids.insert(id.clone(), pkg.clone());
        }
    }

    let mut max = HashMap::new();
    let mut sources = BTreeSet::new();
    for id in ids.keys() {
        let max = max.entry(id.name()).or_insert(id.version());
        *max = cmp::max(id.version(), *max);

        sources.insert(id.source_id());
    }

    let existing_crates = local_dst.read_dir().map(|iter| {
        iter.filter_map(|e| e.ok())
            .filter(|e| e.path().join("Cargo.toml").exists())
            .map(|e| e.path())
            .collect::<Vec<_>>()
    }).unwrap_or(Vec::new());

    let mut added_crates = Vec::new();
    for (id, pkg) in ids.iter() {
        // copy it to the vendor directory
        let src = pkg.manifest_path().parent().expect("manifest_path should point to a file");
        let dir_has_version_suffix = explicit_version || id.version() != max[id.name()];
        let dst_name = if dir_has_version_suffix {
            // Eg vendor/futures-0.1.13
            format!("{}-{}", id.name(), id.version())
        } else {
            // Eg vendor/futures
            id.name().to_string()
        };
        let dst = local_dst.join(&dst_name);
        added_crates.push(dst.clone());

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

    // build the .cargo/config for using the vendored sources
    let mut sources_config = BTreeMap::new();

    // add our vendored source
    let dir = config.cwd().join(local_dst).to_str().expect("vendor path must be utf8").to_string();
    sources_config.insert("vendored-sources".to_string(),  toml::Value::Table(btreemap! {
        "directory".to_string() => toml::Value::String(dir),
    }));

    // replace original sources with vendor
    for source_id in sources.into_iter() {
        let base_name = source_name(source_id);
        let mut name = base_name.clone();

        // append a number if required to make key unique
        let mut suffix = 0;
        while sources_config.contains_key(&name) {
            name = format!("{}-{}", base_name, suffix);
            suffix += 1;
        }

        let kind = if source_id.is_registry() {
            "registry"
        } else if source_id.is_git() {
            "git"
        } else if source_id.is_path() {
            "directory"
        } else {
            panic!("unhandled source kind: {}", source_id);
        };

        let mut source_config = btreemap! {
            kind.to_string() => toml::Value::String(source_id.url().to_string()),
            "replace-with".to_string() => toml::Value::String("vendored-sources".to_string()),
        };

        if let Some(reference) = source_id.git_reference() {
            let (key, value) = match *reference {
                GitReference::Branch(ref branch) => ("branch", branch),
                GitReference::Tag(ref tag) => ("tag", tag),
                GitReference::Rev(ref rev) => ("rev", rev),
            };
            source_config.insert(key.to_string(), toml::Value::String(value.to_string()));
        }

        sources_config.insert(name, toml::Value::Table(source_config));
    }

    Ok(toml::to_string(&toml::Value::Table(btreemap! {
        "source".to_string() => toml::Value::Table(sources_config)
    })).unwrap())
}

fn source_name(id: &SourceId) -> String {
    let ident = id.url().path_segments().and_then(|iter| {
        iter.rev().skip_while(|s| s.is_empty()).next()
    }).unwrap_or("_empty");

    format!("{}-{}", ident, cargo::util::short_hash(id.url()))
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
            Some(".gitattributes") => continue,
            Some(".gitignore") => continue,
            Some(".git") => continue,
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
    Ok(sha.finish().to_hex())
}
