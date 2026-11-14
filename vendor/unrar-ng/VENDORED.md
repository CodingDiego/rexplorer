# unrar-ng 0.7.7, vendored with one fix

Source: the `unrar-ng` 0.7.7 package from crates.io (https://github.com/ttys3/unrar.rs, MIT OR Apache-2.0; both
license texts are beside this file). `src/` is the published `src/` unchanged except for the fix below, and
`README.md` is its readme (the library includes it as its documentation); the examples and the changelog are not
copied. `unrar-ng-sys` (the bundled C++ library) still comes from crates.io.

The published package's sha256 is `9ff7a01700c732826b5a3414f911cb72015642ebdf16566f2e88b36d5dc49aec` (the checksum
`Cargo.lock` held for it), built from upstream commit `0971923085c66fa663b1081dd31c5e1150be12cb`.

## The fix

`src/open_archive.rs`, `Internal::callback`, the `UCM_CHANGEVOLUMEW` arm.

The published code copies 2048 wide characters from the pointer libunrar passes for the next volume's name
(`WideCString::from_ptr_truncate(p1, 2048)`). The bundled libunrar passes `std::wstring::data()` there
(`vendor/unrar/volume.cpp`, `DllVolNotify`), a buffer only as long as the name. The copy reads up to 4 KB past it,
and when that reaches an unmapped page the process dies with an access violation inside `memcpy`. It happens on
every volume change of a multi-volume archive, listing included, and crashes only now and then.

Seen in rexplorer's tests (2026-11-13): `archive::tests::multi_volume_middle_or_missing_volume_fails_clearly`,
stack `archive::list` > `RARProcessFileW` > `MergeArchive` > `DllVolNotify` > `Internal::callback` >
`U16CString::from_ptr_truncate`, in about one of ten runs of the whole core test binary.

The fix reads up to the string's terminating NUL (`from_ptr_str`), which `std::wstring::data()` always has.

## When to drop this folder

When a release of `unrar-ng` no longer reads a fixed length there: delete `vendor/unrar-ng` and the
`[patch.crates-io]` entry in the workspace `Cargo.toml`, and run `cargo update -p unrar-ng`.
