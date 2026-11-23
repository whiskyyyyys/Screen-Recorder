use crate::audio_filters::{AudioFilterInstance, AudioFilterKind, NoiseSuppressionMethod};
use obs_core::FilterId;
use std::collections::HashMap;
use std::f32::consts::PI;

const MIN_DB: f32 = -96.0;
const EPS: f32 = 1e-9;

#[inline]
pub fn db_to_linear(db: f32) -> f32 {
    if db <= MIN_DB {
        0.0
    } else {
        10.0_f32.powf(db / 20.0)
    }
}

#[inline]
pub fn linear_to_db(lin: f32) -> f32 {
    if lin <= EPS {
        MIN_DB
    } else {
        (20.0 * lin.log10()).max(MIN_DB)
    }
}

#[inline]
fn time_coeff(ms: f32, sample_rate: f32) -> f32 {
    let sec = (ms.max(0.05)) * 0.001;
    (-1.0 / (sec * sample_rate.max(8000.0))).exp()
}

#[derive(Debug, Clone, Default)]
struct GateRuntime {
    is_open: bool,
    hold_samples: usize,
    gain: f32,
}

#[derive(Debug, Clone, Default)]
struct DynamicsRuntime {
    envelope: f32,
    gain_db: f32,
}

#[derive(Debug, Clone)]
struct NoiseSuppressRuntime {
    noise_floor: f32,
    speech_env: f32,
    gain: f32,
    hp_prev_in_l: f32,
    hp_prev_out_l: f32,
    hp_prev_in_r: f32,
    hp_prev_out_r: f32,
}

impl Default for NoiseSuppressRuntime {
    fn default() -> Self {
        Self {
            noise_floor: 0.001,
            speech_env: 0.0,
            gain: 1.0,
            hp_prev_in_l: 0.0,
            hp_prev_out_l: 0.0,
            hp_prev_in_r: 0.0,
            hp_prev_out_r: 0.0,
        }
    }
}

#[derive(Debug, Clone, Default)]
struct BiquadChannel {
    x1: f32,
    x2: f32,
    y1: f32,
    y2: f32,
}

impl BiquadChannel {
    #[inline]
    fn process(&mut self, x: f32, b0: f32, b1: f32, b2: f32, a1: f32, a2: f32) -> f32 {
        let y = b0 * x + b1 * self.x1 + b2 * self.x2 - a1 * self.y1 - a2 * self.y2;
        self.x2 = self.x1;
        self.x1 = x;
        self.y2 = self.y1;
        self.y1 = y;
        y
    }
}

#[derive(Debug, Clone, Default)]
struct EqRuntime {
    low_l: BiquadChannel,
    low_r: BiquadChannel,
    mid_l: BiquadChannel,
    mid_r: BiquadChannel,
    high_l: BiquadChannel,
    high_r: BiquadChannel,
}

#[derive(Debug, Clone, Default)]
enum FilterDspState {
    #[default]
    Stateless,
    Gate(GateRuntime),
    Dynamics(DynamicsRuntime),
    NoiseSuppress(NoiseSuppressRuntime),
    Eq(EqRuntime),
}

/// Stateful per-source real-time audio filter processor chain operating on interleaved stereo `f32`.
#[derive(Debug, Clone, Default)]
pub struct AudioFilterChainState {
    states: HashMap<FilterId, FilterDspState>,
}

