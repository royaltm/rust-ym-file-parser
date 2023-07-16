//! YM player
use std::io::{stdout, Write};
use core::ops::AddAssign;
use core::fmt;
use spectrusty_core::{audio::*, chip::nanos_from_frame_tc_cpu_hz};
use spectrusty_audio::{
    synth::*,
    host::cpal::{AudioHandle, AudioHandleAnyFormat}
};
use spectrusty_peripherals::ay::{audio::*, AyRegister, AyRegChange};
use ym_file_parser::YmSong;
use clap::Parser;

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

struct PlayEnv {
    ym_file: YmSong,
    repeat: u32,
    channel_map: ChannelMap,
    track: bool
}

/****************************************************************************/
/*                                  PLAYER                                  */
/****************************************************************************/

fn play<SD, S>(
        fuse: bool,
        audio: AudioHandle<S>,
        env: PlayEnv,
        mode: ChannelMode,
        volume: u8
    )
    where SD: SampleDelta + FromSample<f32> + AddAssign + MulNorm,
          S: FromSample<SD> + AudioSample + cpal::Sample,
          AyFuseAmps<SD>: AmpLevels<SD>,
          AyAmps<SD>: AmpLevels<SD>
{
    if fuse {
        play_with_amps::<AyFuseAmps<_>, _, _>(audio, env, mode, volume)
    }
    else {
        play_with_amps::<AyAmps<_>, _, _>(audio, env, mode, volume)
    }
}

fn play_with_amps<A, SD, S>(
        audio: AudioHandle<S>,
        mut env: PlayEnv,
        mode: ChannelMode,
        volume: u8
    )
    where SD: SampleDelta + FromSample<f32> + AddAssign + MulNorm,
          A: AmpLevels<SD>,
          S: FromSample<SD> + AudioSample + cpal::Sample
{
    log::debug!("Repeat: {}, volume: {volume}%", env.repeat);

    let ampl_level = amplitude_level(volume);
    log::trace!("Amplitude filter: {ampl_level}");
    let ampl_level = SD::from_sample(ampl_level);

    let channels = audio.channels as usize;

    match mode {
        ChannelMode::MixedStereo(mono_filter) if channels >= 2 => {
            log::debug!("Mixer: stereo with filter: {mono_filter}");
            /* a multi-channel to stereo mixer */
            let blep = BlepStereo::build(mono_filter.into_sample())(
                /* a stereo band-limited pulse buffer */
                BandLimited::<SD>::new(2));
            play_with_blep::<A, _, _, _, _>(audio, env, blep, ampl_level,
                |blep, buf| {
                    /* an iterator of sample pairs (stereo channels) */
                    let sample_iter = blep.sum_iter::<S>(0).zip(
                                      blep.sum_iter::<S>(1));
                    /* render each sample */
                    for (chans, (l, r)) in buf.chunks_mut(channels)
                                              .zip(sample_iter)
                    {
                        /* write samples to the first two audio channels */
                        chans[0..2].copy_from_slice(&[l,r]);
                    }
                    /* prepare BLEP for the next frame */
                    blep.next_frame();
                }
            );
        }
        ChannelMode::Channel(channel) if channels >= channel as usize => {
            log::debug!("Mixer: center played on audio channel: {}", channel);
            /* a multi-channel band-limited pulse buffer */
            let blep = BandLimited::<SD>::new(3);
            let third_chan = (channel - 1) as usize;
            play_with_blep::<A, _, _, _, _>(audio, env, blep, ampl_level,
                |blep, buf| {
                    /* an iterator of sample pairs (stereo channels) */
                    let sample_iter = blep.sum_iter::<S>(0).zip(
                                      blep.sum_iter::<S>(1)).zip(
                                      blep.sum_iter::<S>(2));
                    /* render each sample */
                    for (chans, ((l,r),c)) in buf.chunks_mut(channels)
                                                 .zip(sample_iter)
                    {
                        /* write samples to the first two audio channels */
                        chans[0..2].copy_from_slice(&[l,r]);
                        /* write a sample to the center audio channel */
                        chans[third_chan] = c;
                    }
                    /* prepare BLEP for the next frame */
                    blep.next_frame();
                }
            );
        }
        _ => {
            log::debug!("Mixer: mono");
            /* a monophonic band-limited pulse buffer */
            let blep = BandLimited::<SD>::new(1);
            env.channel_map = MONO_CHANNEL_MAP;
            play_with_blep::<A, _, _, _, _>(audio, env, blep, ampl_level,
                |blep, buf| {
                    for (chans, sample) in buf.chunks_mut(channels)
                                              .zip(blep.sum_iter::<S>(0))
                    {
                        /* write samples to all audio channels */
                        chans.fill(sample);
                    }
                    /* prepare BLEP for the next frame */
                    blep.next_frame();
                }
            );
        }
    }
}

