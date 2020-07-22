use core::time::Duration;
use core::num::NonZeroU32;
use core::fmt;
use core::ops::Range;
use chrono::NaiveDateTime;

pub mod flags;
pub mod effects;
mod parse;
mod player;

use flags::*;
use effects::*;

pub const MAX_DD_SAMPLES: usize = 32;

pub const MFP_TIMER_FREQUENCY: u32 = 2_457_600;
const DEFAULT_CHIPSET_FREQUENCY: u32 = 2_000_000;
const DEFAULT_FRAME_FREQUENCY: u16 = 50;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum YmVersion {
    Ym2,
    Ym3,
    Ym4,
    Ym5,
    Ym6,
}

impl fmt::Display for YmVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            YmVersion::Ym2 => "Ym2!",
            YmVersion::Ym3 => "Ym3!",
            YmVersion::Ym4 => "Ym4!",
            YmVersion::Ym5 => "Ym5!",
            YmVersion::Ym6 => "Ym6!",
        }.fmt(f)
    }
}

/// The **YM** music file.
///
/// The YM-file consist of [YmFrame]s that represent the state of the AY/YM chipset registers and
/// contain additional information about special effects.
///
/// Depending on the [YmSong::version] special effects are being encoded differently.
#[derive(Debug, Clone)]
pub struct YmSong {
    /// YM-file version.
    pub version: YmVersion,
    /// The last modification timestamp of the YM-file from the LHA envelope.
    pub created: Option<NaiveDateTime>,
    /// The song attributes.
    pub song_attrs: SongAttributes,
    /// The song title or a file name.
    pub title: String,
    /// The song author.
    pub author: String,
    /// The comment.
    pub comments: String,
    /// The number of cycles per second of the AY/YM chipset clock.
    pub chipset_frequency: u32,
    /// The number of frames played each second.
    pub frame_frequency: u16,
    /// The loop frame index.
    pub loop_frame: u32,
    /// The AY/YM state frames.
    pub frames: Box<[YmFrame]>,
    /// `DIGI-DRUM` samples.
    pub dd_samples: Box<[u8]>,
    /// `DIGI-DRUM` sample end indexes in [YmSong::dd_samples].
    pub dd_samples_ends: [usize;MAX_DD_SAMPLES],
        cursor: usize,
        voice_effects: [(SidVoice, SinusSid, DigiDrum); 3],
        buzzer: SyncBuzzer,
}

