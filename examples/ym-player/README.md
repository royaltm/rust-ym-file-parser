YM player
=========

An example YM song player, implemented using [cpal] and [AY-3-891x] emulator in addition to the [ym-file-parser] library.

Run with:

```
cargo run -r -p ym-player
```

```
cargo run -r -p ym-player -- "ghp/ymfiles/Best Part of The Creation.ym" -c ABC -v 75 -d
```

```
cargo run -r -p ym-player -- -h

An example YM chiptune format player using an AY-3-891x emulator.

Usage: ym-player.exe [OPTIONS] [YM_FILE]

Arguments:
  [YM_FILE]  A file path to an YM song

Options:
  -v, --volume <VOLUME>      Mixer volume: 0 - 100 [default: 50]
  -m, --mono <MONO>          Monophonic channel mixer amplitude: 0.0 - 1.0 [default: 0.8]
  -r, --repeat <REPEAT>      Play counter, 0 = play forever [default: 0]
  -c, --channels <CHANNELS>  YM channels map: Left Center Right [default: "ACB"]
  -d, --debug...             Log verbosity level
  -h, --help                 Print help
  -V, --version              Print version
```

[cpal]: https://crates.io/crates/cpal
[AY-3-891x]: https://docs.rs/spectrusty-peripherals/latest/spectrusty_peripherals/ay/index.html
[ym-file-parser]: https://royaltm.github.io/rust-ym-file-parser/
