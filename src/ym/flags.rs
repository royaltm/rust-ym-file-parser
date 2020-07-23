//! `YmSong` related flags.
use bitflags::bitflags;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum FxChannel {
    Idle   = 0,
    RunOnA = 1,
    RunOnB = 2,
    RunOnC = 3,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum FxType {
    SidVoice = 0,
    DigiDrum = 1,
    SinusSid = 2,
    SyncBuzz = 3,
}

bitflags! {
    #[derive(Default)]
    pub struct SongAttributes: u32 {
        const INTERLEAVED     = 0x0000_0001;
        const DIGIDRUM_SIGNED = 0x0000_0002;
        const DIGIDRUM_4BIT   = 0x0000_0004;
    }
}

bitflags! {
    #[derive(Default)]
    pub struct FxCtrlFlags: u8 {
        const COARSE_PERIOD_MASK = 0b0000_1111;
        const CHAN_CONTROL_MASK  = 0b0011_0000;
        const CHAN_A             = 0b0001_0000;
        const CHAN_B             = 0b0010_0000;
        const CHAN_C             = 0b0011_0000;
        const MFP_TIMER_RESTART  = 0b0100_0000;
        const FX_TYPE_MASK       = 0b1100_0000;
        const FX_TYPE_SID_VOICE  = 0b0000_0000;
        const FX_TYPE_DIGI_DRUM  = 0b0100_0000;
        const FX_TYPE_SINUS_SID  = 0b1000_0000;
        const FX_TYPE_SYNC_BUZZ  = 0b1100_0000;
    }
}

impl SongAttributes {
    /// Returns `true` if frame data was layed out interleaved.
    pub fn is_interleaved(self) -> bool {
        self.intersects(SongAttributes::INTERLEAVED)
    }

    /// Returns `true` if `DIGI-DRUM` data was 4-bit.
    pub fn is_4bit(self) -> bool {
        self.intersects(SongAttributes::DIGIDRUM_4BIT)
    }

    /// Returns `true` if `DIGI-DRUM` data was signed.
    pub fn is_signed(self) -> bool {
        self.intersects(SongAttributes::DIGIDRUM_SIGNED)
    }
}

impl FxCtrlFlags {
    /// Returns the tuple of timer restart boolean and the channel number `[0, 2]` if an
    /// effect is active. Otherwise returns `None`.
    pub fn ts_channel(self) -> Option<(bool, u8)> {
        FxChannel::from(self).channel().map(|ch|
            (self.intersects(FxCtrlFlags::MFP_TIMER_RESTART), ch)
        )
    }

    /// Returns the channel number `[0, 2]` if an effect is active. Otherwise returns `None`.
    pub fn dd_channel(self) -> Option<u8> {
        FxChannel::from(self).channel()
    }

    /// Returns the tuple of the effect type and the channel number `[0, 2]` if an
    /// effect is active. Otherwise returns `None`.
    pub fn fx6_channel(self) -> Option<(FxType, u8)> {
        FxChannel::from(self).channel().map(|ch|
            (FxType::from(self), ch)
        )
    }
}

impl From<FxCtrlFlags> for FxChannel {
    fn from(flags: FxCtrlFlags) -> Self {
        match flags & FxCtrlFlags::CHAN_CONTROL_MASK {             
            FxCtrlFlags::CHAN_A => FxChannel::RunOnA,
            FxCtrlFlags::CHAN_B => FxChannel::RunOnB,
            FxCtrlFlags::CHAN_C => FxChannel::RunOnC,
            _ => FxChannel::Idle,
        }
    }
}

impl From<FxCtrlFlags> for FxType {
    fn from(flags: FxCtrlFlags) -> Self {
        match flags & FxCtrlFlags::FX_TYPE_MASK {
            FxCtrlFlags::FX_TYPE_SID_VOICE => FxType::SidVoice,
            FxCtrlFlags::FX_TYPE_DIGI_DRUM => FxType::DigiDrum,
            FxCtrlFlags::FX_TYPE_SINUS_SID => FxType::SinusSid,
            FxCtrlFlags::FX_TYPE_SYNC_BUZZ => FxType::SyncBuzz,
            _ => unreachable!()
        }
    }
}

impl FxChannel {
    fn channel(self) -> Option<u8> {
        match self as u8 {
            0 => None,
            ch => Some(ch - 1)
        }
    }
}
