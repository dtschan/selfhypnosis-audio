use candle_core::Device;
use color_eyre::eyre::{Report, WrapErr, eyre};
use hf_hub::api::sync::Api;
use hf_hub::{Repo, RepoType};
use qwen_tts::io::voice_prompt::load_voice_prompt;
use qwen_tts::model::loader::LoaderConfig;
use qwen_tts::model::voice_clone::VoiceClonePromptItem;
use qwen_tts::model::{Model, loader::ModelLoader, options::VoiceCloneOptions};
use sha2::{Digest, Sha256};
use std::mem;
use std::str::FromStr;
use std::{fs::File, path::PathBuf};

use crate::audio::AudioWriter;
use crate::audio::AudioRead;
use crate::flac::{FlacReader, FlacWriter};
use crate::manuscript::VoiceOptions;

pub struct QwenTTS {
    language: String,
    model: Model,
    prompt: VoiceClonePromptItem,
    clone_options: VoiceCloneOptions,
}

impl QwenTTS {
    pub fn new(language: &str, voice_profile: &str, voice_options: &VoiceOptions) -> Result<Self, Report> {
        let api = Api::new().wrap_err("failed to create HuggingFace API")?;
        let repo = api.repo(Repo::new(
            "Qwen/Qwen3-TTS-12Hz-1.7B-Base".to_string(),
            RepoType::Model,
        ));

        let model_dir = repo
            .get("config.json")
            .wrap_err("failed to locate Qwen3-TTS model")?
            .parent()
            .ok_or_else(|| eyre!("failed to determine model directory"))?
            .to_path_buf();

        for filename in &[
            "tokenizer.json",
            "vocab.json",
            "merges.txt",
            "tokenizer_config.json",
            "generation_config.json",
            "preprocessor_config.json",
            "speech_tokenizer/config.json",
            "speech_tokenizer/model.safetensors",
        ] {
            match repo.get(filename) {
                Ok(_) => {}
                Err(_) => eprintln!("  (optional file {filename} not found, skipping)"),
            }
        }

        let model_loder = ModelLoader::from_local_dir(model_dir)?;
        let model = model_loder.load_tts_model(&Device::Cpu, &LoaderConfig::default())?;
        let prompt = load_voice_prompt(
            &PathBuf::from_str(&format!("{}.voice.json", voice_profile))?,
            model.device(),
            model.dtype(),
        )
        .map_err(|e| eyre!(e))?;

        Ok(Self {
            language: String::from(language),
            model,
            prompt,
            clone_options: voice_options.into(),
        })
    }

    pub fn generate_speech(&self, text: &str, buf: &mut Vec<f32>) -> Result<(), Report> {
        let file_name = fragment_file_name(text);

        if let Ok(_file) = File::open(&file_name) {
            let mut flac_reader = FlacReader::new(&file_name, 1.0).wrap_err("Failed to open FLAC file")?;
            let _sample_rate = flac_reader.sample_rate();
            flac_reader.read_to_end_f32(buf)?;
            return Ok(());
        }

        let clone_options = VoiceCloneOptions {
            // Enable stochastic sampling for more natural, varied prosody; greedy decoding sounds flat (default: true).
            do_sample: Some(false),
            // Controls randomness of speech: lower → monotone and stable, higher → expressive but may glitch (default: 0.9).
            temperature: Some(0.5),
            // Restricts token choices per step; lower values reduce mispronunciations at the cost of variety (default: 50).
            top_k: Some(50),
            // Nucleus sampling cutoff; lower values trim unlikely pronunciations for cleaner audio (default: 1.0).
            top_p: Some(1.0),
            // Discourages repeated phoneme patterns that cause looping/stuttering artifacts (default: 1.05).
            repetition_penalty: Some(1.05),
            // Enable sampling in the subtalker (code predictor) for richer timbre detail; greedy makes audio dull (default: true).
            subtalker_do_sample: Some(true),
            // Subtalker temperature: affects fine-grained audio texture; higher → breathier/noisier, lower → cleaner (default: 0.9).
            subtalker_temperature: Some(0.9),
            // Subtalker top-k: limits codebook diversity per step; lower → cleaner but more robotic timbre (default: 50).
            subtalker_top_k: Some(50),
            // Subtalker top-p: nucleus cutoff for codebook tokens; lower trims noisy/artifacty codes (default: 1.0).
            subtalker_top_p: Some(1.0),
            // Maximum audio length in tokens; each token ≈ 83 ms at 12 Hz; caps generation time (default: 2048).
            max_new_tokens: Some(2048),
            // Wait for full generation before returning; required for ICL cloning quality (default: true for ICL, false for x-vector-only).
            non_streaming_mode: Some(true),
        };

        let result = self.model.generate_voice_clone_from_text(
            text,
            &self.prompt,
            &self.language,
            Some(clone_options),
        )?;
        let audio_tensor = if result.audio.dims().len() == 2 {
            result
                .audio
                .squeeze(0)
                .wrap_err("Failed to squeeze batch dim")?
        } else {
            result.audio
        };
        let audio_data: Vec<f32> = audio_tensor.to_vec1()?;

        let mut flac_writer = AudioWriter::new(FlacWriter::new(&file_name, 24000, 16, 1)?);
        flac_writer.write_f32(&audio_data)?;
        flac_writer.flush()?;

        let _ = mem::replace(buf, audio_data);

        /*
        let profile_id = self
            .voice_profiles
            .get(voice_profile)
            .ok_or_else(|| eyre!("voice profile '{}' not found", voice_profile))?;

        let request = GenerationRequest {
            profile_id: profile_id.clone(),
            text: text.to_string(),
            language: Some(String::from("de")),
            engine: Some(Some(String::from("qwen"))),
            normalize: Some(true),
            ..Default::default()
        };
        //let result = generate_speech_generate_post(configuration, request.clone())?;
        // let text = request.text.clone();
        let mut result = stream_speech_generate_stream_post(&self.configuration, request)?;
        // Write to a temp file first; rename into place only on success so a
        // failed/partial download never gets served from the cache.
        let tmp_name = format!("{}.tmp", file_name);
        let mut file = File::create(&tmp_name)
            .wrap_err_with(|| eyre!("failed to create temp file '{}'", tmp_name))?;
        result.copy_to(&mut file)?;
        drop(file);
        std::fs::rename(&tmp_name, &file_name)?;
        // if recording_enabled() {
        //     let slug = slugify(&text);
        //     let filename = format!("generate_speech_{slug}.json");
        //     write_fixture(&filename, &request, &result);
        // }*/
        Ok(())
    }
}

