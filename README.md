# cargo-fancy

A couple of hours worth of hacking to produce a UI for Cargo that presents a
form of compilation progress as well as what's currently being compiled. Should
give you a good idea about where the compiler is as part of a compilation!

[![asciicast](https://asciinema.org/a/dbid3w8jqdpc7sauqdzkn784t.png)](https://asciinema.org/a/dbid3w8jqdpc7sauqdzkn784t)

## Installation

```
cargo install cargo-fancy
```

## Usage

Just insert `fancy` into any command you want fancified!

```
$ cargo fancy build
$ cargo fancy test
$ cargo fancy build --release -j3 --manifest-path foo/Cargo.toml
```

## Caveats

* Does not work on Windows
* Requires running against the nightly compiler
* Relies on an unstable an unreliable method of timing the compiler, namely `-Z
  time-passes`

