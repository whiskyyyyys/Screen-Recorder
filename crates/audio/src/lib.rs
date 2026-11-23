use obs_core::SourceId;
use obs_filters::{linear_to_db, AudioFilterChainState, AudioFilterInstance};
use obs_scenes::{AudioMonitoringType, SourceAudioSettings};
use parking_lot::Mutex;
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

pub const MAX_AUDIO_TRACKS: usize = 6;
const MIN_METER_DB: f32 = -60.0;

#[derive(Debug, Clone, Copy, Default)]
pub struct MeterReading {
    pub peak_l_db: f32,
    pub peak_r_db: f32,
    pub rms_l_db: f32,
    pub rms_r_db: f32,
    pub hold_l_db: f32,
    pub hold_r_db: f32,
    pub clipping: bool,
}

/// OBS Studio `OBS_FADER_CUBIC` mapping: slider deflection [0..1] -> amplitude multiplier.
#[inline]
pub fn cubic_fader_to_mul(pos: f32) -> f32 {
    let p = pos.clamp(0.0, 1.5);
    p * p * p
}

/// Inverse of `OBS_FADER_CUBIC`: amplitude multiplier -> slider deflection.
#[inline]
pub fn mul_to_cubic_fader(mul: f32) -> f32 {
    mul.max(0.0).cbrt().clamp(0.0, 1.5)
}

#[derive(Debug, Clone)]
pub struct MixedAudioBlock {
    pub sample_rate: u32,
    pub frames: usize,
    /// Six independent interleaved stereo `f32` tracks for multi-track recording & streaming.
    pub tracks: [Vec<f32>; MAX_AUDIO_TRACKS],
    /// Local audio monitoring output buffer (interleaved stereo `f32`).
    pub monitor: Vec<f32>,
    /// Master meter reading from Track 1.
    pub master_meter: MeterReading,
}

struct ChannelState {
    settings: SourceAudioSettings,
    filters: Vec<AudioFilterInstance>,
    dsp: AudioFilterChainState,
    ring: VecDeque<f32>,
    delay_ring: VecDeque<f32>,
    last_sync_offset_ms: i32,
    meter: MeterReading,
    hold_timer_l: f32,
    hold_timer_r: f32,
}

impl ChannelState {
    fn new(settings: SourceAudioSettings, filters: Vec<AudioFilterInstance>) -> Self {
        Self {
            settings,
            filters,
            dsp: AudioFilterChainState::new(),
            ring: VecDeque::with_capacity(48000),
            delay_ring: VecDeque::with_capacity(9600),
            last_sync_offset_ms: 0,
            meter: MeterReading {
                peak_l_db: MIN_METER_DB,
                peak_r_db: MIN_METER_DB,
                rms_l_db: MIN_METER_DB,
                rms_r_db: MIN_METER_DB,
                hold_l_db: MIN_METER_DB,
                hold_r_db: MIN_METER_DB,
                clipping: false,
            },
            hold_timer_l: 0.0,
            hold_timer_r: 0.0,
        }
    }
}

/// Real-time 6-track floating-point audio mixer with DSP filters, sync offsets, and VU metering.
pub struct AudioMixer {
    sample_rate: Arc<Mutex<u32>>,
    channels: Arc<Mutex<HashMap<SourceId, ChannelState>>>,
    global_mono: Arc<Mutex<bool>>,
    meter_decay_db_per_sec: Arc<Mutex<f32>>,
}

impl AudioMixer {
    pub fn new(sample_rate: u32) -> Self {
        Self {
            sample_rate: Arc::new(Mutex::new(sample_rate.max(8000))),
            channels: Arc::new(Mutex::new(HashMap::new())),
            global_mono: Arc::new(Mutex::new(false)),
            meter_decay_db_per_sec: Arc::new(Mutex::new(23.53)),
        }
    }

    pub fn sample_rate(&self) -> u32 {
        *self.sample_rate.lock()
    }

    pub fn set_sample_rate(&self, sr: u32) {
        *self.sample_rate.lock() = sr.max(8000);
    }

    pub fn set_global_mono(&self, mono: bool) {
        *self.global_mono.lock() = mono;
    }

    pub fn set_meter_decay(&self, db_per_sec: f32) {
        *self.meter_decay_db_per_sec.lock() = db_per_sec.clamp(5.0, 80.0);
    }

    pub fn sync_sources(
        &self,
        desired: &[(SourceId, SourceAudioSettings, Vec<AudioFilterInstance>)],
    ) {
        let mut map = self.channels.lock();
        let active_ids: std::collections::HashSet<SourceId> =
            desired.iter().map(|(id, _, _)| *id).collect();
        map.retain(|id, _| active_ids.contains(id));
        for (id, settings, filters) in desired {
            if let Some(ch) = map.get_mut(id) {
                ch.settings = settings.clone();
                ch.filters = filters.clone();
            } else {
                map.insert(*id, ChannelState::new(settings.clone(), filters.clone()));
            }
        }
    }