/// This type represent the state of the AY/YM chipset registers and contain additional information
/// about special effects.
///
/// ```text
/// X - AY/YM register data.
/// S - Controls special effects.
/// P - Frequency pre-divisor.
/// F - Frequency divisor.
/// - - Unused.
/// ----------------------------------------------------------
///      b7 b6 b5 b4 b3 b2 b1 b0  Register description
///  0:  X  X  X  X  X  X  X  X   Fine period voice A
///  1:  S  S  S  S  X  X  X  X   Coarse period voice A
///  2:  X  X  X  X  X  X  X  X   Fine period voice B
///  3:  S  S  S  S  X  X  X  X   Coarse period voice B
///  4:  X  X  X  X  X  X  X  X   Fine period voice C
///  5:  -  -  -  -  X  X  X  X   Coarse period voice C
///  6:  P  P  P  X  X  X  X  X   Noise period
///  7:  X  X  X  X  X  X  X  X   Mixer control
///  8:  P  P  P  X  X  X  X  X   Volume voice A
///  9:  -  -  -  X  X  X  X  X   Volume voice B
/// 10:  -  -  -  X  X  X  X  X   Volume voice C
/// 11:  X  X  X  X  X  X  X  X   Envelope fine period
/// 12:  X  X  X  X  X  X  X  X   Envelope coarse period
/// 13:  x  x  x  x  X  X  X  X   Envelope shape
/// ----------------------------------------------------------
/// virtual registers to store extra data for special effects:
/// ----------------------------------------------------------
/// 14:  F  F  F  F  F  F  F  F   Frequency divisor for S in 1
/// 15:  F  F  F  F  F  F  F  F   Frequency divisor for S in 3
/// ```
///
/// The AY/YM `Envelope shape` register is modified only if the value of the 13 frame
/// register is not equal to `0xff`.
///
/// # Special effects
///
/// The frequency of a special effect is encoded as `(2457600 / P) / F`.
///
/// The divisor `F` is an unsigned 8-bit integer.
///
/// The pre-divisor `P` is encoded as:
/// 
/// |PPP|  pre-divisor value|
/// |-----------------------|
/// |000|         Timer off |
/// |001|                 4 |
/// |010|                10 |
/// |011|                16 |
/// |100|                50 |
/// |101|                64 |
/// |110|               100 |
/// |111|               200 |
/// 
/// * The pre-divisor `P` in register 6 matches effect controlled by register 1.
/// * The divisor `F` in register 14 matches effect controlled by register 1.
/// * The pre-divisor `P` in register 8 matches effect controlled by register 3.
/// * The divisor `F` in register 15 matches effect controlled by register 3.
///
/// If an effect is active, the additional data resides in `X` bits in the `Volume` register of
/// the relevant voice:
///
/// * For the [`SID voice`][SidVoice] and [`Sinus SID`][SinusSid] effects the 4 lowest `X` bits
///   determine the effect's volume.
/// * For the [`Sync Buzzer`][SyncBuzzer] the 4 lowest `X` bits determine the effect's `Envelope shape`.
/// * For the [`DIGI-DRUM`][DigiDrum] effect the 5 `X` bits determine the played sample number.
/// * The `DIGI-DRUM` sample plays until its end or if its overridden by another effect.
/// * All other effects are active only for the duration of a single frame.
/// * When the `DIGI-DRUM` is active the volume register from the frame for the relevant voice is being
///   ignored and the relevant voice mixer tone and noise bits are forced to be set.
///
/// The control bits of special effects are interpreted differently depending on the YM-file verion.
///
/// ## YM6!
///
/// The `S` bits in registers 1 and 3 controls any two of the selectable effects:
/// ```text
/// b7 b6 b5 b4 
/// -  -  0  0  effect disabled
/// -  -  0  1  effect active on voice A
/// -  -  1  0  effect active on voice B
/// -  -  1  1  effect active on voice C
/// 0  0  -  -  select SID voice effect
/// 0  1  -  -  select DIGI-DRUM effect
/// 1  0  -  -  select Sinus SID effect
/// 1  1  -  -  select Sync Buzzer effect
/// ```
///
/// ## YM4!/YM5!
///
/// The `S` bits in register 1 controls the `SID voice` effect.
/// The `S` bits in register 3 controls the `DIGI-DRUM` effect.
/// ```text
/// b7 b6 b5 b4 
/// -  -  0  0  effect disabled
/// -  -  0  1  effect active on voice A
/// -  -  1  0  effect active on voice B
/// -  -  1  1  effect active on voice C
/// -  0  -  -  SID voice timer continues, ignored for DIGI-DRUM
/// -  1  -  -  SID voice timer restarts, ignored for DIGI-DRUM
///```
///
/// ## YM3!
///
/// There are no special effects in this version.
///
/// ## YM2!
///
/// Only the `DIGI-DRUM` effect is recognized in this format. It is being played on voice C, and
/// uses one of the 40 predefined samples.
///
/// * The effect starts when the highest bit (7) of the `Volume voice C` register (10) is 1.
/// * The sample number is taken from the lowest 7 bits of the `Volume voice C` register (10).
/// * The effect frequency is calculated by `(2457600 / 4) / X`, where `X` is the unsigned 8-bit
///   value stored in the register 12 of the frame.
/// * The value of AY/YM chipset registers 11, 12 and 13 is only written if the value of the
///   frame register 13 is not equal to `0xFF`.
/// * The register 12 of the AY/YM chipset is always being set to `0` in this format.
/// * The register 13 of the AY/YM chipset is always being set to `0x10` in this format.
#[derive(Default, Debug, Clone, Copy)]
pub struct YmFrame {
    /// Frame data.
    pub data: [u8;16]
}

impl YmSong {
    /// Creates a new instance of `YmSong` from the given `frames` and other meta data.
    pub fn new(
            version: YmVersion,
            frames: Box<[YmFrame]>,
            loop_frame: u32,
            title: String,
            created: Option<NaiveDateTime>
        ) -> YmSong
    {
        YmSong {
            version,
            created,
            song_attrs: SongAttributes::default(),
            title,
            author: String::new(),
            comments: String::new(),
            chipset_frequency: DEFAULT_CHIPSET_FREQUENCY,
            frame_frequency: DEFAULT_FRAME_FREQUENCY,
            loop_frame,
            frames,
            dd_samples: Box::new([]),
            dd_samples_ends: [0usize;MAX_DD_SAMPLES],
            cursor: 0,
            voice_effects: Default::default(),
            buzzer: Default::default()
        }
    }

