use obs_core::FilterId;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ExpanderPreset {
    #[default]
    Expander,
    Gate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum NoiseSuppressionMethod {
    #[default]
    AdaptiveSpectral,
    FastNoiseGate,
}

impl NoiseSuppressionMethod {
    pub fn display_name(self) -> &'static str {
        match self {
            Self::AdaptiveSpectral => "RNNoise-style (High Quality, Adaptive)",
            Self::FastNoiseGate => "Speex-style (Low CPU)",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioFilterInstance {
    pub id: FilterId,
    pub name: String,
    pub enabled: bool,
    pub kind: AudioFilterKind,
}

impl AudioFilterInstance {
    pub fn new(name: impl Into<String>, kind: AudioFilterKind) -> Self {
        Self {
            id: FilterId::new(),
            name: name.into(),
            enabled: true,
            kind,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AudioFilterKind {
    Gain {
        gain_db: f32,
    },
    NoiseGate {
        close_threshold_db: f32,
        open_threshold_db: f32,
        attack_ms: f32,
        hold_ms: f32,
        release_ms: f32,
    },
    Compressor {
        ratio: f32,
        threshold_db: f32,
        attack_ms: f32,
        release_ms: f32,
        output_gain_db: f32,
    },
    Limiter {
        threshold_db: f32,
        release_ms: f32,
    },
    Expander {
        preset: ExpanderPreset,
        ratio: f32,
        threshold_db: f32,
        attack_ms: f32,
        release_ms: f32,
        output_gain_db: f32,
    },
    NoiseSuppression {
        method: NoiseSuppressionMethod,
        suppression_level_db: f32,
    },
    ThreeBandEq {
        low_db: f32,
        mid_db: f32,
        high_db: f32,
    },
    InvertPolarity,
}

impl AudioFilterKind {
    pub fn kind_name(&self) -> &'static str {
        match self {
            Self::Gain { .. } => "Gain",
            Self::NoiseGate { .. } => "Noise Gate",
            Self::Compressor { .. } => "Compressor",
            Self::Limiter { .. } => "Limiter",
            Self::Expander { .. } => "Expander",
            Self::NoiseSuppression { .. } => "Noise Suppression",
            Self::ThreeBandEq { .. } => "3-Band Equalizer",
            Self::InvertPolarity => "Invert Polarity",
        }
    }

    pub fn default_gain() -> Self {
        Self::Gain { gain_db: 0.0 }
    }

    pub fn default_noise_gate() -> Self {
        Self::NoiseGate {
            close_threshold_db: -32.0,
            open_threshold_db: -26.0,
            attack_ms: 25.0,
            hold_ms: 200.0,
            release_ms: 150.0,
        }
    }

    pub fn default_compressor() -> Self {
        Self::Compressor {
            ratio: 10.0,
            threshold_db: -18.0,
            attack_ms: 6.0,
            release_ms: 60.0,
            output_gain_db: 0.0,
        }
    }

    pub fn default_limiter() -> Self {
        Self::Limiter {
            threshold_db: -6.0,
            release_ms: 60.0,
        }
    }

    pub fn default_expander() -> Self {
        Self::Expander {
            preset: ExpanderPreset::Expander,
            ratio: 2.0,
            threshold_db: -40.0,
            attack_ms: 10.0,
            release_ms: 50.0,
            output_gain_db: 0.0,
        }
    }

    pub fn default_noise_suppression() -> Self {
        Self::NoiseSuppression {
            method: NoiseSuppressionMethod::AdaptiveSpectral,
            suppression_level_db: -30.0,
        }
    }

    pub fn default_three_band_eq() -> Self {
        Self::ThreeBandEq {
            low_db: 0.0,
            mid_db: 0.0,
            high_db: 0.0,
        }
    }
}