    /// Pushes interleaved stereo `f32` audio samples from a capture/media thread into the source's ring buffer.
    pub fn push_interleaved_stereo(
        &self,
        source_id: SourceId,
        samples: &[f32],
        input_sample_rate: u32,
    ) {
        if samples.len() < 2 {
            return;
        }
        let target_sr = *self.sample_rate.lock();
        let resampled_owned: Vec<f32>;
        let src_slice = if input_sample_rate > 0 && input_sample_rate != target_sr {
            resampled_owned = resample_stereo_linear(samples, input_sample_rate, target_sr);
            &resampled_owned
        } else {
            samples
        };

        let mut map = self.channels.lock();
        let ch = map
            .entry(source_id)
            .or_insert_with(|| ChannelState::new(SourceAudioSettings::default(), Vec::new()));

        let max_samples = (target_sr as usize) * 2; // 1 second cap to prevent drift buildup
        for &s in src_slice {
            ch.ring.push_back(s);
        }
        while ch.ring.len() > max_samples {
            ch.ring.pop_front();
            ch.ring.pop_front();
        }
    }

    pub fn meters_snapshot(&self) -> HashMap<SourceId, MeterReading> {
        self.channels
            .lock()
            .iter()
            .map(|(id, ch)| (*id, ch.meter))
            .collect()
    }

    /// Mixes `num_frames` stereo frames across all active audio sources into 6 tracks + monitor bus.
    pub fn mix_block(&self, num_frames: usize) -> MixedAudioBlock {
        let sr = *self.sample_rate.lock();
        let global_mono = *self.global_mono.lock();
        let decay_rate = *self.meter_decay_db_per_sec.lock();
        let block_sec = (num_frames as f32) / (sr as f32);
        let decay_db = decay_rate * block_sec;

        let len = num_frames * 2;
        let mut tracks: [Vec<f32>; MAX_AUDIO_TRACKS] = [
            vec![0.0; len],
            vec![0.0; len],
            vec![0.0; len],
            vec![0.0; len],
            vec![0.0; len],
            vec![0.0; len],
        ];
        let mut monitor = vec![0.0; len];
        let mut work = vec![0.0f32; len];

        {
            let mut map = self.channels.lock();
            for ch in map.values_mut() {
                work.fill(0.0);
                let avail = ch.ring.len().min(len) & !1;
                for item in work.iter_mut().take(avail) {
                    if let Some(v) = ch.ring.pop_front() {
                        *item = v;
                    }
                }

                // Apply sync offset positive delay line (up to 2000ms)
                let offset_ms = ch.settings.sync_offset_ms.clamp(0, 2000);
                if offset_ms != ch.last_sync_offset_ms {
                    ch.delay_ring.clear();
                    let delay_samples = (((offset_ms as u64) * (sr as u64)) / 1000) as usize * 2;
                    ch.delay_ring.resize(delay_samples, 0.0);
                    ch.last_sync_offset_ms = offset_ms;
                }
                if !ch.delay_ring.is_empty() {
                    for s in work.iter_mut() {
                        ch.delay_ring.push_back(*s);
                        *s = ch.delay_ring.pop_front().unwrap_or(0.0);
                    }
                }

                // Run real-time Rust DSP filters (Gain, Gate, Expander, Compressor, Limiter, NS, EQ, Polarity)
                ch.dsp
                    .process_interleaved_stereo(&ch.filters, &mut work, sr);

                // Apply fader volume, balance, mono downmix, and mute
                let vol = if ch.settings.muted {
                    0.0
                } else {
                    ch.settings.volume_mul.max(0.0)
                };
                let bal = ch.settings.balance.clamp(-1.0, 1.0);
                let gain_l = vol * (1.0 - bal.max(0.0));
                let gain_r = vol * (1.0 + bal.min(0.0));
                let do_mono = global_mono || ch.settings.mono_downmix;

                let mut peak_l = 0.0f32;
                let mut peak_r = 0.0f32;
                let mut sum_sq_l = 0.0f32;
                let mut sum_sq_r = 0.0f32;

                for pair in work.as_chunks_mut::<2>().0 {
                    let (mut l, mut r) = (pair[0] * gain_l, pair[1] * gain_r);
                    if do_mono {
                        let m = 0.5 * (l + r);
                        l = m;
                        r = m;
                    }
                    pair[0] = l;
                    pair[1] = r;

                    let al = l.abs();
                    let ar = r.abs();
                    peak_l = peak_l.max(al);
                    peak_r = peak_r.max(ar);
                    sum_sq_l += l * l;
                    sum_sq_r += r * r;
                }

                let inv_n = 1.0 / (num_frames.max(1) as f32);
                let new_peak_l = linear_to_db(peak_l).clamp(MIN_METER_DB, 0.0);
                let new_peak_r = linear_to_db(peak_r).clamp(MIN_METER_DB, 0.0);
                let rms_l = linear_to_db((sum_sq_l * inv_n).sqrt()).clamp(MIN_METER_DB, 0.0);
                let rms_r = linear_to_db((sum_sq_r * inv_n).sqrt()).clamp(MIN_METER_DB, 0.0);

                ch.meter.peak_l_db = new_peak_l.max(ch.meter.peak_l_db - decay_db);
                ch.meter.peak_r_db = new_peak_r.max(ch.meter.peak_r_db - decay_db);
                ch.meter.rms_l_db = rms_l;
                ch.meter.rms_r_db = rms_r;
                ch.meter.clipping = peak_l >= 0.99 || peak_r >= 0.99;

                if new_peak_l >= ch.meter.hold_l_db {
                    ch.meter.hold_l_db = new_peak_l;
                    ch.hold_timer_l = 1.2;
                } else {
                    ch.hold_timer_l = (ch.hold_timer_l - block_sec).max(0.0);
                    if ch.hold_timer_l <= 0.0 {
                        ch.meter.hold_l_db = (ch.meter.hold_l_db - decay_db).max(MIN_METER_DB);
                    }
                }

                if new_peak_r >= ch.meter.hold_r_db {
                    ch.meter.hold_r_db = new_peak_r;
                    ch.hold_timer_r = 1.2;
                } else {
                    ch.hold_timer_r = (ch.hold_timer_r - block_sec).max(0.0);
                    if ch.hold_timer_r <= 0.0 {
                        ch.meter.hold_r_db = (ch.meter.hold_r_db - decay_db).max(MIN_METER_DB);
                    }
                }

                // Route to outputs
                if ch.settings.monitoring != AudioMonitoringType::MonitorOnly {
                    for (track_idx, track_buf) in tracks.iter_mut().enumerate() {
                        if (ch.settings.track_mask & (1 << track_idx)) != 0 {
                            for (dst, &src) in track_buf.iter_mut().zip(work.iter()) {
                                *dst += src;
                            }
                        }
                    }
                }
                if matches!(
                    ch.settings.monitoring,
                    AudioMonitoringType::MonitorOnly | AudioMonitoringType::MonitorAndOutput
                ) {
                    for (dst, &src) in monitor.iter_mut().zip(work.iter()) {
                        *dst += src;
                    }
                }
            }
        }

        // Soft-clip protection across all 6 tracks + monitor
        for buf in tracks.iter_mut().chain(std::iter::once(&mut monitor)) {
            for s in buf.iter_mut() {
                *s = s.clamp(-1.0, 1.0);
            }
        }

        let master_meter = compute_meter_from_buf(&tracks[0], num_frames);
        MixedAudioBlock {
            sample_rate: sr,
            frames: num_frames,
            tracks,
            monitor,
            master_meter,
        }
    }
}

