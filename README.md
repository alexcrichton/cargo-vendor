# cargo-vendor

This is a proof-of-concept [Cargo](http://doc.crates.io) subcommand which is
used to vendor all [crates.io](https://crates.io) dependencies into a local
directory.

Leveraging `file://` URLs, this subcommand will construct a custom registry
index which only contains the necessary packages, cache all necessary crate
files, and then place everything in a structure that Cargo expects.

## Installation

Currently this can be installed with:

```
$ git clone https://github.com/alexcrichton/cargo-vendor
$ cd cargo-vendor
$ cargo build --release
```

Then move the binary `target/release/cargo-vendor` into your `PATH` or add
`target/release` to your `PATH`.

## Example Usage

First, `cd` into a Cargo project's root directory, then run the following
command:

```
$ cargo vendor
Create a `.cargo/config` with this entry to use the vendor cache:

    [registry]
    index = "file:///home/foo/code/bar/vendor/index"

$ cargo build
    Updating registry `file:///home/foo/code/bar/vendor/index`
 Downloading rustc-serialize v0.3.16 (registry file:///home/foo/code/bar/vendor/index)
   Compiling rustc-serialize v0.3.16 (registry file:///home/foo/code/bar/vendor/index)
   Compiling ...
```

This will populate the `vendor` directory (generating an error it if it already
exists) with the index for the "custom registry" as well as a copy of all the
crates needed. The configuration printed can be placed in any `.cargo/config`
to point your project at that index, and then all future builds will use that
index.

## How it Works

This uses the same mechanisms that Cargo actually uses to test its support for
registry-based sources. Cargo primarily knows where to download crates from
through a git repository called the *index*. The [main crates.io index][index]
contains all the necessary information to download crates and learn about their
dependencies.

[index]: https://github.com/rust-lang/crates.io-index

All this information isn't needed for just one crate build though! This crate
will take your crate's `Cargo.lock` and generate an index with the bare minimum
of information needed to build your crate. This custom index is stored in
`vendor/index` if you'd like to poke around it.

You'll note a `config.json` file at the root of the index itself, and the
notable part of this is the `dl` key which indicates where crates are downloaded
from. This notably also uses the `file://` protocol to specify the root location
where crates are "downloaded" from. This is currently `vendor/download`, and
that location is populated with the downloaded `.crate` file for all of your
dependencies.

After assembling these two locations, you can tell cargo to use a non-default
index via the `registry.index` key in `.cargo/config`. With all that set up
Cargo will talk to your local index and "download" files from it, never touching
the network!

## Drawbacks

* Currently the `Cargo.lock` will change depending on whether the index is
  crates.io or the local vendor'd copy. This is not always desired and may
  require the crates.io `Cargo.lock` to be stored next to the vendor'd
  `Cargo.lock`.

* Updating dependencies currently requires going back to the crates.io
  `Cargo.lock` and then re-vendoring all dependencies.

* The vendor directory may not be suitable for checking into a VCS as it
  contains a git repo itself and also contains tarballs, not checked out
  contents.

# License

`cargo-vendor` is primarily distributed under the terms of both the MIT license
and the Apache License (Version 2.0), with portions covered by various BSD-like
licenses.

See LICENSE-APACHE, and LICENSE-MIT for details.
