extern crate cargo;
extern crate rustc_serialize;

use std::cmp;
use std::collections::{BTreeMap, HashMap};
use std::env;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::Path;

use rustc_serialize::hex::ToHex;
use rustc_serialize::json::{self, ToJson};

use cargo::core::{SourceId, Dependency, Package};
use cargo::CliResult;
use cargo::util::{human, ChainError, ToUrl, Config, CargoResult};
use cargo::util::Sha256;

#[derive(RustcDecodable)]
struct Options {
    arg_path: Option<String>,
    flag_sync: Option<String>,
    flag_host: Option<String>,
    flag_verbose: u32,
    flag_quiet: Option<bool>,
    flag_color: Option<String>,
}

fn main() {
    cargo::execute_main_without_stdin(real_main, false, r#"
Vendor all dependencies for a project locally

Usage:
    cargo vendor [options] [<path>]

Options:
    -h, --help               Print this message
    -s, --sync LOCK          Sync the registry with LOCK
    --host HOST              Registry index to sync with
    -v, --verbose            Use verbose output
    -q, --quiet              No output printed to stdout
    --color WHEN             Coloring: auto, always, never
"#)
}

fn real_main(options: Options, config: &Config) -> CliResult<Option<()>> {
    try!(config.configure_shell(options.flag_verbose,
                                options.flag_quiet,
                                &options.flag_color));

    let default = "vendor".to_string();
    let path = Path::new(options.arg_path.as_ref().unwrap_or(&default));

    try!(fs::create_dir_all(&path).chain_error(|| {
        human(format!("failed to create: `{}`", path.display()))
    }));
    let id = try!(options.flag_host.map(|s| {
        s.to_url().map(|url| SourceId::for_registry(&url)).map_err(human)
    }).unwrap_or_else(|| {
        SourceId::for_central(config)
    }));

    let lockfile = match options.flag_sync {
        Some(ref file) => file,
        None => {
            try!(fs::metadata("Cargo.lock").chain_error(|| {
                human("could not find `Cargo.lock`, must be run in a directory \
                       with Cargo.lock or use the `--sync` option")
            }));
            "Cargo.lock"
        }
    };

    try!(sync(Path::new(lockfile), &path, &id, config).chain_error(|| {
        human("failed to sync")
    }));

    println!("add this to your .cargo/config for this project:

    [source.crates-io]
    registry = '{}'
    replace-with = 'vendored-sources'

    [source.vendored-sources]
    directory = '{}'

", id.url(), config.cwd().join(path).display());

    Ok(None)
}

fn sync(lockfile: &Path,
        local_dst: &Path,
        registry_id: &SourceId,
        config: &Config) -> CargoResult<()> {
    let mut registry = registry_id.load(config);
    let manifest = lockfile.parent().unwrap().join("Cargo.toml");
    let manifest = env::current_dir().unwrap().join(&manifest);
    let pkg = try!(Package::for_path(&manifest, config).chain_error(|| {
        human("failed to load package")
    }));
    let resolve = try!(cargo::ops::load_pkg_lockfile(&pkg, config).chain_error(|| {
        human("failed to load pkg lockfile")
    }));
    let resolve = try!(resolve.chain_error(|| {
        human(format!("lock file `{}` does not exist", lockfile.display()))
    }));

    let hash = cargo::util::hex::short_hash(registry_id);
    let ident = registry_id.url().host().unwrap().to_string();
    let part = format!("{}-{}", ident, hash);

    let src = config.registry_source_path().join(&part);
    let cache = config.registry_cache_path().join(&part);

    let ids = resolve.iter()
                     .filter(|id| id.source_id() == registry_id)
                     .cloned()
                     .collect::<Vec<_>>();
    let mut max = HashMap::new();
    for id in ids.iter() {
        let max = max.entry(id.name()).or_insert(id.version());
        *max = cmp::max(id.version(), *max)
    }

    for id in ids.iter() {
        // First up, download the package
        let vers = format!("={}", id.version());
        let dep = try!(Dependency::parse(id.name(), Some(&vers[..]),
                                         id.source_id()));
        let vec = try!(registry.query(&dep));
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
        let dst_name = if id.version() == max[id.name()] {
            id.name().to_string()
        } else {
            format!("{}-{}", id.name(), id.version())
        };
        let dst = local_dst.join(&dst_name);
        let cksum = dst.join(".cargo-checksum.json");
        if cksum.exists() {
            continue
        }
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
        let src = entry.path();
        let dst = dst.join(entry.file_name());
        if try!(entry.file_type()).is_dir() {
            try!(cp_r(&src, &dst, root, cksums));
        } else {
            try!(fs::copy(&src, &dst));
            let rel = dst.strip_prefix(root).unwrap().to_str().unwrap();
            cksums.insert(rel.to_string(), try!(sha256(&dst)));
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
