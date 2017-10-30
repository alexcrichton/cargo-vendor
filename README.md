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
$ cargo install --git https://github.com/alexcrichton/cargo-vendor
```

You can also install [precompiled
binaries](https://github.com/alexcrichton/cargo-vendor/releases) that are
assembled on the CI for this crate.

## Example Usage

Simply run `cargo vendor` inside of any Cargo project:

```
$ cargo vendor
add this to your .cargo/config for this project:

    [source.crates-io]
    replace-with = 'vendored-sources'

    [source.vendored-sources]
    directory = '/home/alex/code/cargo-vendor/vendor'
```

This will populate the `vendor` directory which contains the source of all
crates.io dependencies. When configured, Cargo will then use this directory
instead of looking at crates.io.

# License

This project is licensed under either of

 * Apache License, Version 2.0, ([LICENSE-APACHE](LICENSE-APACHE) or
   http://www.apache.org/licenses/LICENSE-2.0)
 * MIT license ([LICENSE-MIT](LICENSE-MIT) or
   http://opensource.org/licenses/MIT)

at your option.

### Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in Serde by you, as defined in the Apache-2.0 license, shall be
dual licensed as above, without any additional terms or conditions.
