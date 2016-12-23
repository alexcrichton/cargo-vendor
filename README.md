# cargo-vendor

[![Build Status](https://travis-ci.org/alexcrichton/cargo-vendor.svg?branch=master)](https://travis-ci.org/alexcrichton/cargo-vendor)
[![Build status](https://ci.appveyor.com/api/projects/status/0sqqqnkfgw4o3cvs?svg=true)](https://ci.appveyor.com/project/alexcrichton/cargo-vendor)

This is a [Cargo](http://doc.crates.io) subcommand which
vendors all [crates.io](https://crates.io) dependencies into a local directory
using Cargo's support for [source
replacement](http://doc.crates.io/source-replacement.html).

## Installation

Currently this can be installed with:

```
$ cargo install cargo-vendor
```

## Example Usage

Simply run `cargo vendor` inside of any Cargo project:

```
$ cargo vendor
add this to your .cargo/config for this project:

    [source.crates-io]
    registry = 'https://github.com/rust-lang/crates.io-index'
    replace-with = 'vendored-sources'

    [source.vendored-sources]
    directory = '/home/alex/code/cargo-vendor/vendor'
```

This will populate the `vendor` directory which contains the source of all
crates.io dependencies. When configured, Cargo will then use this directory
instead of looking at crates.io.

# License

`cargo-vendor` is primarily distributed under the terms of both the MIT license
and the Apache License (Version 2.0), with portions covered by various BSD-like
licenses.

See LICENSE-APACHE, and LICENSE-MIT for details.