impl AudioFilterChainState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn process_interleaved_stereo(
        &mut self,
        filters: &[AudioFilterInstance],
        samples: &mut [f32],
        sample_rate: u32,
    ) {
        if samples.len() < 2 || filters.is_empty() {
            return;
        }
        let sr = sample_rate.max(8000) as f32;

        for filter in filters {
            if !filter.enabled {
                continue;
            }
            match &filter.kind {
                AudioFilterKind::Gain { gain_db } => {
                    let mul = db_to_linear(*gain_db);
                    for s in samples.iter_mut() {
                        *s *= mul;
                    }
                }
                AudioFilterKind::InvertPolarity => {
                    for s in samples.iter_mut() {
                        *s = -*s;
                    }
                }
                AudioFilterKind::NoiseGate {
                    close_threshold_db,
                    open_threshold_db,
                    attack_ms,
                    hold_ms,
                    release_ms,
                } => {
                    let entry = self
                        .states
                        .entry(filter.id)
                        .or_insert_with(|| FilterDspState::Gate(GateRuntime::default()));
                    if let FilterDspState::Gate(rt) = entry {
                        process_noise_gate(
                            rt,
                            samples,
                            sr,
                            *close_threshold_db,
                            *open_threshold_db,
                            *attack_ms,
                            *hold_ms,
                            *release_ms,
                        );
                    }
                }
                AudioFilterKind::Compressor {
                    ratio,
                    threshold_db,
                    attack_ms,
                    release_ms,
                    output_gain_db,
                } => {
                    let entry = self
                        .states
                        .entry(filter.id)
                        .or_insert_with(|| FilterDspState::Dynamics(DynamicsRuntime::default()));
                    if let FilterDspState::Dynamics(rt) = entry {
                        process_compressor(
                            rt,
                            samples,
                            sr,
                            *ratio,
                            *threshold_db,
                            *attack_ms,
                            *release_ms,
                            *output_gain_db,
                        );
                    }
                }
                AudioFilterKind::Limiter {
                    threshold_db,
                    release_ms,
                } => {
                    let entry = self
                        .states
                        .entry(filter.id)
                        .or_insert_with(|| FilterDspState::Dynamics(DynamicsRuntime::default()));
                    if let FilterDspState::Dynamics(rt) = entry {
                        process_limiter(rt, samples, sr, *threshold_db, *release_ms);
                    }
                }
                AudioFilterKind::Expander {
                    ratio,
                    threshold_db,
                    attack_ms,
                    release_ms,
                    output_gain_db,
                    ..
                } => {
                    let entry = self
                        .states
                        .entry(filter.id)
                        .or_insert_with(|| FilterDspState::Dynamics(DynamicsRuntime::default()));
                    if let FilterDspState::Dynamics(rt) = entry {
                        process_expander(
                            rt,
                            samples,
                            sr,
                            *ratio,
                            *threshold_db,
                            *attack_ms,
                            *release_ms,
                            *output_gain_db,
                        );
                    }
                }
                AudioFilterKind::NoiseSuppression {
                    method,
                    suppression_level_db,
                } => {
                    let entry = self.states.entry(filter.id).or_insert_with(|| {
                        FilterDspState::NoiseSuppress(NoiseSuppressRuntime::default())
                    });
                    if let FilterDspState::NoiseSuppress(rt) = entry {
                        process_noise_suppression(rt, samples, sr, *method, *suppression_level_db);
                    }
                }
                AudioFilterKind::ThreeBandEq {
                    low_db,
                    mid_db,
                    high_db,
                } => {
                    let entry = self
                        .states
                        .entry(filter.id)
                        .or_insert_with(|| FilterDspState::Eq(EqRuntime::default()));
                    if let FilterDspState::Eq(rt) = entry {
                        process_three_band_eq(rt, samples, sr, *low_db, *mid_db, *high_db);
                    }
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn process_noise_gate(
    rt: &mut GateRuntime,
    samples: &mut [f32],
    sr: f32,
    close_db: f32,
    open_db: f32,
    attack_ms: f32,
    hold_ms: f32,
    release_ms: f32,
) {
    let open_lin = db_to_linear(open_db.max(close_db));
    let close_lin = db_to_linear(close_db.min(open_db));
    let hold_target = ((hold_ms.max(0.0) * 0.001) * sr) as usize;
    let attack_alpha = 1.0 - time_coeff(attack_ms, sr);
    let release_alpha = 1.0 - time_coeff(release_ms, sr);

    for frame in samples.as_chunks_mut::<2>().0 {
        let level = frame[0].abs().max(frame[1].abs());
        if level >= open_lin {
            rt.is_open = true;
            rt.hold_samples = hold_target;
        } else if rt.is_open {
            if rt.hold_samples > 0 {
                rt.hold_samples -= 1;
            } else if level < close_lin {
                rt.is_open = false;
            }
        }

        let target = if rt.is_open { 1.0 } else { 0.0 };
        let alpha = if target > rt.gain {
            attack_alpha
        } else {
            release_alpha
        };
        rt.gain += (target - rt.gain) * alpha;
        frame[0] *= rt.gain;
        frame[1] *= rt.gain;
    }
}

#[allow(clippy::too_many_arguments)]
fn process_compressor(
    rt: &mut DynamicsRuntime,
    samples: &mut [f32],
    sr: f32,
    ratio: f32,
    threshold_db: f32,
    attack_ms: f32,
    release_ms: f32,
    output_gain_db: f32,
) {
    let ratio = ratio.max(1.0);
    let attack_c = time_coeff(attack_ms, sr);
    let release_c = time_coeff(release_ms, sr);
    let makeup = db_to_linear(output_gain_db);

    for frame in samples.as_chunks_mut::<2>().0 {
        let peak = frame[0].abs().max(frame[1].abs());
        let c = if peak > rt.envelope {
            attack_c
        } else {
            release_c
        };
        rt.envelope = c * rt.envelope + (1.0 - c) * peak;

        let env_db = linear_to_db(rt.envelope);
        let target_gr_db = if env_db > threshold_db {
            let over = env_db - threshold_db;
            -(over * (1.0 - 1.0 / ratio))
        } else {
            0.0
        };

        let smooth_c = if target_gr_db < rt.gain_db {
            attack_c
        } else {
            release_c
        };
        rt.gain_db = smooth_c * rt.gain_db + (1.0 - smooth_c) * target_gr_db;
        let g = db_to_linear(rt.gain_db) * makeup;
        frame[0] *= g;
        frame[1] *= g;
    }
}

fn process_limiter(
    rt: &mut DynamicsRuntime,
    samples: &mut [f32],
    sr: f32,
    threshold_db: f32,
    release_ms: f32,
) {
    let thresh_lin = db_to_linear(threshold_db).max(1e-5);
    let release_c = time_coeff(release_ms, sr);

    for frame in samples.as_chunks_mut::<2>().0 {
        let peak = frame[0].abs().max(frame[1].abs());
        if peak > rt.envelope {
            // Near-instantaneous peak attack for brickwall limiting
            rt.envelope = peak;
        } else {
            rt.envelope = release_c * rt.envelope + (1.0 - release_c) * peak;
        }

        let gain = if rt.envelope > thresh_lin {
            thresh_lin / rt.envelope
        } else {
            1.0
        };
        frame[0] = (frame[0] * gain).clamp(-thresh_lin, thresh_lin);
        frame[1] = (frame[1] * gain).clamp(-thresh_lin, thresh_lin);
    }
}

#[allow(clippy::too_many_arguments)]
fn process_expander(
    rt: &mut DynamicsRuntime,
    samples: &mut [f32],
    sr: f32,
    ratio: f32,
    threshold_db: f32,
    attack_ms: f32,
    release_ms: f32,
    output_gain_db: f32,
) {
    let ratio = ratio.max(1.0);
    let attack_c = time_coeff(attack_ms, sr);
    let release_c = time_coeff(release_ms, sr);
    let makeup = db_to_linear(output_gain_db);

    for frame in samples.as_chunks_mut::<2>().0 {
        let peak = frame[0].abs().max(frame[1].abs());
        let c = if peak > rt.envelope {
            attack_c
        } else {
            release_c
        };
        rt.envelope = c * rt.envelope + (1.0 - c) * peak;

        let env_db = linear_to_db(rt.envelope);
        let target_gr_db = if env_db < threshold_db {
            let under = threshold_db - env_db;
            -(under * (ratio - 1.0)).min(60.0)
        } else {
            0.0
        };

        let smooth_c = if target_gr_db > rt.gain_db {
            attack_c
        } else {
            release_c
        };
        rt.gain_db = smooth_c * rt.gain_db + (1.0 - smooth_c) * target_gr_db;
        let g = db_to_linear(rt.gain_db) * makeup;
        frame[0] *= g;
        frame[1] *= g;
    }
}

fn process_noise_suppression(
    rt: &mut NoiseSuppressRuntime,
    samples: &mut [f32],
    sr: f32,
    method: NoiseSuppressionMethod,
    suppression_level_db: f32,
) {
    // 80 Hz DC/rumble high-pass filter first (removes fan/desk rumble cleanly)
    let rc = 1.0 / (2.0 * PI * 80.0);
    let dt = 1.0 / sr;
    let hp_alpha = rc / (rc + dt);

    let min_gain = db_to_linear(suppression_level_db.clamp(-60.0, 0.0));
    let fast_c = time_coeff(8.0, sr);
    let release_c = time_coeff(120.0, sr);
    let floor_rise = time_coeff(1800.0, sr);
    let floor_fall = time_coeff(40.0, sr);

    for frame in samples.as_chunks_mut::<2>().0 {
        let in_l = frame[0];
        let in_r = frame[1];
        let out_l = hp_alpha * (rt.hp_prev_out_l + in_l - rt.hp_prev_in_l);
        let out_r = hp_alpha * (rt.hp_prev_out_r + in_r - rt.hp_prev_in_r);
        rt.hp_prev_in_l = in_l;
        rt.hp_prev_out_l = out_l;
        rt.hp_prev_in_r = in_r;
        rt.hp_prev_out_r = out_r;

        let energy = out_l.abs().max(out_r.abs());
        rt.speech_env = if energy > rt.speech_env {
            fast_c * rt.speech_env + (1.0 - fast_c) * energy
        } else {
            release_c * rt.speech_env + (1.0 - release_c) * energy
        };

        // Minimum-statistics stationary noise floor estimator
        if rt.speech_env < rt.noise_floor {
            rt.noise_floor = floor_fall * rt.noise_floor + (1.0 - floor_fall) * rt.speech_env;
        } else {
            rt.noise_floor = floor_rise * rt.noise_floor + (1.0 - floor_rise) * rt.speech_env;
        }
        let noise_est = rt.noise_floor.max(1e-5);
        let snr = rt.speech_env / noise_est;

        let target_gain = match method {
            NoiseSuppressionMethod::AdaptiveSpectral => {
                // Smooth Wiener-like broadband attenuation curve
                let ratio = ((snr - 1.5) / 4.5).clamp(0.0, 1.0);
                min_gain + (1.0 - min_gain) * ratio * ratio
            }
            NoiseSuppressionMethod::FastNoiseGate => {
                if snr > 2.2 {
                    1.0
                } else {
                    min_gain
                }
            }
        };

        let g_c = if target_gain > rt.gain {
            fast_c
        } else {
            release_c
        };
        rt.gain = g_c * rt.gain + (1.0 - g_c) * target_gain;
        frame[0] = out_l * rt.gain;
        frame[1] = out_r * rt.gain;
    }
}

fn peaking_coeffs(freq: f32, q: f32, gain_db: f32, sr: f32) -> (f32, f32, f32, f32, f32) {
    let a = 10.0_f32.powf(gain_db / 40.0);
    let w0 = 2.0 * PI * (freq / sr).clamp(0.001, 0.45);
    let alpha = w0.sin() / (2.0 * q);
    let cos_w0 = w0.cos();

    let b0 = 1.0 + alpha * a;
    let b1 = -2.0 * cos_w0;
    let b2 = 1.0 - alpha * a;
    let a0 = 1.0 + alpha / a;
    let a1 = -2.0 * cos_w0;
    let a2 = 1.0 - alpha / a;
    (b0 / a0, b1 / a0, b2 / a0, a1 / a0, a2 / a0)
}

fn process_three_band_eq(
    rt: &mut EqRuntime,
    samples: &mut [f32],
    sr: f32,
    low_db: f32,
    mid_db: f32,
    high_db: f32,
) {
    let low = peaking_coeffs(120.0, 0.7, low_db.clamp(-24.0, 24.0), sr);
    let mid = peaking_coeffs(1000.0, 1.0, mid_db.clamp(-24.0, 24.0), sr);
    let high = peaking_coeffs(6000.0, 0.7, high_db.clamp(-24.0, 24.0), sr);

    for frame in samples.as_chunks_mut::<2>().0 {
        let mut l = frame[0];
        let mut r = frame[1];
        if low_db.abs() > 0.01 {
            l = rt.low_l.process(l, low.0, low.1, low.2, low.3, low.4);
            r = rt.low_r.process(r, low.0, low.1, low.2, low.3, low.4);
        }
        if mid_db.abs() > 0.01 {
            l = rt.mid_l.process(l, mid.0, mid.1, mid.2, mid.3, mid.4);
            r = rt.mid_r.process(r, mid.0, mid.1, mid.2, mid.3, mid.4);
        }
        if high_db.abs() > 0.01 {
            l = rt.high_l.process(l, high.0, high.1, high.2, high.3, high.4);
            r = rt.high_r.process(r, high.0, high.1, high.2, high.3, high.4);
        }
        frame[0] = l;
        frame[1] = r;
    }
}