fn play_with_blep<A, SD, S, B, F>(
        mut audio: AudioHandle<S>,
        env: PlayEnv,
        bandlim: B,
        ampl_level: SD,
        render_audio: F
    )
    where A: AmpLevels<SD>,
          SD: SampleDelta + MulNorm,
          S: AudioSample + cpal::Sample,
          B: Blep<SampleDelta=SD>,
          F: Fn(&mut B, &mut Vec<S>)
{
    let PlayEnv { mut ym_file, repeat, channel_map, track } = env;
    log::debug!("Channels: {channel_map} {:?}", channel_map.0);
    /* Spectrusty's emulated AY is clocked at a half frequency of a host CPU clock,
       we need to adjust cycles counter */
    let host_frame_cycles = (ym_file.frame_cycles() * HOST_CLOCK_RATIO as f32) as i32;
    let host_frequency = ym_file.chipset_frequency as f64 * HOST_CLOCK_RATIO as f64;

    log::trace!("AY host frequency: {} Hz, frame: {} cycles", host_frequency, host_frame_cycles);

    /* create an BLEP amplitude filter wrapper */
    let mut bandlim = BlepAmpFilter::build(ampl_level)(bandlim);

    /* ensure BLEP has enough space to fit a single audio frame
       (there is no margin - our frames will have constant size). */
    bandlim.ensure_frame_time(audio.sample_rate, host_frequency, host_frame_cycles, 0);

    /* number of audio output channels */
    let channels = audio.channels as usize;

    log::debug!("Audio playback: {} Hz, {channels} ch.", audio.sample_rate);

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

        if track {
            let cur_secs = ym_file.cursor() as f32 / ym_file.frame_frequency as f32;
            print_current(&mut last_secs, cur_secs, total_secs);
        }

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
}

/****************************************************************************/
/*                                   MAIN                                   */
/****************************************************************************/

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

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    simple_logger::init_with_level(match args.debug {
        0 => log::Level::Warn,
        1 => log::Level::Info,
        2 => log::Level::Debug,
        _ => log::Level::Trace
    })?;

    let ym_file = match args.ym_file {
        Some(ym_path) => {
            log::info!("Loading YM file: {}", ym_path);
            ym_file_parser::parse_file(ym_path)?
        }
        None => YmSong::parse(BUZZ_YM)?
    };

    log::info!(r#"{} "{}" by {}, {}, duration: {:?}"#,
        ym_file.version,
        ym_file.title.trim(),
        ym_file.author.trim(),
        ym_file.comments.trim(),
        ym_file.song_duration());

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

    /* create an audio backend */
    let audio = AudioHandleAnyFormat::create(&cpal::default_host(), frame_duration_nanos, 5)?;

    /* start audio thread */
    audio.play()?;

    let Args { volume, repeat, channels, mode, fuse, track, .. } = args;

    let env = PlayEnv { ym_file, repeat, channel_map: channels, track };

    match audio {
        AudioHandleAnyFormat::I16(audio) => {
            log::trace!("Audio format: I16");
            play::<i16, _>(fuse, audio, env, mode, volume)
        },
        AudioHandleAnyFormat::U16(audio) => {
            log::trace!("Audio format: U16");
            play::<i16, _>(fuse, audio, env, mode, volume)
        }
        AudioHandleAnyFormat::F32(audio) => {
            log::trace!("Audio format: F32");
            play::<f32, _>(fuse, audio, env, mode, volume)
        }
    }

    Ok(())
}