// impl Into<VoiceCloneOptions> for &VoiceOptions {
//     fn into(self) -> VoiceCloneOptions {
//         VoiceCloneOptions {
//             do_sample: Some(self.talker_sampling.enabled),
//             temperature: Some(self.talker_sampling.temperature as f64),
//             top_k: Some(self.talker_sampling.top_k as usize),
//             top_p: Some(self.talker_sampling.top_p as f64),
//             repetition_penalty: Some(self.talker_repetition_penalty as f64),
//             subtalker_do_sample: Some(self.subtalker_sampling.enabled),
//             subtalker_temperature: Some(self.subtalker_sampling.temperature as f64),
//             subtalker_top_k: Some(self.subtalker_sampling.top_k as usize),
//             subtalker_top_p: Some(self.subtalker_sampling.top_p as f64),
//             max_new_tokens: Some(self.max_new_tokens as usize),
//             non_streaming_mode: Some(true),
//         }
//     }
// }

impl From<&VoiceOptions> for VoiceCloneOptions {
    fn from(value: &VoiceOptions) -> Self {
        VoiceCloneOptions {
            do_sample: Some(value.talker_sampling.enabled),
            temperature: Some(value.talker_sampling.temperature as f64),
            top_k: Some(value.talker_sampling.top_k as usize),
            top_p: Some(value.talker_sampling.top_p as f64),
            repetition_penalty: Some(value.talker_repetition_penalty as f64),
            subtalker_do_sample: Some(value.subtalker_sampling.enabled),
            subtalker_temperature: Some(value.subtalker_sampling.temperature as f64),
            subtalker_top_k: Some(value.subtalker_sampling.top_k as usize),
            subtalker_top_p: Some(value.subtalker_sampling.top_p as f64),
            max_new_tokens: Some(value.max_new_tokens as usize),
            non_streaming_mode: Some(true),
        }
    }
}

fn fragment_file_name(text: &str) -> String {
    const MAX_SLUG_LENGTH: usize = 96;

    let mut slug = String::with_capacity(MAX_SLUG_LENGTH);
    let mut previous_was_separator = false;

    for character in text.chars() {
        if slug.len() >= MAX_SLUG_LENGTH {
            break;
        }

        if character.is_ascii_alphanumeric() {
            slug.push(character.to_ascii_lowercase());
            previous_was_separator = false;
        } else if !slug.is_empty() && !previous_was_separator {
            slug.push('-');
            previous_was_separator = true;
        }
    }

    let slug = slug.trim_matches('-');
    let slug = if slug.is_empty() { "fragment" } else { slug };
    let digest = hex::encode(Sha256::digest(text));

    format!("generated/{slug}--{}.flac", &digest[..16])
}

#[cfg(test)]
mod tests {
    use super::fragment_file_name;

    #[test]
    fn fragment_file_name_is_readable_and_portable() {
        assert_eq!(
            fragment_file_name("Hello, world!"),
            "generated/hello-world--315f5bdb76d078c4.flac"
        );
    }

    #[test]
    fn fragment_file_name_distinguishes_equivalent_slugs() {
        let with_space = fragment_file_name("Hello world");
        let with_comma = fragment_file_name("Hello, world");

        assert!(with_space.starts_with("generated/hello-world--"));
        assert!(with_comma.starts_with("generated/hello-world--"));
        assert_ne!(with_space, with_comma);
    }

    #[test]
    fn fragment_file_name_uses_a_fallback_for_non_ascii_text() {
        assert!(fragment_file_name("こんにちは").starts_with("generated/fragment--"));
    }
}
