# Third-party notices

rexplorer's own code is under the MIT license (see `LICENSE`). The release binary also contains the
following third-party code.

## UnRAR (libunrar)

RAR support links the UnRAR source library by Alexander Roshal, bundled unmodified by the
[`unrar-ng-sys`](https://crates.io/crates/unrar-ng-sys) crate (libunrar 7.21). Its license is not MIT. The
required paragraph, verbatim:

> UnRAR source code may be used in any software to handle RAR archives without limitations free of charge, but
> cannot be used to develop RAR (WinRAR) compatible archiver and to re-create RAR compression algorithm, which is
> proprietary. Distribution of modified UnRAR source code in separate form or as a part of other software is
> permitted, provided that full text of this paragraph, starting from "UnRAR source code" words, is included in
> license, or in documentation if license is not available, and in source code comments of resulting package.

The full UnRAR license ships with the crate source (`vendor/unrar/license.txt`). All copyrights to RAR and UnRAR
belong to Alexander Roshal. rexplorer only lists and extracts RAR archives; it never creates them.

## Rust crates

| Crate | License |
|---|---|
| `windows`, `windows-core`, `windows-collections` (Microsoft) | MIT OR Apache-2.0 |
| `unrar-ng`, `unrar-ng-sys` | MIT OR Apache-2.0 (bindings; libunrar as above) |
| `cc` (build only) | MIT OR Apache-2.0 |

Every other crate in `Cargo.lock` is MIT or Apache-2.0 licensed (`aho-corasick` and `memchr` also offer Unlicense;
`unicode-ident` adds Unicode-3.0 for its data tables). `cargo metadata` lists the full set.
