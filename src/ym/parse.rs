use core::convert::TryInto;
use core::mem;
use std::io::{self, Read, Seek, SeekFrom};

use log::warn;

use delharc::*;

use super::*;

const YM2_SAMPLES_4BIT: &[u8] = include_bytes!("../../resources/ym2_samples4bit.bin");

pub const YM2_SAMPLE_ENDS: [usize; 40] = [
      631,  1262,  1752,  2242,  2941,  3446,  4173,  4653,
     6761, 10992, 11370, 12897, 13155, 13413, 13864, 15659,
    15930, 16563, 17942, 18089, 18228, 18313, 18463, 18970,
    19200, 19320, 19591, 19884, 20275, 20666, 21057, 21464,
    21871, 22278, 22595, 23002, 23313, 23772, 24101, 24757
];

impl YmSong {
    /// Attempts to parse an YM-file that can be either compressed or uncompressed, from the
    /// given stream source.
    ///
    /// Provide `file_name` which will be used as a fallback song title.
    ///
    /// Returns an instance of `YmSong` on success.
    pub fn parse_any<R, S>(
            mut rd: R,
            file_name: S
        ) -> io::Result<YmSong>
        where R: Read + Seek, S: Into<String>
    {
        let pos = rd.seek(SeekFrom::Current(0))?;
        let mut rd = match LhaDecodeReader::new(rd) {
            Ok(lha) if lha.is_decoder_supported() => {
                return Self::parse_lha_reader(lha)
            }
            Ok(lha) => lha.into_inner(),
            Err(e) => e.into_inner()
        };
        let file_len = rd.seek(SeekFrom::End(0))?;
        rd.seek(SeekFrom::Start(pos))?;
        let mut buf_rd = io::BufReader::new(rd);
        parse_ym(&mut buf_rd, file_len, file_name.into(), None)
    }

    /// Attempts to parse an uncompressed YM-file from the given stream source.
    ///
    /// Provide `file_name` which will be used as a fallback song title.
    ///
    /// Returns an instance of `YmSong` on success.
    pub fn parse_unpacked<R, S>(
            mut rd: R,
            file_name: S
        ) -> io::Result<YmSong>
        where R: Read + Seek, S: Into<String>
    {
        let pos = rd.seek(SeekFrom::Current(0))?;
        let file_len = rd.seek(SeekFrom::End(0))?;
        rd.seek(SeekFrom::Start(pos))?;
        let mut buf_rd = io::BufReader::new(rd);
        parse_ym(&mut buf_rd, file_len, file_name.into(), None)
    }

    /// Attempts to parse a compressed YM-file from the given stream source.
    ///
    /// Returns an instance of `YmSong` on success.
    pub fn parse<R: Read>(rd: R) -> io::Result<YmSong> {
        Self::parse_lha_reader(LhaDecodeReader::new(rd)?)
    }

    fn parse_lha_reader<R: Read>(lha_reader: LhaDecodeReader<R>) -> io::Result<YmSong> {
        // let header = lha_reader.header();
        // println!("{:?} {} {:?} {} {:?}",
        //     header.parse_pathname(),
        //     header.level,
        //     header.compression_method().unwrap(),
        //     header.parse_last_modified(),
        //     header.parse_os_type().unwrap());
        let title = lha_reader.header().parse_pathname().file_name()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_else(|| String::new());
        let created = lha_reader.header().parse_last_modified().to_naive_utc();
        let file_len = lha_reader.len();
        let mut buf_rd = io::BufReader::new(lha_reader);
        parse_ym(&mut buf_rd, file_len, title, created)
    }
}

fn parse_ym(
        rd: &mut dyn io::BufRead,
        file_len: u64,
        title: String,
        created: Option<NaiveDateTime>
    ) -> io::Result<YmSong>
{
    let mut ident = [0u8;4];
    rd.read_exact(&mut ident)?;
    match &ident {
        b"YM2!" => parse_ym2(rd, file_len - mem::size_of_val(&ident) as u64, title, created),
        b"YM3!"|
        b"YM3b" => parse_ym3(YmVersion::Ym3, rd, file_len - mem::size_of_val(&ident) as u64, title, created),
        b"YM4!" => parse_ym4(rd, created),
        b"YM5!" => parse_ym5(YmVersion::Ym5, rd, created),
        b"YM6!" => parse_ym5(YmVersion::Ym6, rd, created),
        _ => {
            Err(io::Error::new(io::ErrorKind::InvalidData, "unrecognized file signature"))
        }
    }
}

