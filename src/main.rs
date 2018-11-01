extern crate cargo;
extern crate env_logger;
#[macro_use]
extern crate serde_derive;
#[macro_use]
extern crate serde_json;
extern crate toml;
#[macro_use]
extern crate failure;
extern crate docopt;

use std::collections::{BTreeMap, HashMap, BTreeSet};
use std::collections::hash_map::DefaultHasher;
use std::fs::{self, File};
use std::hash::Hasher;
use std::io::{self, Read, Write};
use std::path::{PathBuf, Path};

use cargo::core::{Workspace, GitReference, SourceId, enable_nightly_features};
use cargo::CliResult;
use cargo::util::{Config, CargoResult, CargoResultExt};
use cargo::util::Sha256;
use docopt::Docopt;

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
    flag_relative_path: bool,
    flag_only_git_deps: bool,
    flag_no_merge_sources: bool,
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

const SOURCES_FILE_NAME: &str = ".sources";

fn main() {
    env_logger::init();

    // We're doing the vendoring operation outselves, so we don't actually want
    // to respect any of the `source` configuration in Cargo itself. That's
    // intended for other consumers of Cargo, but we want to go straight to the
    // source, e.g. crates.io, to fetch crates.
    let mut config = {
        let config_orig = Config::default().unwrap();
        let mut values = config_orig.values().unwrap().clone();
        values.remove("source");
        let config = Config::default().unwrap();
        config.set_values(values).unwrap();
        config
    };

    let usage = r#"
Vendor all dependencies for a project locally

Usage:
    cargo vendor [options] [<path>]

Options:
    -h, --help               Print this message
    -V, --version            Print version information
    -s, --sync TOML ...      Sync one or more `Cargo.toml` or `Cargo.lock`
    -v, --verbose ...        Use verbose output
    -q, --quiet              No output printed to stdout
    -x, --explicit-version   Always include version in subdir name
    --disallow-duplicates    Disallow two versions of one crate
    --no-delete              Don't delete older crates in the vendor directory
    --only-git-deps          Only vendor git dependencies, not crates.io dependencies
    --frozen                 Require Cargo.lock and cache are up to date
    --locked                 Require Cargo.lock is up to date
    --color WHEN             Coloring: auto, always, never
    --relative-path          Use relative vendor path for .cargo/config
    --no-merge-sources       Keep sources separate

This cargo subcommand will vendor all crates.io dependencies for a project into
the specified directory at `<path>`. The `cargo vendor` command is intended to
be run in the same directory as `Cargo.toml`, but the manifest can also be
specified via one or more instances of the `--sync` flag. After this command
completes the vendor directory specified by `<path>` will contain all sources
from crates.io necessary to build the manifests specified.

The `cargo vendor` command will also print out the configuration necessary
to use the vendored sources, which when needed is then encoded into
`.cargo/config`.
"#;

    let options = Docopt::new(usage)
        .and_then(|d| d.deserialize())
        .unwrap_or_else(|e| e.exit());
    let result = real_main(options, &mut config);
    if let Err(e) = result {
        cargo::exit_with_error(e, &mut *config.shell());
    }
}

