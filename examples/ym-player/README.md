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
cargo run -r -p ym-player -- --help

An example YM chiptune format player using an AY-3-891x emulator.

Usage: ym-player [OPTIONS] [YM_FILE]

Arguments:
  [YM_FILE]
          A file path to an YM song

Options:
  -v, --volume <VOLUME>
          Audio mixer volume: 0 - 100

          [default: 50]

  -r, --repeat <REPEAT>
          Play counter, 0 to play forever

          [default: 0]

  -c, --channels <CHANNELS>
          YM channels map: Left Center Right

          [default: ACB]

  -m, --mode <MODE>
          Channel mode: s|m|0.s|N.

          "s" - stereo mode with a center channel mixed with an amplitude of 0.8

          "m" - monophonic mode, played on all audio channels

          "0.s" - stereo mode, center channel amplitude: 0.s

          "N" - multi-channel mode, redirect center channel to Nth (3+) audio channel

          [default: 0.8]

  -f, --fuse
          Switch to alternative YM amplitude levels (measured vs specs)

  -d, --debug...
          Log verbosity level.

          -d for INFO, -dd for DEBUG, -ddd for TRACE

  -h, --help
          Print help (see a summary with '-h')

  -V, --version
          Print version
```

[cpal]: https://crates.io/crates/cpal
[AY-3-891x]: https://docs.rs/spectrusty-peripherals/latest/spectrusty_peripherals/ay/index.html
[ym-file-parser]: https://royaltm.github.io/rust-ym-file-parser/