fn parse_ym2<R: io::BufRead>(
        rd: R,
        size: u64,
        title: String,
        created: Option<NaiveDateTime>
    ) -> io::Result<YmSong>
{
    parse_ym3(YmVersion::Ym2, rd, size, title, created).map(|mut ym_song| {
        let mut dd_samples = Vec::with_capacity(2 * YM2_SAMPLES_4BIT.len());
        for smp in YM2_SAMPLES_4BIT.iter().copied() {
            dd_samples.push(smp >> 4);
            dd_samples.push(smp & 0x0F);
        }
        ym_song.dd_samples = dd_samples.into_boxed_slice();
        ym_song
    })
}

fn parse_ym3<R: io::BufRead>(
        version: YmVersion,
        mut rd: R,
        size: u64,
        title: String,
        created: Option<NaiveDateTime>
    ) -> io::Result<YmSong>
{
    let includes_loop = match size % 14 {
        0 => false,
        4 => true,
        _ => return Err(io::Error::new(io::ErrorKind::InvalidData, "wrong file size"))
    };
    let nframes = (size / 14).try_into().map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    if nframes == 0 {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "no YM data"))
    }
    let frames = read_interleaved_frames(nframes, 14, &mut rd)?;

    let loop_frame = if includes_loop {
        read_dword(rd)?
    }
    else { 0 };

    Ok(YmSong::new(version, frames, loop_frame, title, created))
}

fn parse_ym4<R: io::BufRead>(mut rd: R, created: Option<NaiveDateTime>) -> io::Result<YmSong> {
    let (nframes, song_attrs, dd_nsamples) = parse_ym4_common(rd.by_ref())?;
    let loop_frame = read_dword(rd.by_ref())?;
    let (dd_samples, dd_samples_ends) = read_digidrum_samples(rd.by_ref(), dd_nsamples, song_attrs)?;
    let (title, author, comments) = read_song_meta(rd.by_ref())?;
    let frames = if song_attrs.is_interleaved() {
        read_interleaved_frames(nframes, 16, rd.by_ref())
    }
    else {
        read_non_interleaved_frames(nframes, 16, rd.by_ref())
    }?;
    read_song_end_tag(rd)?;
    Ok(YmSong::new(YmVersion::Ym4, frames, loop_frame, title, created)
              .with_samples(song_attrs, dd_samples, dd_samples_ends)
              .with_meta(author, comments))
}

fn parse_ym5<R: io::BufRead>(
        version: YmVersion,
        mut rd: R,
        created: Option<NaiveDateTime>
    ) -> io::Result<YmSong>
{
    let (nframes, song_attrs, dd_nsamples) = parse_ym4_common(rd.by_ref())?;
    let chipset_frequency = read_dword(rd.by_ref())?;
    let frame_frequency = read_word(rd.by_ref())?;
    if chipset_frequency == 0 {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "chipset period must not be 0"))
    }
    if frame_frequency == 0 {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "frame period must not be 0"))
    }

    let loop_frame = read_dword(rd.by_ref())?;
    if 0 != read_word(rd.by_ref())? {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "unknown additional header data"))
    }
    let (dd_samples, dd_samples_ends) = read_digidrum_samples(rd.by_ref(), dd_nsamples, song_attrs)?;
    let (title, author, comments) = read_song_meta(rd.by_ref())?;

    let frames = if song_attrs.is_interleaved() {
        read_interleaved_frames(nframes, 16, rd.by_ref())
    }
    else {
        read_non_interleaved_frames(nframes, 16, rd.by_ref())
    }?;

    read_song_end_tag(rd)?;

    Ok(YmSong::new(version, frames, loop_frame, title, created)
              .with_samples(song_attrs, dd_samples, dd_samples_ends)
              .with_meta(author, comments)
              .with_frequency(chipset_frequency, frame_frequency))
}

