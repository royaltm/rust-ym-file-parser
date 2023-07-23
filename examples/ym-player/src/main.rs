//! YM player
use std::io::{stdout, Write};
use core::ops::AddAssign;
use core::fmt;
use spectrusty_core::{audio::*, chip::nanos_from_frame_tc_cpu_hz};
use spectrusty_audio::{
    synth::ext::*,
    host::cpal::{AudioHandle, AudioHandleAnyFormat}
};
use spectrusty_peripherals::ay::{audio::*, AyRegister, AyRegChange};
use ym_file_parser::YmSong;
use clap::Parser;
use cpal::traits::*;

/* built-in song */
static BUZZ_YM: &[u8] = include_bytes!("../BUZZ.YM");

const NORMAL_AMPLITUDE: u8 = 100;

/* calculate amplitude level */
fn amplitude_level<T: Copy + FromSample<f32>>(level: u8) -> T {
    const A: f32 = 3.1623e-3;
    const B: f32 = 5.757;
    let y: f32 = match level {
        0  => 0.0,
        NORMAL_AMPLITUDE => 1.0,
        v => {
            let x = v as f32 / NORMAL_AMPLITUDE as f32;
            A * (B * x).exp()
        }
    };
    T::from_sample(y)
}

/* AY/YM channels mapped as follows: [A, B, C], where N -> 0: left, 1: right, 2: center */
#[derive(Debug, Clone, Copy)]
struct ChannelMap([usize; 3]);

impl fmt::Display for ChannelMap {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // [A, B, C], where N -> 0: left, 1: right, 2: center
        let [a, b, c] = self.0;
        if a == b && b == c {
            write!(f, "mono")
        }
        else {
            let mut res = ['?'; 3];
            res[a] = 'A';
            res[b] = 'B';
            res[c] = 'C';
            let [l, r, c] = res;
            write!(f, "{l}{c}{r}")
        }
    }
}

impl Default for ChannelMap {
    fn default() -> Self {
        ChannelMap([0, 1, 2]) // ACB
    }
}

const MONO_CHANNEL_MAP: ChannelMap = ChannelMap([0, 0, 0]);

/* How to mix YM audio channels */
#[derive(Debug, Clone, Copy)]
enum ChannelMode {
    /// Center channel is mixed-in with stereo channels.
    MixedStereo(f32),
    /// All channels are mixed-in together into a single audio channel.
    Mono,
    /// Left and right channel are played in stereo, redirect a center channel into a specific audio channel.
    Channel(u32)
}

impl Default for ChannelMode {
    fn default() -> Self {
        ChannelMode::MixedStereo(0.8)
    }
}

impl fmt::Display for ChannelMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ChannelMode::MixedStereo(ampl) => write!(f, "{ampl}"),
            ChannelMode::Mono => write!(f, "m"),
            ChannelMode::Channel(n) => write!(f, "{n}"),
        }
    }
}

fn print_time(secs: u32) {
    let hours = secs / 3600;
    let minutes = (secs % 3600) / 60;
    let secs = secs % 60;
    if hours != 0 {
        print!("{hours}:{minutes:02}:{secs:02}");
    }
    else {
        print!("{minutes:02}:{secs:02}");
    }
}

fn print_current(last_secs: &mut u32, cur_secs: f32, total_secs: f32) {
    let secs = cur_secs.trunc() as u32;
    if *last_secs == secs {
        return;
    }
    *last_secs = secs;
    print!("\r");
    print_time(secs);
    print!(" -> ");
    print_time((total_secs - cur_secs).trunc() as u32);
    stdout().flush().unwrap();
}

/****************************************************************************/
/*                                  PLAYER                                  */
/****************************************************************************/

struct PlayEnv {
    ym_file: YmSong,
    ampl_level: f32,
    repeat: u32,
    channel_map: ChannelMap,
    track: bool,
}

