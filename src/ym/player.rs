use core::ops::Range;
use super::*;

use super::parse::YM2_SAMPLE_ENDS;

impl YmSong {
    /// Resets the state of the player.
    pub fn reset(&mut self) {
        self.cursor = 0;
        for (sv, ss, dd) in self.voice_effects.iter_mut() {
            sv.stop();
            ss.stop();
            dd.stop();
        }
        self.buzzer.stop();
    }

    /// Returns the current frame cursor value.
    pub fn cursor(&self) -> u32 {
        self.cursor as u32
    }

    fn fx_update(&mut self, fx: FxType, chan: u8, divisor: NonZeroU32, vol: u8) {
        let step = self.timer_interval(divisor);
        match fx {
            FxType::SidVoice => {
                // println!("SID voice on {} v: {} {} Hz", chan, vol & 0x0f, self.clock_frequency() as f32 / step);
                let sid_voice = &mut self.voice_effects[chan as usize].0;
                sid_voice.start(vol & 0x0f, step);
            }
            FxType::DigiDrum => {
                // println!("digi on {} sample: {} {} Hz", chan, sample, self.clock_frequency() as f32 / step);
                let Range { start, end } = self.sample_data_range(vol as usize);
                let ddrum = &mut self.voice_effects[chan as usize].2;
                ddrum.start(start, end, step);
            }
            FxType::SinusSid => {
                // println!("{} sinus SID on {} v: {} {} Hz", self.cursor, chan, vol, self.clock_frequency() as f32 / step);
                let sinus_sid = &mut self.voice_effects[chan as usize].1;
                sinus_sid.start(vol & 0x0f, step);
            }
            FxType::SyncBuzz => {
                // println!("buzzer on {} shape: {} {} Hz {}", chan, vol & 0x0f, self.clock_frequency() as f32 / step, step);
                self.buzzer.start(vol & 0x0f, step);
            }
        }
    }

    fn play_ym2_frame<F: FnMut(f32, u8, u8)>(&mut self, rec: &mut F) {
        let frame = &self.frames[self.cursor];
        let shape = frame.data[ENV_REG as usize];
        if shape != 0xff {
            rec(0.0, ENV_PER_FINE_REG, frame.data[ENV_PER_FINE_REG as usize]);
            rec(0.0, ENV_PER_COARSE_REG, 0);
            rec(0.0, ENV_REG, 0x10);
        }

        let vol_c = frame.data[VOL_C_REG as usize];
        if vol_c & 0x80 == 0x80 {
            let sample: usize = (vol_c & 0x7f).into();
            let prediv: u32 = frame.data[ENV_PER_COARSE_REG as usize].into();
            if let Some(&end) = YM2_SAMPLE_ENDS.get(sample) {
                if let Some(divisor) = NonZeroU32::new(4 * prediv) {
                    let step = self.timer_interval(divisor);
                    // println!("MADMAX digi sample: {} div: {} {} Hz", sample, divisor, self.clock_frequency() as f32 / step);
                    let cur = match sample {
                        0 => 0,
                        index => YM2_SAMPLE_ENDS[index - 1]
                    };
                    let ddrum = &mut self.voice_effects[2].2;
                    ddrum.start(cur, end, step);
                }
            }
        }
    }

    fn play_ym3_frame<F: FnMut(f32, u8, u8)>(&mut self, rec: &mut F) {
        let frame = &self.frames[self.cursor];
        for (val, reg) in frame.data[ENV_PER_FINE_REG as usize..].iter().copied().zip(ENV_PER_FINE_REG..ENV_REG) {
            rec(0.0, reg, val);
        }
        let shape = frame.data[ENV_REG as usize];
        if shape != 0xff {
            rec(0.0, ENV_REG, shape);
        }
    }

    fn play_ym5_frame<F: FnMut(f32, u8, u8)>(&mut self, rec: &mut F) {
        self.play_ym3_frame(rec);
        let frame = &self.frames[self.cursor];
        let ts = frame.fx0().ts_channel().and_then(|(reset_sid, chan)|
            frame.timer_divisor0().map(|div| (reset_sid, chan, div, frame.vol(chan)))
        );
        let dd = frame.fx1().dd_channel().and_then(|chan|
            frame.timer_divisor1().map(|div| (chan, div, frame.vol(chan)))
        );
        if let Some((reset_sid, chan, divisor, vol)) = ts {
            if reset_sid {
                self.voice_effects[chan as usize].0.reset();
            }
            self.fx_update(FxType::SidVoice, chan, divisor, vol);
        }
        if let Some((chan, divisor, vol)) = dd {
            self.fx_update(FxType::DigiDrum, chan, divisor, vol);
        }        
    }

