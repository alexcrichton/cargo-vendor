#!/bin/sh

set -ex

name="$CRATE_NAME-$TRAVIS_TAG-$TARGET"
mkdir $name
cp target/$TARGET/release/cargo-vendor $name/
cp README.md LICENSE-MIT LICENSE-APACHE $name/
tar czvf $name.tar.gz $name
