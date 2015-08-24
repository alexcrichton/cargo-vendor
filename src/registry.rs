use std::collections::HashMap;
use std::fs::{self, File};
use std::io::prelude::*;
use std::path::Path;

use cargo::Config;
use cargo::core::Package;
use cargo::core::dependency::Kind;
use cargo::util::{human, hex, CargoResult, ChainError};
use git2::{self, Repository};
use rustc_serialize::json;
use url::Url;

#[derive(RustcEncodable)]
struct RegistryPackage {
    name: String,
    vers: String,
    deps: Vec<RegistryDependency>,
    features: HashMap<String, Vec<String>>,
    cksum: String,
    yanked: Option<bool>,
}

#[derive(RustcEncodable)]
struct RegistryDependency {
    name: String,
    req: String,
    features: Vec<String>,
    optional: bool,
    default_features: bool,
    target: Option<String>,
    kind: String,
}

pub fn vendor(config: &Config,
              packages: &[Package],
              into: &Path) -> CargoResult<()> {
    let index = into.join("index");
    let download = into.join("cache");
    try!(fs::create_dir(&download));
    let index_url = try!(Url::from_file_path(&index).map_err(|()| {
        human(format!("failed to convert {:?} to a URL", index))
    }));
    let dl_url = try!(Url::from_file_path(&download).map_err(|()| {
        human(format!("failed to convert {:?} to a URL", download))
    }));
    let repo = try!(Repository::init(&index));
    try!(File::create(&index.join("config.json")).and_then(|mut f| {
        f.write_all(format!(r#"{{"dl":"{}","api":""}}"#, dl_url).as_bytes())
    }));

    for package in packages {
        try!(vendor_package(config, package, &index, &download).chain_error(|| {
            human(format!("failed to vendor `{}`", package.package_id()))
        }));
    }

    try!(commit_index(&repo).chain_error(|| {
        human("failed to commit the index")
    }));

    println!("Create a `.cargo/config` with this entry to use the vendor cache:

    [registry]
    index = \"{}\"

", index_url);
    Ok(())
}

fn vendor_package(config: &Config,
                  package: &Package,
                  index: &Path,
                  download: &Path) -> CargoResult<()> {
    let package_id = package.package_id();
    let source_id = package_id.source_id();

    // Copy the crate file into place
    let crate_file = config.registry_cache_path().join({
        let hash = hex::short_hash(source_id);
        let ident = source_id.url().host().unwrap().to_string();
        format!("{}-{}", ident, hash)
    }).join({
        format!("{}-{}.crate", package_id.name(), package_id.version())
    });
    let dst = download.join(package_id.name())
                      .join(package_id.version().to_string())
                      .join(crate_file.file_name().unwrap());
    try!(fs::create_dir_all(dst.parent().unwrap()));
    try!(fs::copy(&crate_file, &dst).chain_error(|| {
        human(format!("cached crate file `{}` doesn't exist for `{}`",
                      crate_file.display(), package_id))
    }));

    // Create an entry in the index for this package
    let package = RegistryPackage {
        name: package_id.name().to_string(),
        vers: package_id.version().to_string(),
        features: package.summary().features().clone(),
        yanked: Some(false),
        cksum: String::new(),
        deps: package.dependencies().iter().map(|d| {
            RegistryDependency {
                name: d.name().to_string(),
                req: d.version_req().to_string(),
                features: d.features().to_vec(),
                optional: d.is_optional(),
                default_features: d.uses_default_features(),
                target: d.only_for_platform().map(|t| t.to_string()),
                kind: match d.kind() {
                    Kind::Normal => "normal".to_string(),
                    Kind::Build => "build".to_string(),
                    Kind::Development => "dev".to_string(),
                },
            }
        }).collect(),
    };
    let json = json::encode(&package).unwrap();
    let dst = match package_id.name().len() {
        1 => index.join("1").join(package_id.name()),
        2 => index.join("2").join(package_id.name()),
        3 => index.join("3").join(&package_id.name()[..1])
                            .join(package_id.name()),
        _ => index.join(&package_id.name()[..2])
                  .join(&package_id.name()[2..4])
                  .join(package_id.name()),
    };
    try!(fs::create_dir_all(dst.parent().unwrap()));
    try!(File::create(&dst).and_then(|mut f| {
        f.write_all(json.as_bytes())
    }));
    Ok(())
}

fn commit_index(repo: &Repository) -> CargoResult<()> {
    let mut index = try!(repo.index());
    try!(index.add_all(&["*"], git2::ADD_DEFAULT, None));
    try!(index.write());
    let tree_id = try!(index.write_tree());
    let tree = try!(repo.find_tree(tree_id));
    let sig = try!(repo.signature());
    try!(repo.commit(Some("HEAD"), &sig, &sig, "Initial commit", &tree, &[]));
    Ok(())
}