    fn play_ym6_frame<F: FnMut(f32, u8, u8)>(&mut self, rec: &mut F) {
        self.play_ym3_frame(rec);
        let frame = &self.frames[self.cursor];

        let fx0 = frame.fx0().fx6_channel().and_then(|(fx, chan)|
            frame.timer_divisor0().map(|div| (fx, chan, div, frame.vol(chan)))
        );
        let fx1 = frame.fx1().fx6_channel().and_then(|(fx, chan)|
            frame.timer_divisor1().map(|div| (fx, chan, div, frame.vol(chan)))
        );
        if let Some((fx, chan, divisor, vol)) = fx0 {
            self.fx_update(fx, chan, divisor, vol);
        }
        if let Some((fx, chan, divisor, vol)) = fx1 {
            self.fx_update(fx, chan, divisor, vol);
        }
    }

    /// Produces the changes to the AY/YM chipset registers for the current frame indicated by
    /// the cursor and advances the cursor forward one frame.
    ///
    /// Provide a function that receives 3 arguments:
    /// * The timestamp as a cycle relative to the current frame, where `0.0` is the
    ///   beginning of a frame. The timestamp will be always larger than `0.0` and less than the
    ///   value returned from [YmSong::frame_cycles].
    /// * The modified register's number `[0, 13]`.
    /// * The modified register's new value.
    ///
    /// The changes are always being provided in the ascending order of the timestamp.
    ///
    /// Returns `true` if this was the last frame before the cursor has been set to the loop frame.
    /// Otherwise returns `false`.
    ///
    /// This method can be used to populate changes to the AY/YM chipset or an emulator, to play
    /// the YM-file song.
    pub fn produce_next_ay_frame<F: FnMut(f32, u8, u8)>(&mut self, mut rec: F) -> bool {
        for (sv, ss, ..) in self.voice_effects.iter_mut() {
            sv.stop();
            ss.stop();
        }
        self.buzzer.stop();

        match self.version {
            YmVersion::Ym2 => self.play_ym2_frame(&mut rec),
            YmVersion::Ym3 => self.play_ym3_frame(&mut rec),
            YmVersion::Ym4|
            YmVersion::Ym5 => self.play_ym5_frame(&mut rec),
            YmVersion::Ym6 => self.play_ym6_frame(&mut rec),
        }

        let cursor = self.cursor;
        let frame = self.frames[cursor];
        for (val, reg) in frame.data.iter().copied().zip(0..MIXER_REG) {
            rec(0.0, reg, val);
        }

        let mut chan_mix = frame.data[MIXER_REG as usize];

        let frame_cycles = self.frame_cycles();
        let mut voice_effects = &mut self.voice_effects[..];
        let mut frm_iters: [(Option<_>, Option<_>, Option<_>); 3] = Default::default();
        let mut tgt = frm_iters.iter_mut();
        let mut reg = VOL_A_REG;
        let mut chan_mask = 0b001001;
        while let Some((svssdd, rest)) = voice_effects.split_first_mut() {
            let (tsv, tss, tdd) = tgt.next().unwrap();
            let (ref mut sv, ref mut ss, ref mut dd) = svssdd;
            if let Some(iter) = sv.iter_frame(frame_cycles, reg) {
                *tsv = Some(iter);
            }
            else if let Some(iter) = ss.iter_frame(frame_cycles, reg) {
                *tss = Some(iter);
            }
            else if let Some(iter) = dd.iter_frame(frame_cycles,
                                                    reg,
                                                    &self.dd_samples,
                                                    frame.vol(reg))
            {
                chan_mix |= chan_mask;
                *tdd = Some(iter);
            }
            else {
                rec(0.0, reg, frame.vol(reg))
            }
            voice_effects = rest;
            reg += 1;
            chan_mask <<= 1;
        }

        rec(0.0, MIXER_REG, chan_mix);

        let mut buzzer_iter = self.buzzer.iter_frame(frame_cycles);
        let mut mixer: Mixer<_> = frm_iters.iter_mut().filter_map(iter_select).collect();
        if let Some(iter) = buzzer_iter.as_mut() {
            mixer.push(iter)
        }

        for (ts, reg, val) in mixer {
            rec(ts, reg, val)
        }

        let nframes = self.frames.len();
        match (cursor + 1) % nframes {
            0 => {
                self.cursor = (self.loop_frame as usize).min(nframes - 1);
                true
            }
            cursor => {
                self.cursor = cursor;
                false
            }
        }
    }
}