fn parse_ym4_common<R: Read>(mut rd: R) -> io::Result<(usize, SongAttributes, u16)> {
    let mut leonard = [0u8;8];
    rd.read_exact(&mut leonard)?;
    if &leonard != b"LeOnArD!" {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "unrecognized file verify signature"))
    }
    let nframes = read_dword(rd.by_ref())?
                  .try_into().map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    if nframes == 0 {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "no YM data"))
    }
    let attrs = SongAttributes::from_bits_truncate(read_dword(rd.by_ref())?);
    let dd_nsamples = read_word(rd.by_ref())?;
    if (dd_nsamples as usize) > MAX_DD_SAMPLES {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "too many digi-drum samples"))
    }
    Ok((nframes, attrs, dd_nsamples))
}

fn read_digidrum_samples<R: Read>(
        mut rd: R,
        nsamples: u16,
        song_attrs: SongAttributes,
    ) -> io::Result<(Box<[u8]>, [usize;MAX_DD_SAMPLES as usize])>
{
    assert!((nsamples as usize) <= MAX_DD_SAMPLES);
    let mut sample_data = Vec::new();
    let mut sample_ends = [0usize;MAX_DD_SAMPLES as usize];
    for sep in sample_ends[0..nsamples as usize].iter_mut() {
        let nbytes = read_dword(rd.by_ref())?;
        sample_data.reserve(nbytes as usize);
        if nbytes as usize != rd.by_ref().take(nbytes as u64).read_to_end(&mut sample_data)? {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "file ended prematurely"))
        }
        *sep = sample_data.len();
    }

    if !song_attrs.is_4bit() {
        if song_attrs.is_signed() {
            for t in sample_data.iter_mut() {
                *t = t.wrapping_add(0x80) >> 4;
            }
        }
        else {
            for t in sample_data.iter_mut() {
                *t = *t >> 4;
            }
        }
    }

    Ok((sample_data.into_boxed_slice(), sample_ends))
}

fn read_song_meta<R: io::BufRead>(
        mut rd: R
    ) -> io::Result<(String, String, String)>
{
    let title = read_cstr(rd.by_ref())?;
    let author = read_cstr(rd.by_ref())?;
    let comments = read_cstr(rd.by_ref())?;
    Ok((title, author, comments))
}

fn read_interleaved_frames<R: Read>(nframes: usize, regs: usize, mut rd: R) -> io::Result<Box<[YmFrame]>> {
    let mut frames = vec![YmFrame::default();nframes].into_boxed_slice();
    for r in 0..regs {
        for (fp, res) in frames.iter_mut().zip(rd.by_ref().bytes()) {
            fp.data[r] = res?;
        }
    }
    Ok(frames)
}

fn read_non_interleaved_frames<R: Read>(nframes: usize, regs: usize, mut rd: R) -> io::Result<Box<[YmFrame]>> {
    let mut frames = vec![YmFrame::default();nframes].into_boxed_slice();
    for fp in frames.iter_mut() {
        rd.read_exact(&mut fp.data[0..regs])?;
    }
    Ok(frames)
}

fn read_song_end_tag<R: Read>(mut rd: R) -> io::Result<()> {
    let mut end_mark = [0u8;4];
    match rd.read_exact(&mut end_mark) {
        Ok(..) => {
            if &end_mark != b"End!" {
                warn!("WARNING: invalid End! tag");
                // return Err(io::Error::new(io::ErrorKind::InvalidData, "missing end tag"))
            }
        }
        Err(..) => {
            warn!("WARNING: no End! tag");
        }
    }
    Ok(())
}

fn read_dword<R: Read>(mut rd: R) -> io::Result<u32> {
    let mut dword = [0u8;4];
    rd.read_exact(&mut dword)?;
    Ok(u32::from_be_bytes(dword))
}

fn read_word<R: Read>(mut rd: R) -> io::Result<u16> {
    let mut word = [0u8;2];
    rd.read_exact(&mut word)?;
    Ok(u16::from_be_bytes(word))
}

fn read_cstr<R: io::BufRead>(mut rd: R) -> io::Result<String> {
    let mut vec = Vec::with_capacity(128);
    if 0 == rd.read_until(0, &mut vec)? {
        return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "file ended prematurely"))
    }
    vec.pop();
    match String::from_utf8(vec) {
        Ok(mut s) => {
            s.shrink_to_fit();
            Ok(s)
        },
        Err(e) => {
            Ok(String::from_utf8_lossy(&e.into_bytes()).into_owned())
        }
    }
}
