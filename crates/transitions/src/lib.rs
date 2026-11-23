use obs_core::{SceneId, TransitionId};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum TransitionDirection {
    #[default]
    Left,
    Right,
    Up,
    Down,
}

impl TransitionDirection {
    pub fn display_name(self) -> &'static str {
        match self {
            Self::Left => "Left",
            Self::Right => "Right",
            Self::Up => "Up",
            Self::Down => "Down",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TransitionKind {
    Cut,
    Fade,
    Swipe {
        direction: TransitionDirection,
        swipe_in: bool,
    },
    Slide {
        direction: TransitionDirection,
    },
    FadeToColor {
        color_rgba: [u8; 4],
        /// 0.0..=1.0 midpoint where A switches to B (OBS default 0.5)
        switch_point: f32,
    },
    LumaWipe {
        wipe_name: String,
        softness: f32,
        invert: bool,
    },
    Stinger {
        video_path: String,
        transition_point_ms: u32,
    },
}

impl TransitionKind {
    pub fn display_type(&self) -> &'static str {
        match self {
            Self::Cut => "Cut",
            Self::Fade => "Fade",
            Self::Swipe { .. } => "Swipe",
            Self::Slide { .. } => "Slide",
            Self::FadeToColor { .. } => "Fade to Color",
            Self::LumaWipe { .. } => "Luma Wipe",
            Self::Stinger { .. } => "Stinger",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransitionConfig {
    pub id: TransitionId,
    pub name: String,
    pub kind: TransitionKind,
    pub duration_ms: u32,
}

impl TransitionConfig {
    pub fn default_presets() -> Vec<Self> {
        vec![
            Self {
                id: TransitionId::new(),
                name: "Cut".to_string(),
                kind: TransitionKind::Cut,
                duration_ms: 0,
            },
            Self {
                id: TransitionId::new(),
                name: "Fade".to_string(),
                kind: TransitionKind::Fade,
                duration_ms: 300,
            },
            Self {
                id: TransitionId::new(),
                name: "Swipe".to_string(),
                kind: TransitionKind::Swipe {
                    direction: TransitionDirection::Left,
                    swipe_in: true,
                },
                duration_ms: 350,
            },
            Self {
                id: TransitionId::new(),
                name: "Slide".to_string(),
                kind: TransitionKind::Slide {
                    direction: TransitionDirection::Right,
                },
                duration_ms: 350,
            },
            Self {
                id: TransitionId::new(),
                name: "Fade to Color".to_string(),
                kind: TransitionKind::FadeToColor {
                    color_rgba: [0, 0, 0, 255],
                    switch_point: 0.5,
                },
                duration_ms: 400,
            },
            Self {
                id: TransitionId::new(),
                name: "Luma Wipe".to_string(),
                kind: TransitionKind::LumaWipe {
                    wipe_name: "linear-h.png".to_string(),
                    softness: 0.15,
                    invert: false,
                },
                duration_ms: 450,
            },
        ]
    }
}

/// Active transition state driving compositor blending & Studio Mode T-Bar.
#[derive(Debug, Clone)]
pub struct ActiveTransition {
    pub config: TransitionConfig,
    pub from_scene: SceneId,
    pub to_scene: SceneId,
    pub elapsed_ms: f32,
    pub manual_t_bar: bool,
    pub progress: f32,
}

impl ActiveTransition {
    pub fn start_auto(config: TransitionConfig, from_scene: SceneId, to_scene: SceneId) -> Self {
        let immediate = matches!(config.kind, TransitionKind::Cut) || config.duration_ms == 0;
        Self {
            config,
            from_scene,
            to_scene,
            elapsed_ms: 0.0,
            manual_t_bar: false,
            progress: if immediate { 1.0 } else { 0.0 },
        }
    }

    pub fn set_t_bar(
        config: TransitionConfig,
        from_scene: SceneId,
        to_scene: SceneId,
        t: f32,
    ) -> Self {
        Self {
            config,
            from_scene,
            to_scene,
            elapsed_ms: 0.0,
            manual_t_bar: true,
            progress: t.clamp(0.0, 1.0),
        }
    }

    /// Advances an automatic transition by `dt_ms` and returns true when finished.
    pub fn advance(&mut self, dt_ms: f32) -> bool {
        if self.manual_t_bar {
            return self.progress >= 1.0;
        }
        if self.config.duration_ms == 0 || matches!(self.config.kind, TransitionKind::Cut) {
            self.progress = 1.0;
            return true;
        }
        self.elapsed_ms += dt_ms.max(0.0);
        self.progress = (self.elapsed_ms / (self.config.duration_ms as f32)).clamp(0.0, 1.0);
        self.progress >= 1.0
    }

    /// Smooth cubic ease-in-out progress for visual compositing.
    pub fn eased_progress(&self) -> f32 {
        let t = self.progress.clamp(0.0, 1.0);
        if t < 0.5 {
            4.0 * t * t * t
        } else {
            1.0 - (-2.0 * t + 2.0).powi(3) / 2.0
        }
    }
}
