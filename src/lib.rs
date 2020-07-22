//! YM-file parser and player helper.
//!
//! This [format] was designed by [Leonard/OXYGENE] for his AY-emulator [StSound].
//!
//! YM-files are distributed as compressed [LHA] archives.
//!
//! This library can help uncompress, parse the YM-files, and produce the AY/YM register changes for the players.
//!
//! [format]: http://leonard.oxg.free.fr/ymformat.html
//! [Leonard/OXYGENE]: http://leonard.oxg.free.fr
//! [StSound]: http://leonard.oxg.free.fr/stsound.html
//! [LHA]: https://en.wikipedia.org/wiki/LHA_(file_format)
use std::{io, fs, path::Path};

mod ym;

pub use ym::*;

/// Attempts to parse an YM-file that can be either compressed or uncompressed, from the
/// given file `path`.
///
/// Returns an instance of `YmSong` on success.
pub fn parse_file<P: AsRef<Path>>(path: P) -> io::Result<YmSong> {
    let file = fs::File::open(path.as_ref())?;
    let file_name = path.as_ref().file_name()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_else(|| String::new());
    YmSong::parse_any(file, file_name)
}
