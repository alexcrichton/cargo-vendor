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

Also note that the output of `cargo vendor` that should be configuration is all
on stdout (as opposed to stderr where other messages go), so you can also do:

```
$ cargo vendor > .cargo/config
```

to vendor and initialize your config in the same step!

### Flag `--no-merge-sources`

If the vendored Cargo project makes use of `[replace]` sections it can happen
that the vendoring operation fails, e.g. with an error like this:

```
found duplicate version of package `libc v0.2.43` vendored from two sources:
...
```

The flag `--no-merge-sources` should be able to solve that. Make sure to grab
the `.cargo/config` file directly from standard output since the config gets more
complicated and unpredictable.

Example:

```
$ cargo vendor --no-merge-sources > .cargo/config
```

### Curl built against the NSS SSL library

If you saw this error message:

```
error: failed to sync
...
Caused by:
  [77] Problem with the SSL CA cert (path? access rights?)
```

That means most likely, that cargo-vendor's curl was linked against
`libcurl-nss.so` due to installed libcurl NSS development files, and that the
required library `libnsspem.so` is missing. See also the curl man page: "If
curl is built against the NSS SSL library, the NSS PEM PKCS#11 module
(libnsspem.so) needs to be available for this option to work properly."

In order to avoid this failure you can either

 * install the missing library (e.g. Debian: `nss-plugin-pem`), or
 * remove the libcurl NSS development files (e.g. Debian: `libcurl4-nss-dev`) and
   rebuild cargo-vendor.

# License

This project is licensed under either of

 * Apache License, Version 2.0, ([LICENSE-APACHE](LICENSE-APACHE) or
   http://www.apache.org/licenses/LICENSE-2.0)
 * MIT license ([LICENSE-MIT](LICENSE-MIT) or
   http://opensource.org/licenses/MIT)

at your option.

### Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in cargo-vendor by you, as defined in the Apache-2.0 license, shall be
dual licensed as above, without any additional terms or conditions.