fn play_with_blep<A, B, SD, S>(
        PlayEnv { mut ym_file, ampl_level, repeat, channel_map, track }: PlayEnv,
        mut audio: AudioHandle<S>,
        bandlim: &mut B,
        render_audio: &dyn Fn(&mut BlepAmpFilter<&mut B>, &mut Vec<S>)
    )
    where A: AmpLevels<SD>,
          B: BandLimitedExt<SD, S> + ?Sized,
          SD: SampleDelta + FromSample<f32> + MulNorm,
          S: AudioSample + cpal::SizedSample
{
    log::debug!("Channels: {channel_map} {:?}", channel_map.0);
    /* Spectrusty's emulated AY is clocked at a half frequency of a host CPU clock,
       we need to adjust cycles counter */
    let host_frame_cycles = (ym_file.frame_cycles() * HOST_CLOCK_RATIO as f32) as i32;
    let host_frequency = ym_file.chipset_frequency as f64 * HOST_CLOCK_RATIO as f64;

    log::trace!("AY host frequency: {} Hz, frame: {} cycles", host_frequency, host_frame_cycles);

    /* create a BLEP amplitude filter wrapper */
    let mut bandlim = BlepAmpFilter::new(SD::from_sample(ampl_level), bandlim);

    /* ensure BLEP has enough space to fit a single audio frame
       (there is no margin - our frames will have constant size). */
    bandlim.ensure_frame_time(audio.sample_rate, host_frequency, host_frame_cycles, 0);

    /* number of audio output channels */
    let channels = audio.channels as usize;

    /* create an emulator instance */
    let mut ay = Ay3_891xAudio::default();
    /* buffered frame changes to AY-3-891x registers */
    let mut changes = Vec::new();

    /* play counter */
    let mut counter = repeat;

    /* total seconds */
    let total_secs = ym_file.frames.len() as f32 / ym_file.frame_frequency as f32;

    let mut last_secs: u32 = u32::MAX;

    loop {
        if track {
            let cur_secs = ym_file.cursor() as f32 / ym_file.frame_frequency as f32;
            print_current(&mut last_secs, cur_secs, total_secs);
        }

        /* produce YM chipset changes */
        let finished = ym_file.produce_next_ay_frame(|ts, reg, val| {
            changes.push(
                AyRegChange::new(
                    (ts * HOST_CLOCK_RATIO as f32).trunc() as i32,
                    AyRegister::from(reg),
                    val))
        });

        /* render audio into BLEP */
        ay.render_audio::<A,_,_>(changes.drain(..),
                                 &mut bandlim,
                                 host_frame_cycles,
                                 host_frame_cycles,
                                 channel_map.0);
        /* close frame */
        let frame_sample_count = bandlim.end_frame(host_frame_cycles);

        /* render BLEP frame into the sample buffer */
        audio.producer.render_frame(|ref mut buf| {
            /* ensure the BLEP frame fits into the sample buffer */
            buf.resize(frame_sample_count * channels, S::silence());
            render_audio(&mut bandlim, buf);
        });

        /* send a rendered sample buffer to the consumer */
        audio.producer.send_frame().unwrap();

        if finished {
            log::info!("Finished.");
            if repeat != 0 {
                counter -= 1;
                if counter == 0 {
                    break;
                }
            }
        }
    }

    /* let the audio thread finish playing */
    for _ in 0..50 {
        audio.producer.render_frame(|ref mut buf| {
            buf.fill(S::silence());
        });
        audio.producer.send_frame().unwrap();
    }
    audio.close();
}

fn play_with_amps<A, SD, S>(
        audio: AudioHandle<S>,
        ym_file: YmSong,
        args: Args
    )
    where A: AmpLevels<SD>,
          SD: SampleDelta + FromSample<f32> + AddAssign + MulNorm + 'static + std::fmt::Debug,
          S: FromSample<SD> + AudioSample + cpal::SizedSample
{
    let Args { volume, repeat, channels: channel_map, mode, track, hpass, lpass, .. } = args;
    log::debug!("Repeat: {repeat}, volume: {volume}%");

    let ampl_level = amplitude_level(args.volume);
    log::trace!("Amplitude filter: {ampl_level}");

    let mut env = PlayEnv { ym_file, ampl_level, repeat, channel_map, track };

    let channels = audio.channels as usize;

    match mode {
        ChannelMode::MixedStereo(mono_filter) if channels >= 2 => {
            /* a multi-channel to stereo mixer */
            let mut blep = BlepStereo::new(mono_filter.into_sample(), 
                /* a stereo band-limited pulse buffer */
                BandLimitedAny::new(2, lpass, hpass));
            log::debug!("Band limited: {blep:?}");
            let blep: &mut dyn BandLimitedExt<_, _> = &mut blep;
            play_with_blep::<A, _, _, _>(env, audio, blep,
                &|blep, buf| {
                    blep.render_audio_map_interleaved(buf, channels, &[0, 1]);
                    /* prepare BLEP for the next frame */
                    blep.next_frame_ext();
                }
            );
        }
        ChannelMode::Channel(channel) if channels >= channel as usize => {
            /* a multi-channel band-limited pulse buffer */
            let third_chan = (channel - 1) as usize;
            let mut blep = BandLimitedAny::new(3, lpass, hpass);
            log::debug!("Band limited: {blep:?}");
            let blep: &mut dyn BandLimitedExt<_, _> = &mut blep;
            play_with_blep::<A, _, _, _>(env, audio, blep,
                &|blep, buf| {
                    blep.render_audio_map_interleaved(buf, channels, &[0, 1, third_chan]);
                    /* prepare BLEP for the next frame */
                    blep.next_frame_ext();
                }
            );
        }
        _ => {
            /* a monophonic band-limited pulse buffer */
            let mut blep = BandLimitedAny::new(1, lpass, hpass);
            log::debug!("Band limited: {blep:?}");
            let blep: &mut dyn BandLimitedExt<_, _> = &mut blep;
            env.channel_map = MONO_CHANNEL_MAP;
            play_with_blep::<A, _, _, _>(env, audio, blep,
                &|blep, buf| {
                    blep.render_audio_fill_interleaved(buf, channels, 0);
                    /* prepare BLEP for the next frame */
                    blep.next_frame_ext();
                }
            );
        }
    }
}

