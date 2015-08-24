extern crate cargo;
extern crate rustc_serialize;
extern crate url;
extern crate git2;

use std::fs;

use cargo::core::Source;
use cargo::core::registry::PackageRegistry;
use cargo::ops;
use cargo::sources::PathSource;
use cargo::{Config, CliResult};
use cargo::util::{important_paths, human, ChainError};

#[derive(RustcDecodable)]
struct Options {
    flag_verbose: bool,
    flag_quiet: bool,
}

mod registry;

fn main() {
    cargo::execute_main_without_stdin(real_main, false, r#"
Vendor all dependencies for a project locally

Usage:
    cargo vendor [options]

Options:
    -h, --help               Print this message
    -v, --verbose            Use verbose output
    -q, --quiet              No output printed to stdout
    --color WHEN             Coloring: auto, always, never
"#)
}

fn real_main(options: Options, config: &Config) -> CliResult<Option<()>> {
    try!(config.shell().set_verbosity(options.flag_verbose, options.flag_quiet));

    // Load the root package
    let root = try!(important_paths::find_root_manifest_for_cwd(None));
    let mut source = try!(PathSource::for_path(root.parent().unwrap(), config));
    try!(source.update());
    let package = try!(source.root_package());

    // Resolve all dependencies (generating or using Cargo.lock if necessary)
    let mut registry = PackageRegistry::new(config);
    try!(registry.add_sources(&[package.package_id().source_id().clone()]));
    let resolve = try!(ops::resolve_pkg(&mut registry, &package));

    // And vendor everything!
    let package_ids = resolve.iter().filter(|s| s.source_id().is_registry())
                             .map(|x| x.clone())
                             .collect::<Vec<_>>();
    let packages = try!(registry.get(&package_ids));
    let vendor_dir = config.cwd().join("vendor");
    try!(fs::create_dir(&vendor_dir).chain_error(|| {
        human("failed to create a vendor directory")
    }));
    try!(registry::vendor(config, &packages, &vendor_dir));

    Ok(None)
}
