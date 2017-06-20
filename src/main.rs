extern crate cargo;
extern crate env_logger;
extern crate rustc_serialize;
#[macro_use]
extern crate serde_json;

use std::cmp;
use std::collections::{BTreeMap, HashMap, BTreeSet};
use std::env;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::Path;

use rustc_serialize::hex::ToHex;

use cargo::core::{SourceId, Dependency, Workspace};
use cargo::CliResult;
use cargo::util::{human, ChainError, ToUrl, Config, CargoResult};
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

    try!(fs::create_dir_all(&path).chain_error(|| {
        human(format!("failed to create: `{}`", path.display()))
    }));
    let id = try!(options.flag_host.map(|s| {
        s.to_url().map(|url| SourceId::for_registry(&url)).map_err(human)
    }).unwrap_or_else(|| {
        SourceId::crates_io(config)
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

    try!(sync(&workspaces,
              &path,
              &id,
              config,
              options.flag_explicit_version.unwrap_or(false),
              options.flag_no_delete.unwrap_or(false)).chain_error(|| {
        human("failed to sync")
    }));

    if !options.flag_quiet.unwrap_or(false) {
        println!("To use vendored sources, add this to your .cargo/config for this project:

    [source.crates-io]
    registry = '{}'
    replace-with = 'vendored-sources'

    [source.vendored-sources]
    directory = '{}'

", id.url(), config.cwd().join(path).display());
    }

    Ok(())
}

fn sync(workspaces: &[Workspace],
        local_dst: &Path,
        registry_id: &SourceId,
        config: &Config,
        explicit_version: bool,
        no_delete: bool) -> CargoResult<()> {
    let mut ids = BTreeSet::new();
    let mut registry = registry_id.load(config);

    for ws in workspaces {
        let (_, resolve) = try!(cargo::ops::resolve_ws(&ws).chain_error(|| {
            human("failed to load pkg lockfile")
        }));
        ids.extend(resolve.iter()
                     .filter(|id| id.source_id() == registry_id)
                     .cloned());
    }

    let hash = cargo::util::hex::short_hash(registry_id);
    let ident = registry_id.url().host().unwrap().to_string();
    let part = format!("{}-{}", ident, hash);

    let src = config.registry_source_path().join(&part);
    let cache = config.registry_cache_path().join(&part);

    let mut max = HashMap::new();
    for id in ids.iter() {
        let max = max.entry(id.name()).or_insert(id.version());
        *max = cmp::max(id.version(), *max)
    }

    let existing_crates = local_dst.read_dir().map(|iter| {
        iter.filter_map(|e| e.ok())
            .filter(|e| e.path().join("Cargo.toml").exists())
            .map(|e| e.path())
            .collect::<Vec<_>>()
    }).unwrap_or(Vec::new());

    let mut added_crates = Vec::new();
    for id in ids.iter() {
        // First up, download the package
        let vers = format!("={}", id.version());
        let dep = try!(Dependency::parse_no_deprecated(id.name(),
                                                       Some(&vers[..]),
                                                       id.source_id()));
        let mut vec = try!(registry.query(&dep));

        // Some versions have "build metadata" which is ignored by semver when
        // matching. That means that `vec` being returned may have more than one
        // element, so we filter out all non-equivalent versions with different
        // build metadata than the one we're looking for.
        //
        // Note that we also don't compare semver versions directly as the
        // default equality ignores build metadata.
        if vec.len() > 1 {
            vec.retain(|version| {
                version.package_id().version().to_string() == id.version().to_string()
            });
        }
        if vec.len() == 0 {
            return Err(human(format!("could not find package: {}", id)))
        }
        if vec.len() > 1 {
            return Err(human(format!("found too many packages: {}", id)))
        }
        try!(registry.download(id).chain_error(|| {
            human(format!("failed to download package from registry"))
        }));

        // Next up, copy it to the vendor directory
        let name = format!("{}-{}", id.name(), id.version());
        let src = src.join(&name).into_path_unlocked();
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
                              &format!("{} to {}", id, dst.display()))?;

        let _ = fs::remove_dir_all(&dst);
        let mut map = BTreeMap::new();
        try!(cp_r(&src, &dst, &dst, &mut map).chain_error(|| {
            human(format!("failed to copy over vendored sources for: {}", id))
        }));

        // Finally, emit the metadata about this package
        let crate_file = format!("{}-{}.crate", id.name(), id.version());
        let crate_file = cache.join(&crate_file).into_path_unlocked();

        let json = json!({
            "package": try!(sha256(&crate_file)),
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

    Ok(())
}

fn cp_r(src: &Path,
        dst: &Path,
        root: &Path,
        cksums: &mut BTreeMap<String, String>) -> io::Result<()> {
    try!(fs::create_dir(dst));
    for entry in try!(src.read_dir()) {
        let entry = try!(entry);

        // Skip git config files and SCM backup/reject files as they're not
        // relevant to builds most of the time and if we respect them (e.g. in
        // git) then it'll probably mess with the checksums.
        match entry.file_name().to_str() {
            Some(".gitattribute") => continue,
            Some(".gitignore") => continue,
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
    Ok(sha.finish().to_hex())
}