fn play<SD, S>(
        audio: AudioHandle<S>,
        ym_file: YmSong,
        args: Args
    )
    where SD: SampleDelta + FromSample<f32> + AddAssign + MulNorm + 'static + std::fmt::Debug,
          S: FromSample<SD> + AudioSample + cpal::SizedSample,
          AyFuseAmps<SD>: AmpLevels<SD>,
          AyAmps<SD>: AmpLevels<SD>
{
    if args.fuse {
        log::debug!("YM amplitide levels: fuse (measured)");
        play_with_amps::<AyFuseAmps<_>, _, _>(audio, ym_file, args)
    }
    else {
        log::debug!("YM amplitide levels: default (specs)");
        play_with_amps::<AyAmps<_>, _, _>(audio, ym_file, args)
    }
}

/****************************************************************************/
/*                                   MAIN                                   */
/****************************************************************************/

#[derive(Default, Debug, Clone, Copy, PartialEq)]
struct StreamConfigHint {
    channels: Option<cpal::ChannelCount>,
    sample_rate: Option<cpal::SampleRate>,
    sample_format: Option<cpal::SampleFormat>
}

impl fmt::Display for StreamConfigHint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self == &StreamConfigHint::default() {
            return f.write_str("*");
        }
        if let Some(format) = self.sample_format {
            write!(f, "{:?}", format)?;
        }
        if self.channels.is_some() && self.sample_rate.is_some() {
            f.write_str(",")?;
        }
        if let Some(channels) = self.channels {
            write!(f, "{}", channels)?;
        }
        if let Some(rate) = self.sample_rate {
            write!(f, "@{}", rate.0)?;
        }
        Ok(())
    }
}

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// A file path to an YM song.
    ym_file: Option<String>,

    /// Audio mixer volume: 0 - 100.
    #[arg(short, long, default_value_t = 50, value_parser = volume_in_range)]
    volume: u8,

    /// Play counter, 0 to play forever.
    #[arg(short, long, default_value_t = 0)]
    repeat: u32,

    /// YM channels map: Left Center Right.
    #[arg(short, long, default_value_t = ChannelMap::default(), value_parser = parse_channels)]
    channels: ChannelMap,

    /// Channel mode: s|m|0.s|N.
    ///
    /// "s" - stereo mode with a center channel mixed with an amplitude of 0.8
    ///
    /// "m" - monophonic mode, played on all audio channels
    ///
    /// "0.s" - stereo mode, center channel amplitude: 0.s
    ///
    /// "N" - multi-channel mode, redirect center channel to Nth (3+) audio channel
    #[arg(short, long, default_value_t = ChannelMode::default(), value_parser = parse_channel_mode)]
    mode: ChannelMode,

    /// Switch to alternative YM amplitude levels (measured vs specs).
    #[arg(short, long, default_value_t = false)]
    fuse: bool,

    /// Enable low-pass audio band filter.
    #[arg(long, default_value_t = false)]
    lpass: bool,

    /// Enable high-pass audio band filter.
    #[arg(long, default_value_t = false)]
    hpass: bool,

    /// Desired audio output parameters: ST,CHANS@RATE.
    ///
    /// ST is a sample type, e.g.: U8, I16, U32, F32.
    /// 
    /// CHANS is the number of channels and RATE is the sample rate.
    #[arg(short, long, default_value_t = StreamConfigHint::default(), value_parser = parse_stream_config)]
    audio: StreamConfigHint,

    /// Track the current song time.
    #[arg(short, long, default_value_t = false)]
    track: bool,

    /// Log verbosity level.
    ///
    /// -d for INFO, -dd for DEBUG, -ddd for TRACE
    #[arg(short, long, action = clap::ArgAction::Count)]
    debug: u8
}

