//! Special effects.
use core::cmp::Ordering;
use core::iter::{self, Peekable, FromIterator};
use arrayvec::ArrayVec;

use lazy_static::lazy_static;

pub const MIXER_REG: u8 = 7;
pub const VOL_A_REG: u8 = 8;
pub const VOL_B_REG: u8 = 9;
pub const VOL_C_REG: u8 = 10;
pub const ENV_PER_FINE_REG: u8 = 11;
pub const ENV_PER_COARSE_REG: u8 = 12;
pub const ENV_REG: u8 = 13;

/// The timer type, used by all of the special effects.
#[derive(Debug, Default, Clone, Copy)]
struct Timer {
    current: f32,
    step: f32
}

/// The `Sync Buzzer` effect writes periodically into the AY/YM register 13 a set up shape value,
/// which resets the chipset's internal volume envelope control timer.
#[derive(Debug, Default, Clone, Copy)]
pub struct SyncBuzzer {
    timer: Timer,
    shape: u8,
    active: bool
}

/// The `SID voice` effect modulates the channel's volume alternating between 0 and some set up value.
#[derive(Debug, Default, Clone, Copy)]
pub struct SidVoice {
    timer: Timer,
    vol: u8,
    cur: bool,
    active: bool
}

/// The `Sinus SID` effect modulates the channel's volume, by applying the scaled sinusoid shape with
/// the period of 8 samples, with the set up amplitude.
#[derive(Debug, Default, Clone, Copy)]
pub struct SinusSid {
    timer: Timer,
    amplitude: u8,
    phase: u8,
    active: bool
}

/// The `DIGI-DRUM` effect modulates the channel's volume level, by applying to it 4-bit sample values.
#[derive(Debug, Default, Clone, Copy)]
pub struct DigiDrum {
    timer: Timer,
    cur: usize,
    end: usize
}

#[derive(Debug)]
struct TimerIter<'a> {
    timer: &'a mut Timer,
    limit: f32
}

pub(super) struct Mixer<I: Iterator> {
    iters: ArrayVec<Peekable<I>, 4>
}

const SINUS_SID_PERIOD: usize = 8;
const SINUS_SID_MASK: usize = SINUS_SID_PERIOD - 1;

lazy_static! {
    static ref SINUS_SID: [u8;SINUS_SID_PERIOD] = {
        use core::f32::consts::PI;
        let mut sinus_sid = [0u8;SINUS_SID_PERIOD];
        for (n, p) in sinus_sid.iter_mut().enumerate() {
            let x = 2.0 * PI * n as f32 / SINUS_SID_PERIOD as f32;
            *p = ((x.cos() * 0.5 + 0.5) * 255.0).round() as u8;
        }
        sinus_sid
    };
}

#[inline(always)]
fn sinus_sid(phase: usize, vol: u16) -> u8 {
    ((SINUS_SID[phase & SINUS_SID_MASK] as u16 * vol + 127) / 255) as u8
}

impl<'a> TimerIter<'a> {
    #[inline]
    fn new(timer: &'a mut Timer, limit: f32) -> TimerIter<'a> {
        TimerIter { timer, limit }
    }

    #[inline]
    fn force_end(&mut self) {
        self.timer.current = self.limit;
    }
}

impl<'a> Iterator for TimerIter<'a> {
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        let Timer { current, step } = *self.timer;
        if current < self.limit {
            self.timer.current = current + step;
            return Some(current)
        }
        self.timer.current -= self.limit;
        None
    }
}

impl Timer {
    fn reset(&mut self) {
        self.current = 0.0;
    }

    fn set_step(&mut self, step: f32) {
        assert!(step > f32::EPSILON);
        self.step = step;
    }

    /// Returns the number of ticks
    fn fast_forward(&mut self, limit: f32) -> f32 {
        let step = self.step;
        if step >= f32::EPSILON {
            let rest = limit - self.current;
            self.current = step - (rest % step);
            return (rest / step).trunc()
        }
        0.0
    }
}

impl SyncBuzzer {
    pub fn stop(&mut self) {
        self.active = false;
    }

    pub fn start(&mut self, shape: u8, step: f32) {
        self.timer.set_step(step);
        self.shape = shape & 0x0f;
        self.active = true;
    }

    pub fn iter_frame<'a>(&'a mut self, limit: f32) -> Option<impl Iterator<Item=(f32, u8, u8)> + 'a> {
        if self.active {
            let shape = self.shape;
            return Some(
                TimerIter::new(&mut self.timer, limit)
                           .map(move |ts| (ts, ENV_REG, shape) )
            )
        }
        None
    }
}

impl SidVoice {
    pub fn is_active(&self) -> bool {
        self.active
    }

    pub fn stop(&mut self) {
        self.active = false;
    }

    pub fn reset(&mut self) {
        self.timer.reset();
        self.cur = false;
    }

    pub fn start(&mut self, vol: u8, step: f32) {
        self.timer.set_step(step);
        self.vol = vol;
        self.active = true;
    }

