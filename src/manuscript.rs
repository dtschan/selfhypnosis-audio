use serde::Deserialize;

#[derive(Deserialize, Debug)]
pub struct Manuscript {
    pub title: String,
    pub voice: Voice,
    pub music: Music,
    pub segments: Vec<Segment>,
}

#[derive(Deserialize, Debug)]
pub struct Segment {
    pub text: Speech,
    pub pause: u32,
    #[serde(default)]
    pub effects: Vec<Effect>,
}

#[derive(Deserialize, Debug)]
#[serde(untagged)]
pub enum Speech {
    Text(String),
    // Breath {
    //     inhale_word: String,
    //     inhale_duration: u32,
    //     exhale_word: String,
    //     exhale_duration: u32,
    //     total_duration: u32,
    // },
}

#[derive(Deserialize, Debug)]
#[serde(tag = "name")]
pub enum Effect {
    Stretch {
        factor: f32,
    },
}

#[derive(Deserialize, Debug)]
pub struct Voice {
    pub speaker: String,
    #[serde(default)]
    pub options: VoiceOptions,
}

#[derive(Deserialize, Debug)]
#[serde(default)]
pub struct VoiceOptions {
    pub talker_sampling: TalkerSampling,
    pub talker_repetition_penalty: f32,
    pub subtalker_sampling: SubtalkerSampling,
    pub max_new_tokens: u32,
}

impl Default for VoiceOptions {
    fn default() -> Self {
        Self {
            talker_sampling: TalkerSampling::default(),
            talker_repetition_penalty: 1.05,
            subtalker_sampling: SubtalkerSampling::default(),
            max_new_tokens: 2048,
        }
    }
}

#[derive(Deserialize, Debug)]
#[serde(default)]
pub struct TalkerSampling {
    pub enabled: bool,
    pub temperature: f32,
    pub top_k: u32,
    pub top_p: f32,
}

impl Default for TalkerSampling {
    fn default() -> Self {
        Self {
            enabled: false,
            temperature: 0.9,
            top_k: 50,
            top_p: 1.0,
        }
    }
}

#[derive(Deserialize, Debug)]
#[serde(default)]
pub struct SubtalkerSampling {
    pub enabled: bool,
    pub temperature: f32,
    pub top_k: u32,
    pub top_p: f32,
}

impl Default for SubtalkerSampling {
    fn default() -> Self {
        Self {
            enabled: true,
            temperature: 0.9,
            top_k: 50,
            top_p: 1.0,
        }
    }
}

pub fn no_voice_loudness_db_default() -> f32 {
    -28.0
}

#[derive(Deserialize, Debug)]
pub struct Music {
    pub files: Vec<String>,
    #[serde(default = "no_voice_loudness_db_default")]
    pub no_voice_loudness_db: f32,
    pub with_voice_loudness_db: Option<f32>,
}

impl Music {
    pub fn with_voice_gain(&self) -> f32 {
        10f32.powf((self.with_voice_loudness_db.unwrap_or(self.no_voice_loudness_db) - self.no_voice_loudness_db) / 20.0)
    }
}