fn real_main(options: Options, config: &mut Config) -> CliResult {
    if options.flag_version {
        println!("{} {}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"));
        return Ok(());
    }

    // We're not too interested in gating users based on nightly features or
    // not, so just assume they're all enabled in the version of Cargo we're
    // using.
    enable_nightly_features();

    config.configure(options.flag_verbose,
                     options.flag_quiet,
                     &options.flag_color,
                     options.flag_frozen,
                     options.flag_locked,
                     &None, // target_dir,
                     &[])?;

    let default = "vendor".to_string();
    let path = Path::new(options.arg_path.as_ref().unwrap_or(&default));

    let sources_file = path.join(SOURCES_FILE_NAME);
    let is_multi_sources = sources_file.exists();
    if is_multi_sources && !options.flag_no_merge_sources
        || !is_multi_sources && options.flag_no_merge_sources {
            fs::remove_dir_all(path).ok();
        }

    fs::create_dir_all(&path).chain_err(|| {
        format!("failed to create: `{}`", path.display())
    }).map_err(|e| cargo::CargoError::from(e))?;

    if !is_multi_sources && options.flag_no_merge_sources {
        let mut file = File::create(sources_file)
            .map_err(|e| cargo::CargoError::from(e))?;
        file.write_all(json!([]).to_string().as_bytes())
            .map_err(|e| cargo::CargoError::from(e))?;
    }

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

    let vendor_config = sync(
        &workspaces,
        &path,
        config,
        options.flag_explicit_version.unwrap_or(false),
        options.flag_no_delete.unwrap_or(false),
        options.flag_disallow_duplicates,
        options.flag_relative_path,
        options.flag_only_git_deps,
        !options.flag_no_merge_sources,
    ).chain_err(|| {
        format!("failed to sync")
    }).map_err(|e| cargo::CargoError::from(e))?;

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
        disallow_duplicates: bool,
        use_relative_path: bool,
        only_git_deps: bool,
        merge_sources: bool) -> CargoResult<VendorConfig> {
    let canonical_local_dst = local_dst.canonicalize().unwrap_or(local_dst.to_path_buf());
    let mut ids = BTreeMap::new();
    let mut added_crates = Vec::new();

    // First up attempt to work around rust-lang/cargo#5956. Apparently build
    // artifacts sprout up in Cargo's global cache for whatever reason, although
    // it's unsure what tool is causing these issues at this time. For now we
    // apply a heavy-hammer approach which is to delete Cargo's unpacked version
    // of each crate to start off with. After we do this we'll re-resolve and
    // redownload again, which should trigger Cargo to re-extract all the
    // crates.
    //
    // Note that errors are largely ignored here as this is a best-effort
    // attempt. If anything fails here we basically just move on to the next
    // crate to work with.
    for ws in workspaces {
        let (packages, resolve) = cargo::ops::resolve_ws(&ws).chain_err(|| {
            "failed to load pkg lockfile"
        })?;

        for pkg in resolve.iter() {
            // Don't delete actual source code!
            if pkg.source_id().is_path() {
                continue
            }
            if pkg.source_id().is_git() {
                continue;
            }
            if let Ok(pkg) = packages.get(pkg) {
                drop(fs::remove_dir_all(pkg.manifest_path().parent().unwrap()));
            }
        }
    }

    for ws in workspaces {
        let (packages, resolve) = cargo::ops::resolve_ws(&ws).chain_err(|| {
            "failed to load pkg lockfile"
        })?;

        for pkg in resolve.iter() {
            if pkg.source_id().is_path() {
                let path = pkg.source_id().url().to_file_path().expect("path");
                let canonical_path = path.canonicalize().unwrap_or(path.to_path_buf());
                if canonical_path.starts_with(canonical_local_dst.as_path()) {
                    added_crates.push(canonical_path);
                }
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

        match map.get(&id.version()) {
            Some(prev) if merge_sources =>
                bail!("found duplicate version of package `{} v{}` \
                       vendored from two sources:\n\
                       \n\
                       \tsource 1: {}\n\
                       \tsource 2: {}",
                      id.name(),
                      id.version(),
                      prev,
                      id.source_id()),
            _ => {},
        }
        map.insert(id.version(), id.source_id());
    }

    let source_paths = if merge_sources {
        let mut set = BTreeSet::new();
        set.insert(canonical_local_dst.clone());
        set
    } else {
        let sources_file = canonical_local_dst.join(SOURCES_FILE_NAME);
        let file = File::open(&sources_file)?;
        serde_json::from_reader::<_,BTreeSet<PathBuf>>(file)?
            .into_iter()
            .map(|p| canonical_local_dst.join(p))
            .collect()
    };

    let existing_crates: Vec<PathBuf> = source_paths
        .iter()
        .flat_map(|path| path
                  .read_dir()
                  .map(|iter| iter
                       .filter_map(|e| e.ok())
                       .filter(|e| e.path().join("Cargo.toml").exists())
                       .map(|e| e.path())
                       .collect::<Vec<_>>())
                  .unwrap_or(Vec::new()))
        .collect();

    let mut sources = BTreeSet::new();
    for (id, pkg) in ids.iter() {
        // Next up, copy it to the vendor directory
        let src = pkg.manifest_path().parent().expect("manifest_path should point to a file");
        let max_version = *versions[&id.name()].iter().rev().next().unwrap().0;
        let dir_has_version_suffix = explicit_version || id.version() != max_version;
        let dst_name = if dir_has_version_suffix {
            if !explicit_version && disallow_duplicates {
                bail!("found duplicate versions of package `{}` \
                        at {} and {}, but this was disallowed via \
                        --disallow-duplicates",
                       pkg.name(),
                       id.version(),
                       max_version)
            }
            // Eg vendor/futures-0.1.13
            format!("{}-{}", id.name(), id.version())
        } else {
            // Eg vendor/futures
            id.name().to_string()
        };


        if !id.source_id().is_git() && only_git_deps {
            // Skip out if we only want to process git dependencies
            continue;
        }

        let source_dir = if merge_sources {
            canonical_local_dst.clone()
        } else {
            canonical_local_dst.join(source_id_to_dir_name(id.source_id()))
        };
        if sources.insert(id.source_id()) && !merge_sources {
            fs::create_dir_all(&source_dir).chain_err(|| {
                format!("failed to create: `{}`", source_dir.display())
            }).map_err(|e| cargo::CargoError::from(e))?;
        }
        let dst = source_dir.join(&dst_name);
        added_crates.push(dst.clone());

        let cksum = dst.join(".cargo-checksum.json");
        if dir_has_version_suffix && cksum.exists() {
            // Always re-copy directory without version suffix in case the version changed
            continue
        }

        config.shell().status("Vendoring",
                              &format!("{} ({}) to {}", id, src.to_string_lossy(), dst.display()))?;

        let _ = fs::remove_dir_all(&dst);
        let pathsource = cargo::sources::path::PathSource::new(&src, id.source_id(), config);
        let paths = pathsource.list_files(&pkg)?;
        let mut map = BTreeMap::new();
        cp_sources(&src, &paths, &dst, &mut map).chain_err(|| {
            format!("failed to copy over vendored sources for: {}", id)
        })?;

        // Finally, emit the metadata about this package
        let json = json!({
            "package": pkg.summary().checksum(),
            "files": map,
        });

        File::create(&cksum)?.write_all(json.to_string().as_bytes())?;
    }

    if !no_delete {
        for path in existing_crates {
            if !added_crates.contains(&path) {
                fs::remove_dir_all(&path)?;
            }
        }
    }

    if !merge_sources {
        let sources_file = canonical_local_dst.join(SOURCES_FILE_NAME);
        let file = File::open(&sources_file)?;
        let mut new_sources: BTreeSet<String> = sources
            .iter()
            .map(|src_id| source_id_to_dir_name(src_id))
            .collect();
        let old_sources: BTreeSet<String> = serde_json::from_reader::<_,BTreeSet<String>>(file)?
            .difference(&new_sources)
            .map(|e| e.clone())
            .collect();
        for dir_name in old_sources {
            let path = canonical_local_dst.join(dir_name.clone());
            if path.is_dir() {
                if path.read_dir()?.next().is_none() {
                    fs::remove_dir(path)?;
                } else {
                    new_sources.insert(dir_name.clone());
                }
            }
        }
        let file = File::create(sources_file)?;
        serde_json::to_writer(file, &new_sources)?;
    }

    // add our vendored source
    let dir = if use_relative_path {
        local_dst.to_path_buf()
    } else {
        config.cwd().join(local_dst)
    };
    let mut config = BTreeMap::new();

    let merged_source_name = "vendored-sources";
    if merge_sources {
        config.insert(merged_source_name.to_string(), VendorSource::Directory {
            directory: dir.clone(),
        });
    }

    // replace original sources with vendor
    for source_id in sources {
        let name = if source_id.is_default_registry() {
            "crates-io".to_string()
        } else {
            source_id.url().to_string()
        };

        let replace_name = if !merge_sources {
            format!("vendor+{}", name)
        } else {
            merged_source_name.to_string()
        };

        if !merge_sources {
            let src_id_string = source_id_to_dir_name(source_id);
            let src_dir = dir.join(src_id_string.clone());
            config.insert(replace_name.clone(), VendorSource::Directory {
                directory: src_dir,
            });
        }

        let source = if source_id.is_default_registry() {
            VendorSource::Registry {
                registry: None,
                replace_with: replace_name,
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
                replace_with: replace_name,
            }
        } else {
            panic!()
        };
        config.insert(name, source);
    }

    Ok(VendorConfig { source: config })
}

fn cp_sources(src: &Path,
              paths: &Vec<PathBuf>,
              dst: &Path,
              cksums: &mut BTreeMap<String, String>) -> CargoResult<()> {
    for p in paths {
        let relative = p.strip_prefix(&src).unwrap();

        match relative.to_str() {
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
        };

        // Join pathname components individually to make sure that the joined
        // path uses the correct directory separators everywhere, since
        // `relative` may use Unix-style and `dst` may require Windows-style
        // backslashes.
        let dst = relative.iter().fold(dst.to_owned(), |acc, component| {
            acc.join(&component)
        });

        fs::create_dir_all(dst.parent().unwrap())?;

        fs::copy(&p, &dst).chain_err(|| {
            format!("failed to copy `{}` to `{}`", p.display(), dst.display())
        })?;
        cksums.insert(relative.to_str().unwrap().replace("\\", "/"), sha256(&dst)?);
    }
    Ok(())
}

fn source_id_to_dir_name(src_id: &SourceId) -> String {
    let src_type = if src_id.is_registry() {
        "registry"
    } else if src_id.is_git() {
        "git"
    } else {
        panic!()
    };
    let mut hasher = DefaultHasher::new();
    src_id.stable_hash(Path::new(""), &mut hasher);
    let src_hash = hasher.finish();
    let mut bytes = [0; 8];
    for i in 0..7 {
        bytes[i] = (src_hash >> i * 8) as u8
    }
    format!("{}-{}", src_type, hex(&bytes))
}

fn sha256(p: &Path) -> io::Result<String> {
    let mut file = File::open(p)?;
    let mut sha = Sha256::new();
    let mut buf = [0; 2048];
    loop {
        let n = file.read(&mut buf)?;
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
