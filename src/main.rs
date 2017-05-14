extern crate cargo;
extern crate env_logger;
extern crate rustc_serialize;

use std::cmp;
use std::collections::{BTreeMap, HashMap, BTreeSet};
use std::env;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::Path;

use rustc_serialize::hex::ToHex;
use rustc_serialize::json::{self, ToJson};

use cargo::core::{SourceId, Dependency, Workspace};
use cargo::CliResult;
use cargo::util::{human, ChainError, ToUrl, Config, CargoResult};
use cargo::util::Sha256;

#[derive(RustcDecodable)]
struct Options {
    arg_path: Option<String>,
    flag_sync: Vec<String>,
    flag_host: Option<String>,
    flag_verbose: u32,
    flag_quiet: Option<bool>,
    flag_explicit_version: Option<bool>,
    flag_color: Option<String>,
}

fn main() {
    env_logger::init().unwrap();
    let config = Config::default().unwrap();
    let args = env::args().collect::<Vec<_>>();
    let result = cargo::call_main_without_stdin(real_main, &config, r#"
Vendor all dependencies for a project locally

Usage:
    cargo vendor [options] [<path>]

Options:
    -h, --help               Print this message
    -s, --sync LOCK ...      Sync the registry with LOCK
    --host HOST              Registry index to sync with
    -v, --verbose ...        Use verbose output
    -q, --quiet              No output printed to stdout
    -x, --explicit-version   Always include version in subdir name
    --color WHEN             Coloring: auto, always, never

This cargo subcommand will vendor all crates.io dependencies for a project into
the specified directory at `<path>`. The `cargo vendor` command requires that
a `Cargo.lock` already exists and it will ensure that after the command
completes the vendor directory specified by `<path>` will contain all sources
necessary to build the project from crates.io.

The `cargo vendor` command will also print out the configuration necessary
to use the vendored sources, which when needed is then encoded into
`.cargo/config`.
"#, &args, false);

    if let Err(e) = result {
        cargo::exit_with_error(e, &mut *config.shell());
    }
}

fn real_main(options: Options, config: &Config) -> CliResult {
    try!(config.configure(options.flag_verbose,
                          options.flag_quiet,
                          &options.flag_color,
                          /* frozen = */ false,
                          /* locked = */ false));

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

    let mut lockfiles = options.flag_sync;
    if lockfiles.len() == 0 {
        try!(fs::metadata("Cargo.lock").chain_error(|| {
            human("could not find `Cargo.lock`, must be run in a directory \
                   with Cargo.lock or use the `--sync` option")
        }));
        lockfiles.push("Cargo.lock".into());
    }

    let explicit = options.flag_explicit_version.unwrap_or(false);
    try!(sync(&lockfiles, &path, &id, config, explicit).chain_error(|| {
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

fn sync(lockfiles: &[String],
        local_dst: &Path,
        registry_id: &SourceId,
        config: &Config,
        explicit_version: bool) -> CargoResult<()> {
    let mut ids = BTreeSet::new();
    let mut registry = registry_id.load(config);

    for lockfile in lockfiles {
        let lockfile = Path::new(lockfile);
        let manifest = lockfile.parent().unwrap().join("Cargo.toml");
        let manifest = env::current_dir().unwrap().join(&manifest);
        let ws = try!(Workspace::new(&manifest, config));
        let resolve = try!(cargo::ops::load_pkg_lockfile(&ws).chain_error(|| {
            human("failed to load pkg lockfile")
        }));
        let resolve = try!(resolve.chain_error(|| {
            human(format!("lock file `{}` does not exist", lockfile.display()))
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
        let mut json = BTreeMap::new();
        let crate_file = format!("{}-{}.crate", id.name(), id.version());
        let crate_file = cache.join(&crate_file).into_path_unlocked();
        json.insert("package", try!(sha256(&crate_file)).to_json());
        json.insert("files", map.to_json());
        let json = json::encode(&json).unwrap();

        try!(try!(File::create(&cksum)).write_all(json.as_bytes()));
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

        // Skip .gitattributes as they're not relevant to builds most of the
        // time and if we respect them (e.g. in git) then it'll probably mess
        // with the checksums.
        if entry.file_name().to_str() == Some(".gitattributes") {
            continue
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
