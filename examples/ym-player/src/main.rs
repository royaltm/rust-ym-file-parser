//! YM player
use spectrusty::audio::{
    synth::*,
    host::cpal::{AudioHandle, AudioHandleAnyFormat}
};
use spectrusty::audio::*;
use spectrusty::chip::nanos_from_frame_tc_cpu_hz;
use spectrusty::peripherals::ay::{audio::*, AyRegister, AyRegChange};
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

/* AY/YM channels mapped as follows: [LEFT RIGHT CENTER] */
const DEFAULT_CHANNEL_MAP: [usize; 3] = [0, 1, 2]; // ACB

/****************************************************************************/
/*                                  PLAYER                                  */
/****************************************************************************/

fn play<T>(
        mut audio: AudioHandle<T>,
        mut ym_file: YmSong,
        repeat: u32,
        volume: u8,
        mono_filter: f32,
        channel_map: [usize; 3]
    )
    where T: 'static + FromSample<f32> + AudioSample + cpal::Sample + Send,
          i16: IntoSample<T>,
{
    log::debug!("Repeat: {repeat}, volume: {volume}%, mono: {mono_filter}, channels: {channel_map:?}");

    /* Spectrusty's emulated AY is clocked at a half frequency of a host CPU clock,
       we need to adjust cycles counter */
    let host_frame_cycles = (ym_file.frame_cycles() * HOST_CLOCK_RATIO as f32) as i32;
    let host_frequency = ym_file.clock_frequency() as f64 * HOST_CLOCK_RATIO as f64;

    log::trace!("AY host frequency: {} Hz, frame: {} cycles", host_frequency, host_frame_cycles);

    let amp_filter = amplitude_level(volume);
    log::trace!("Amplitude filter: {amp_filter}");

    /* create an amplitude filtered band-limited pulse buffer with >= 3 channels */
    let mut bandlim = BlepAmpFilter::build(amp_filter)(
        /* a multi-channel to stereo mixer */
        BlepStereo::build(mono_filter)(
            /* a stereo band-limited pulse buffer */
            BandLimited::<f32>::new(2)));
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
        ay.render_audio::<AyAmps<f32>,_,_>(changes.drain(..),
                                           &mut bandlim,
                                           host_frame_cycles,
                                           host_frame_cycles,
                                           channel_map);
        /* close frame */
        let frame_sample_count = Blep::end_frame(&mut bandlim, host_frame_cycles);

        /* render BLEP frame into the sample buffer */
        audio.producer.render_frame(|ref mut buf| {
            /* ensure the BLEP frame fits into the sample buffer */
            buf.resize(frame_sample_count * channels, T::silence());
            /* an iterator of sample pairs (stereo channels) */
            let sample_iter = bandlim.sum_iter::<T>(0).zip(
                              bandlim.sum_iter::<T>(1))
                              .map(|(l,r)| [l,r]);
            /* render each sample */
            for (chans, samples) in buf.chunks_mut(channels)
                                       .zip(sample_iter)
            {
                /* write samples to the first two audio channels */
                for (p, sample) in chans.iter_mut()
                                        .zip(samples.into_iter())
                {
                    *p = sample;
                }
            }
        });

        /* send a rendered sample buffer to the consumer */
        audio.producer.send_frame().unwrap();
        /* prepare BLEP for the next frame */
        bandlim.next_frame();

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

    /// Mixer volume: 0 - 100.
    #[arg(short, long, default_value_t = 50, value_parser = volume_in_range)]
    volume: u8,

    /// Monophonic channel mixer amplitude: 0.0 - 1.0.
    #[arg(short, long, default_value_t = 0.8, value_parser = amp_in_range)]
    mono: f32,

    /// Play counter, 0 = play forever.
    #[arg(short, long, default_value_t = 0)]
    repeat: u32,

    /// YM channels map: Left Center Right [default: "ACB"].
    #[arg(short, long)]
    channels: Option<String>,

    /// Log verbosity level.
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

fn amp_in_range(s: &str) -> Result<f32, String> {
    let amp: f32 = s
        .parse()
        .map_err(|_| format!("`{s}` isn't an amplitude"))?;
    if amp >= 0.0 && amp <= 1.0 {
        Ok(amp)
    } else {
        Err("amplitude not in range 0.0 - 1.0".into())
    }
}

fn parse_channels(s: &str) -> Result<[usize; 3], String> {
    const ERROR_MSG: &str = "channel mapping should be a permutation of ABC characters";
    if s.len() != 3 {
        return Err(ERROR_MSG.into());
    }
    let mut channels = [usize::MAX; 3];
    for (ch, pos) in s.chars().zip([0, 2, 1].into_iter()) {
        let chan = match ch.to_ascii_uppercase() {
            'A' => 0,
            'B' => 1,
            'C' => 2,
            _ => return Err(ERROR_MSG.into())
        };
        if let Some(_) = channels.iter().find(|&&ch| chan == ch) {
            return Err(ERROR_MSG.into());
        }
        channels[pos] = chan;
    }
    Ok(channels)
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
    let audio = AudioHandleAnyFormat::create(&cpal::default_host(), frame_duration_nanos, 1)?;

    /* start audio thread */
    audio.play()?;

    let channels = args.channels.map(|s| parse_channels(&s)).transpose()?
                                .unwrap_or(DEFAULT_CHANNEL_MAP);
    let Args { volume, mono, repeat, .. } = args;

    match audio {
        AudioHandleAnyFormat::I16(audio) => play::<i16>(audio, ym_file, repeat, volume, mono, channels),
        AudioHandleAnyFormat::U16(audio) => play::<u16>(audio, ym_file, repeat, volume, mono, channels),
        AudioHandleAnyFormat::F32(audio) => play::<f32>(audio, ym_file, repeat, volume, mono, channels),
    }

    Ok(())
}