fn volume_in_range(s: &str) -> Result<u8, String> {
    let volume: usize = s
        .parse()
        .map_err(|_| format!("`{s}` isn't a volume"))?;
    if (0..=NORMAL_AMPLITUDE as usize).contains(&volume) {
        Ok(volume as u8)
    } else {
        Err(format!("volume not in range 0 - {NORMAL_AMPLITUDE}"))
    }
}

fn parse_channel_mode(s: &str) -> Result<ChannelMode, String> {
    Ok(match s {
        "s"|"S" => ChannelMode::MixedStereo(0.8),
        "m"|"M" => ChannelMode::Mono,
        s if s.starts_with("0.") => {
            let amp: f32 = s.parse().map_err(|_| format!("`{s}` isn't a stereo mixer amplitude"))?;
            ChannelMode::MixedStereo(amp)
        }
        s => {
            let channel: u32 = s.parse().map_err(|_| format!("`{s}` isn't a mixer mode channel"))?;
            if channel < 3 {
                return Err("mixer mode channel must be >= 3".into());
            }
            ChannelMode::Channel(channel)
        }
    })
}

fn parse_channels(s: &str) -> Result<ChannelMap, String> {
    const ERROR_MSG: &str = "channel mapping should be a permutation of ABC characters";
    if s.len() != 3 {
        return Err(ERROR_MSG.into());
    }
    let mut channels = [usize::MAX; 3];
    // [A, B, C], where N -> 0: left, 1: right, 2: center
    for (ch, chan) in s.chars().zip([0, 2, 1].into_iter()) {
        let pos = match ch.to_ascii_uppercase() {
            'A' => 0,
            'B' => 1,
            'C' => 2,
            _ => return Err(ERROR_MSG.into())
        };
        if channels[pos] != usize::MAX {
            return Err(ERROR_MSG.into());
        }
        channels[pos] = chan;
    }
    Ok(ChannelMap(channels))
}

fn parse_stream_config(mut s: &str) -> Result<StreamConfigHint, String> {
    let mut config = StreamConfigHint::default();
    if s == "*" {
        return Ok(config);
    }
    const FORMATS: &[([&str;2], cpal::SampleFormat)] = &[
             (["i8", "I8"], cpal::SampleFormat::I8),
             (["u8", "U8"], cpal::SampleFormat::U8),
             (["i16", "I16"], cpal::SampleFormat::I16),
             (["u16", "U16"], cpal::SampleFormat::U16),
             (["i32", "I32"], cpal::SampleFormat::I32),
             (["u32", "U32"], cpal::SampleFormat::U32),
             (["f32", "F32"], cpal::SampleFormat::F32),
             (["i64", "I64"], cpal::SampleFormat::I64),
             (["u64", "U64"], cpal::SampleFormat::U64),
             (["f64", "F64"], cpal::SampleFormat::F64)];
    for ([lc, uc], format) in FORMATS.into_iter() {
        if s.starts_with(lc) || s.starts_with(uc) {
            config.sample_format = Some(*format);
            (_, s) = s.split_at(lc.len());
            break;
        }
    }
    if s.starts_with(",") {
        (_, s) = s.split_at(1);
    }
    let chan = match s.split_once("@") {
        Some((chan, rate)) => {
            if !rate.is_empty() {
                config.sample_rate = Some(cpal::SampleRate(u32::from_str_radix(rate, 10)
                                     .map_err(|_| "expected sample rate")?));
            }
            chan
        },
        None => s
    };
    if !chan.is_empty() {
        config.channels = Some(u16::from_str_radix(chan, 10)
                          .map_err(|_| "expected number of channels")?);
    }
    Ok(config)
}