fn compute_meter_from_buf(buf: &[f32], num_frames: usize) -> MeterReading {
    let mut peak_l = 0.0f32;
    let mut peak_r = 0.0f32;
    let mut sq_l = 0.0f32;
    let mut sq_r = 0.0f32;
    for pair in buf.as_chunks::<2>().0 {
        peak_l = peak_l.max(pair[0].abs());
        peak_r = peak_r.max(pair[1].abs());
        sq_l += pair[0] * pair[0];
        sq_r += pair[1] * pair[1];
    }
    let inv_n = 1.0 / (num_frames.max(1) as f32);
    MeterReading {
        peak_l_db: linear_to_db(peak_l).clamp(MIN_METER_DB, 0.0),
        peak_r_db: linear_to_db(peak_r).clamp(MIN_METER_DB, 0.0),
        rms_l_db: linear_to_db((sq_l * inv_n).sqrt()).clamp(MIN_METER_DB, 0.0),
        rms_r_db: linear_to_db((sq_r * inv_n).sqrt()).clamp(MIN_METER_DB, 0.0),
        hold_l_db: linear_to_db(peak_l).clamp(MIN_METER_DB, 0.0),
        hold_r_db: linear_to_db(peak_r).clamp(MIN_METER_DB, 0.0),
        clipping: peak_l >= 0.99 || peak_r >= 0.99,
    }
}

/// Linear stereo resampler for converting 44.1kHz <-> 48kHz capture streams cleanly.
pub fn resample_stereo_linear(input: &[f32], src_sr: u32, dst_sr: u32) -> Vec<f32> {
    let in_frames = input.len() / 2;
    if in_frames == 0 || src_sr == 0 || dst_sr == 0 {
        return Vec::new();
    }
    let ratio = (dst_sr as f64) / (src_sr as f64);
    let out_frames = ((in_frames as f64) * ratio).round().max(1.0) as usize;
    let mut out = Vec::with_capacity(out_frames * 2);
    for i in 0..out_frames {
        let pos = (i as f64) / ratio;
        let idx = (pos.floor() as usize).min(in_frames - 1);
        let next = (idx + 1).min(in_frames - 1);
        let frac = (pos - (idx as f64)) as f32;
        let l = input[idx * 2] * (1.0 - frac) + input[next * 2] * frac;
        let r = input[idx * 2 + 1] * (1.0 - frac) + input[next * 2 + 1] * frac;
        out.push(l);
        out.push(r);
    }
    out
}