    /// Returns `YmSong` with the `author` and `comments` set from the given arguments.
    pub fn with_meta(mut self, author: String, comments: String) -> YmSong {
        self.author = author;
        self.comments = comments;
        self
    }

    /// Returns `YmSong` with the `song_attrs`, `dd_samples` and `dd_samples_ends` set from the given arguments.
    pub fn with_samples(
            mut self,
            song_attrs: SongAttributes,
            dd_samples: Box<[u8]>,
            dd_samples_ends: [usize;MAX_DD_SAMPLES]
        ) -> YmSong
     {
        self.song_attrs = song_attrs;
        self.dd_samples = dd_samples;
        self.dd_samples_ends = dd_samples_ends;
        self
    }

    /// Returns `YmSong` with the `chipset_frequency` and `frame_frequency` set from the given arguments.
    pub fn with_frequency(mut self, chipset_frequency: u32, frame_frequency: u16) -> YmSong {
        self.chipset_frequency = chipset_frequency;
        self.frame_frequency = frame_frequency;
        self
    }

    /// Returns the song duration.
    pub fn song_duration(&self) -> Duration {
        let seconds = self.frames.len() as f64 / self.frame_frequency as f64;
        Duration::from_secs_f64(seconds)
    }

    /// Returns the AY/YM chipset clock frequency.
    #[inline]
    pub fn clock_frequency(&self) -> f32 {
        self.chipset_frequency as f32
    }

    /// Returns the number of AY/YM chipset clock cycles of a single music frame.
    pub fn frame_cycles(&self) -> f32 {
        self.clock_frequency() / self.frame_frequency as f32
    }

    /// Calculates the timer interval in clock cycles, from the given `divisor`.
    pub fn timer_interval(&self, divisor: NonZeroU32) -> f32 {
        let divisor = divisor.get() as f32;
        self.clock_frequency() as f32 * divisor / MFP_TIMER_FREQUENCY as f32
    }

    /// Returns the indicated sample data range in the [YmSong::dd_samples] for the given `sample`.
    ///
    /// # Panics
    /// Panics if `sample` value is not below [MAX_DD_SAMPLES].
    pub fn sample_data_range(&self, sample: usize) -> Range<usize> {
        let end = self.dd_samples_ends[sample];
        let start = match sample {
            0 => 0,
            index => self.dd_samples_ends[index - 1]
        };
        start..end
    }
}

impl YmFrame {
    /// Returns special effect control flags from the register 1.
    pub fn fx0(&self) -> FxCtrlFlags {
        FxCtrlFlags::from_bits_truncate(self.data[1])
    }

    /// Returns special effect control flags from the register 3.
    pub fn fx1(&self) -> FxCtrlFlags {
        FxCtrlFlags::from_bits_truncate(self.data[3])
    }

    /// Returns the value of the volume register for the indicated `chan`.
    ///
    /// The 2 lowest `chan` bits indicates the voice channel:
    /// ```text
    ///  b1 b0 voice channel
    ///  0  0  A
    ///  0  1  B
    ///  1  0  C
    ///  1  1  invalid
    /// ```
    pub fn vol(&self, chan: u8) -> u8 {
        let chan = chan & 3;
        debug_assert_ne!(chan, 3);
        self.data[(VOL_A_REG + chan) as usize] & 0x1f
    }

    /// Calculates the timer divsor for the special effect `fx0`.
    pub fn timer_divisor0(&self) -> Option<NonZeroU32> {
        calculate_timer_divisor(self.data[6], self.data[14])
    }

    /// Calculates the timer divsor for the special effect `fx1`.
    pub fn timer_divisor1(&self) -> Option<NonZeroU32> {
        calculate_timer_divisor(self.data[8], self.data[15])
    }
}

fn calculate_timer_divisor(prediv3: u8, div8: u8) -> Option<NonZeroU32> {
    let prediv = match prediv3 & 0b11100000 {
        0b00000000 => 0,
        0b00100000 => 4,
        0b01000000 => 10,
        0b01100000 => 16,
        0b10000000 => 50,
        0b10100000 => 64,
        0b11000000 => 100,
        0b11100000 => 200,
        _ => unreachable!()
    };
    NonZeroU32::new(prediv * div8 as u32)
}