fn find_best_audio_config(device: &cpal::Device, request: StreamConfigHint) -> Result<cpal::SupportedStreamConfig, Box<dyn std::error::Error>>
{
    log::trace!("Audio device: {}", device.name().unwrap_or_else(|e| e.to_string()));
    let default_config = device.default_output_config()?;
    if request == StreamConfigHint::default() {
        return Ok(default_config);
    }
    let channels = request.channels.unwrap_or(default_config.channels());
    for config in device.supported_output_configs()? {
        if config.channels() != channels {
            continue;
        }
        if let Some(sample_format) = request.sample_format {
            if config.sample_format() != sample_format {
                continue;
            }
        }
        else if config.sample_format() != default_config.sample_format() {
            continue;
        }
        let sample_rate = match request.sample_rate {
            Some(sample_rate) => if !(config.min_sample_rate()..=config.max_sample_rate()).contains(&sample_rate) {
                continue;
            }
            else {
                sample_rate
            }
            None => default_config.sample_rate()
        };
        return Ok(config.with_sample_rate(sample_rate));
    }
    Err("Could not find the audio configuration matching given parameters")?
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    simple_logger::init_with_level(match args.debug {
        0 => log::Level::Warn,
        1 => log::Level::Info,
        2 => log::Level::Debug,
        _ => log::Level::Trace
    })?;

    let ym_file = match args.ym_file {
        Some(ref ym_path) => {
            log::info!("Loading YM file: {}", ym_path);
            ym_file_parser::parse_file(ym_path)?
        }
        None => YmSong::parse(BUZZ_YM)?
    };

    log::info!(r#"{} "{}" by {}"#,
        ym_file.version,
        ym_file.title.trim(),
        ym_file.author.trim());

    log::info!(r#"Duration: {:?} {}"#,
        ym_file.song_duration(),
        ym_file.comments.trim());

    log::debug!("Chip: {} Hz, frame: {} Hz, {} cycles each",
        ym_file.clock_frequency(),
        ym_file.frame_frequency,
        ym_file.frame_cycles());

    log::debug!("Frames total: {}, loop to: {}, {:?}",
        ym_file.frames.len(),
        ym_file.loop_frame,
        ym_file.song_attrs);

    if log::log_enabled!(log::Level::Debug) && !ym_file.dd_samples.is_empty() {
        let mut sample_lens = Vec::with_capacity(ym_file.dd_samples_ends.len());
        ym_file.dd_samples_ends.iter().try_fold(0,
            |prev, &off| {
                (off != 0).then(|| {
                    sample_lens.push(off - prev);
                    off
                })
            });
        log::debug!("Drums: {}, sample lengths: {sample_lens:?}, total: {}",
                sample_lens.len(), ym_file.dd_samples.len());
    }

    /* calculate a duration of a single frame */
    let frame_duration_nanos = nanos_from_frame_tc_cpu_hz(
                                 ym_file.frame_cycles().round() as u32,
                                 ym_file.chipset_frequency) as u32;

    log::trace!("Frame duration: {} ns", frame_duration_nanos);

    let device = cpal::default_host().default_output_device().ok_or("no default audio device!")?;
    log::debug!("Audio request: {}", args.audio);
    let supported_config = find_best_audio_config(&device, args.audio)?;
    log::trace!("Audio config supported: {supported_config:?}");
    let config = supported_config.config();

    // if let &cpal::SupportedBufferSize::Range { min, max } = supported_config.buffer_size() {
    //     let frame_duration_secs = core::time::Duration::from_nanos(frame_duration_nanos.into()).as_secs_f64();
    //     let audio_frame_samples = (config.sample_rate.0 as f64 * frame_duration_secs).ceil() as cpal::FrameCount;
    //     if (min..=max).contains(&audio_frame_samples) {
    //         config.buffer_size = cpal::BufferSize::Fixed(audio_frame_samples);
    //     }
    // }

    /* create an audio backend */
    log::trace!("Audio config selected: {config:?}");
    let latency = 20000000 / frame_duration_nanos as usize + 5;
    let audio = AudioHandleAnyFormat::create_with_device_config_and_sample_format(
                    &device, &config, supported_config.sample_format(), frame_duration_nanos, latency)?;

    log::trace!("Audio format: {:?}", audio.sample_format());

    /* start audio thread */
    audio.play()?;

    match audio {
        AudioHandleAnyFormat::I8(audio)  => play::<i16, _>(audio, ym_file, args),
        AudioHandleAnyFormat::U8(audio)  => play::<i16, _>(audio, ym_file, args),
        AudioHandleAnyFormat::I16(audio) => play::<i16, _>(audio, ym_file, args),
        AudioHandleAnyFormat::U16(audio) => play::<i16, _>(audio, ym_file, args),
        AudioHandleAnyFormat::I32(audio) => play::<i32, _>(audio, ym_file, args),
        AudioHandleAnyFormat::U32(audio) => play::<i32, _>(audio, ym_file, args),
        AudioHandleAnyFormat::I64(audio) => play::<f64, _>(audio, ym_file, args),
        AudioHandleAnyFormat::U64(audio) => play::<f64, _>(audio, ym_file, args),
        AudioHandleAnyFormat::F32(audio) => play::<f32, _>(audio, ym_file, args),
        AudioHandleAnyFormat::F64(audio) => play::<f64, _>(audio, ym_file, args),
        _ => Err("Unsupported audio sample format!")?
    }

    Ok(())
}
