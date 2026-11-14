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

UnRAR's CRC32 code is based on Intel's Slicing-by-8 code, under this BSD license (from `vendor/unrar/acknow.txt`):

```
Copyright (c) 2004-2006 Intel Corporation.
All Rights Reserved

Redistribution and use in source and binary forms, with or without
modification, are permitted provided that the following conditions
are met:

Redistributions of source code must retain the above copyright notice,
this list of conditions and the following disclaimer.

Redistributions in binary form must reproduce the above copyright
notice, this list of conditions and the following disclaimer
in the documentation and/or other materials provided with
the distribution.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS
"AS IS" AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT
LIMITED TO, THE IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS
FOR A PARTICULAR PURPOSE ARE DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT
HOLDER OR CONTRIBUTORS BE LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL,
SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT
LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES; LOSS OF USE,
DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND
ON ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY,
OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT
OF THE USE OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF
SUCH DAMAGE.
```

## Rust crates

Every crate compiled into `rexplorer.exe` (x64 and arm64). Each one offers the MIT license, and rexplorer uses it
under MIT; the MIT text is below. CI fails when this table and `Cargo.lock` disagree (the `notices` job in
`.github/workflows/ci.yml`).

| Crate | Version | License | Copyright |
|---|---|---|---|
| `aho-corasick` | 1.1.5 | Unlicense OR MIT | Copyright (c) 2015 Andrew Gallant |
| `bitflags` | 2.13.2 | MIT OR Apache-2.0 | Copyright (c) 2014 The Rust Project Developers |
| `libc` | 0.2.190 | MIT OR Apache-2.0 | Copyright (c) The Rust Project Developers |
| `memchr` | 2.8.3 | Unlicense OR MIT | Copyright (c) 2015 Andrew Gallant |
| `regex` | 1.13.1 | MIT OR Apache-2.0 | Copyright (c) 2014 The Rust Project Developers |
| `regex-automata` | 0.4.18 | MIT OR Apache-2.0 | Copyright (c) 2014 The Rust Project Developers |
| `regex-syntax` | 0.8.11 | MIT OR Apache-2.0 | Copyright (c) 2014 The Rust Project Developers |
| `unrar-ng` | 0.7.7, modified: built from `vendor/unrar-ng` with one fix (`VENDORED.md` there) | MIT OR Apache-2.0 | The unrar.rs authors (Danyel Bayraktar, vjoki, ttys3) |
| `unrar-ng-sys` | 0.7.7 | MIT OR Apache-2.0 (bindings; libunrar as above) | The unrar.rs authors (Danyel Bayraktar, ttys3) |
| `widestring` | 1.2.1 | MIT OR Apache-2.0 | The widestring-rs authors |
| `winapi` | 0.3.9 | MIT OR Apache-2.0 | Copyright (c) 2015-2018 The winapi-rs Developers |
| `windows` | 0.62.2 | MIT OR Apache-2.0 | Copyright (c) Microsoft Corporation |
| `windows-collections` | 0.3.2 | MIT OR Apache-2.0 | Copyright (c) Microsoft Corporation |
| `windows-core` | 0.62.2 | MIT OR Apache-2.0 | Copyright (c) Microsoft Corporation |
| `windows-future` | 0.3.2 | MIT OR Apache-2.0 | Copyright (c) Microsoft Corporation |
| `windows-link` | 0.2.1 | MIT OR Apache-2.0 | Copyright (c) Microsoft Corporation |
| `windows-numerics` | 0.3.1 | MIT OR Apache-2.0 | Copyright (c) Microsoft Corporation |
| `windows-result` | 0.4.1 | MIT OR Apache-2.0 | Copyright (c) Microsoft Corporation |
| `windows-strings` | 0.5.1 | MIT OR Apache-2.0 | Copyright (c) Microsoft Corporation |
| `windows-threading` | 0.2.1 | MIT OR Apache-2.0 | Copyright (c) Microsoft Corporation |

Used only while building, not part of the exe: `cc`, `find-msvc-tools`, `shlex` (compile libunrar), and the
macro crates `windows-implement`, `windows-interface`, `proc-macro2`, `quote`, `syn`, `unicode-ident`. All are
MIT OR Apache-2.0 (`unicode-ident` adds Unicode-3.0).

To refresh the table after a dependency change, list the crates with:

```
cargo tree --locked -e normal,no-proc-macro --prefix none --target x86_64-pc-windows-msvc --target aarch64-pc-windows-msvc
```

### MIT license

Applies to every crate above, with the copyright line from its row:

```
Permission is hereby granted, free of charge, to any person obtaining a copy of this software and associated
documentation files (the "Software"), to deal in the Software without restriction, including without limitation
the rights to use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies of the Software, and to
permit persons to whom the Software is furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all copies or substantial portions of
the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO
THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT,
TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

## Assets

- App icon (`crates/app/app.ico`, the only image embedded as a resource): rexplorer's own artwork, drawn by
  `scripts/make-icon.py`, under rexplorer's MIT license.
- The English and Spanish text (`crates/core/src/lang/*.ini`) is rexplorer's own.
- No fonts or other images are bundled. Fonts, file icons and thumbnails come from Windows at run time.