    pub fn iter_frame<'a>(
            &'a mut self,
            limit: f32,
            reg: u8
        ) -> Option<impl Iterator<Item=(f32, u8, u8)> + 'a>
    {
        if self.active {
            let vol = self.vol;
            let cur = &mut self.cur;
            return Some(
                TimerIter::new(&mut self.timer, limit).map(move |ts| {
                    let res = *cur;
                    *cur = !res;
                    let v = if res { 0 } else { vol };
                    (ts, reg, v)
                })
            )
        }
        if self.timer.fast_forward(limit) as u32 & 1 == 1 {
            self.cur = !self.cur;
        }
        None
    }
}

impl DigiDrum {
    pub fn is_active(&self) -> bool {
        self.cur < self.end
    }

    pub fn stop(&mut self) {
        self.end = 0;
    }

    pub fn start(&mut self, start: usize, end: usize, step: f32) {
        self.timer.reset();
        self.timer.set_step(step);
        self.cur = start;
        self.end = end;
    }

    pub fn iter_frame<'a, 'b: 'a>(
            &'a mut self,
            limit: f32,
            reg: u8,
            dd_samples: &'b [u8],
            end_vol: u8
        ) -> Option<impl Iterator<Item=(f32, u8, u8)> + 'a>
    {
        let end = self.end;
        let cur = &mut self.cur;
        if *cur < end {
            let mut samples = dd_samples[*cur..end].iter();
            let mut timer = TimerIter::new(&mut self.timer, limit);
            return Some(iter::from_fn(move || {
                match timer.next() {
                    Some(ts) => {
                        match samples.next() {
                            Some(vol) => Some((ts, reg, *vol)),
                            None => {
                                timer.force_end();
                                Some((ts, reg, end_vol))
                            }
                        }
                    }
                    None => {
                        *cur = end - samples.len();
                        None
                    }
                }
            }))
        }
        None
    }
}

impl SinusSid {
    pub fn is_active(&self) -> bool {
        self.active
    }

    pub fn stop(&mut self) {
        self.active = false;
    }

    pub fn start(&mut self, amplitude: u8, step: f32) {
        self.timer.set_step(step);
        self.amplitude = amplitude;
        self.active = true;
    }

    pub fn iter_frame<'a>(
            &'a mut self,
            limit: f32,
            reg: u8,
        ) -> Option<impl Iterator<Item=(f32, u8, u8)> + 'a>
    {
        if self.active {
            let amplitude = self.amplitude as u16;
            let phase = &mut self.phase;
            return Some(
                TimerIter::new(&mut self.timer, limit).map(move |ts| {
                    let ph = *phase;
                    let v = sinus_sid(ph as usize, amplitude);
                    *phase = (ph + 1) & SINUS_SID_MASK as u8;
                    (ts, reg, v as u8)
                })
            )
        }
        None
    }

}

impl<I: Iterator> Mixer<I> {
    pub(super) fn push(&mut self, iter: I) {
        self.iters.push(iter.peekable())
    }
}

impl<I> Iterator for Mixer<I>
    where I: Iterator<Item=(f32, u8, u8)>,
{
    type Item = (f32, u8, u8);

    fn next(&mut self) -> Option<Self::Item> {
        if let Some((pos, ..)) = self.iters.iter_mut().map(Peekable::peek).enumerate()
                                     .min_by(|(_, a), (_, b)|
            match (a, b) {
                (Some((ta, ..)), Some((tb, ..))) => ta.partial_cmp(tb).unwrap(),
                (Some(..), None) => Ordering::Less,
                (None, Some(..)) => Ordering::Greater,
                (None, None) => Ordering::Equal
            })
        {
            self.iters[pos].next()
        }
        else {
            None
        }
    }
}

pub(super) fn iter_select<'a, A: Iterator<Item=(f32, u8, u8)>,
                       B: Iterator<Item=(f32, u8, u8)>,
                       C: Iterator<Item=(f32, u8, u8)>>(
        (it_a, it_b, it_c): &'a mut (Option<A>, Option<B>, Option<C>)
    ) -> Option<&'a mut dyn Iterator<Item=(f32, u8, u8)>>
{
    if let Some(it) = it_a.as_mut() {
        Some(it)
    }
    else if let Some(it) = it_b.as_mut() {
        Some(it)
    }
    else if let Some(it) = it_c.as_mut() {
        Some(it)
    }
    else {
        None
    }
}

impl<I> FromIterator<I> for Mixer<I>
    where I: Iterator<Item=(f32, u8, u8)>
{
    fn from_iter<T: IntoIterator<Item = I>>(
            iter: T
        ) -> Self
    {
        let iters = iter.into_iter().map(Iterator::peekable).collect();
        Mixer { iters }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sinus_sid_works() {
        assert_eq!(SINUS_SID_PERIOD, 8);
        use core::f32::consts::PI;
        for vol in 0..16 {
            println!("vol: {}", vol);
            for n in 0..SINUS_SID_PERIOD {
                let x = 2.0 * PI * n as f32 / SINUS_SID_PERIOD as f32;
                let y = x.cos() * 0.5 + 0.5;
                let v0 = (y * vol as f32).round() as u8;
                let v1 = sinus_sid(n as usize, vol);
                println!("{:02}: {:02} {:02} {:03} {:.8} {:.8}", n, v0, v1, SINUS_SID[n as usize], y, x);
                assert_eq!(v0, v1);
            }
        }
    }
}
