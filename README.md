YM-file parser
--------------

`Cargo.toml`:

```toml
[dependencies]
ym-file-parser = { git = "https://github.com/royaltm/rust-ym-file-parser" }
```

[Documentation].

The [YM-file format] was designed by [Leonard/OXYGENE] for his AY-emulator [StSound].

YM-files are distributed as compressed [LHA] archives.

This library can help uncompress, parse the YM-files, and produce the AY/YM register changes for the players.

The following YM-file types are supported: `Ym2!`, `Ym3!`, `YM3b`, `Ym4!`, `Ym5!` and `Ym6!`.

The YM music files can be downloaded from [here](https://bulba.untergrund.net/main_e.htm).

[Documentation]: https://royaltm.github.io/rust-ym-file-parser/doc/ym_file_parser/
[YM-file format]: http://leonard.oxg.free.fr/ymformat.html
[Leonard/OXYGENE]: http://leonard.oxg.free.fr
[StSound]: http://leonard.oxg.free.fr/stsound.html
[LHA]: https://en.wikipedia.org/wiki/LHA_(file_format)