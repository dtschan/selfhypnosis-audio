mod manuscript;
mod effect_processor;
mod audio;
mod ebur128;
mod flac;
mod qwen_tts;
mod resampler;
mod time_stretcher;

use std::env;
use std::fmt::Display;
use std::fs::File;
use std::path::PathBuf;

use glob::glob;
use color_eyre::eyre::Report;
use tracing::instrument;
use tracing_error::ErrorLayer;
use tracing_subscriber::{
    EnvFilter, filter::LevelFilter, layer::SubscriberExt, util::SubscriberInitExt,
};

use crate::audio::LoopAudioReader;
use crate::effect_processor::MonoEffectProcessor;
use crate::manuscript::Effect;
use crate::manuscript::Manuscript;
use crate::manuscript::Speech;
use crate::{
    audio::{AudioRead, AudioWriter},
    ebur128::Ebur128,
    flac::{FlacReader, FlacWriter},
    qwen_tts::QwenTTS,
    resampler::Resampler,
    time_stretcher::TimeStretcher,
};

/*pub struct GeneratedSpeech {
    // pub generation_id: String,
    pub duration: f64,
    pub wav_file: String,
}*/

#[instrument]
fn calculate_gains(music: &[PathBuf], target_loudness_db: f32) -> Result<Vec<f32>, Report> {
    let ebur128 = ebur128::Ebur128::new(44100, 2);
    let mut gains = Vec::new();
    let mut buf = Vec::new();
    for path in music {
        let mut reader = FlacReader::new(path, 1.0)?;
        buf.clear();
        reader.read_to_end_f32(&mut buf)?;
        let report = ebur128.normalize(&mut buf, target_loudness_db, -1.0)?;
        let linear_gain = 10f32.powf(report.applied_gain_db as f32 / 20.0);
        gains.push(linear_gain);
    }
    Ok(gains)
}

struct HypnosisManuscriptGenerator {
    qwen_tts: QwenTTS,
    mono_effect_processor: MonoEffectProcessor,
    resampler: Resampler,
    ebur128: Ebur128,
    input_samples: Vec<f32>,
    music_samples: Vec<f32>,
    music: LoopAudioReader<FlacReader>,
    flac_file: AudioWriter<FlacWriter>,
}

impl HypnosisManuscriptGenerator {
    pub fn new(qwen_tts: QwenTTS, mono_effect_processor: MonoEffectProcessor, resampler: Resampler, ebur128: Ebur128, music: LoopAudioReader<FlacReader>, flac_file: AudioWriter<FlacWriter>) -> Self {
        Self {
            qwen_tts,
            mono_effect_processor,
            resampler,
            ebur128,
            input_samples: Vec::new(),
            music_samples: Vec::new(),
            music,
            flac_file,
        }
    }

    fn generate_speech(&mut self, text: impl Display, effects: &[Effect], music_gain: f32) -> Result<u32, Report> {
        let text = format!(" {} ", text);
        self.qwen_tts.generate_speech(&text, &mut self.input_samples)?;

        println!("input_sample len: {}", self.input_samples.len());
        self.mono_effect_processor.process(effects, &mut self.input_samples);
        println!("output_sample len 2: {}", self.input_samples.len());
        self.resampler.reset();
        self.input_samples = self.resampler.resample(&self.input_samples)?;
        // let dc = input_samples.iter().sum::<f32>() / input_samples.len() as f32;
        // println!("DC offset: {}", dc);

        println!("input_sample len 3: {}", self.input_samples.len());
        // Remove DC offset before normalization
        // for sample in input_samples.iter_mut() {
        //     *sample -= dc;
        // }

        // Normalize while still mono using short-term LUFS for better handling of short clips
        let lufs = self.ebur128.normalize_by_short_term(&mut self.input_samples, -20.0, -1.0)?;
        println!("lufs (short-term): {:?}", lufs);

        println!("input_sample len 4: {}", self.input_samples.len());
        mono_to_stereo(&mut self.input_samples);

        //println!("input_sample len 5: {}", self.input_samples.len());
        //self.stereo_effect_processor.process(effects, &mut self.input_samples);

        //
        //
        self.music_samples.resize(self.input_samples.len(), 0.0);
        self.music.read_exact_f32(&mut self.music_samples)?;
        for (i, sample) in self.input_samples.iter_mut().enumerate() {
            *sample += self.music_samples[i] * music_gain;
        }
        println!("input_sample len 6: {}", self.input_samples.len());
        self.flac_file.write_f32_dither(&self.input_samples)?;

        Ok((self.input_samples.len() as f32 / 88.2) as u32)
    }

    fn generate_pause(&mut self, duration: u32) -> Result<(), Report> {
        println!("Writing silence of {}ms", duration);
        self.music_samples.resize(duration as usize * 44100 * 2 / 1000, 0.0);
        self.music.read_exact_f32(&mut self.music_samples)?;
        self.flac_file.write_f32_dither(&self.music_samples)?;

        Ok(())
    }

    /* /// Generate speech for a single beat in a breath segment.
    ///
    /// The TTS output is trimmed of leading/trailing silence. If the trimmed
    /// speech is shorter than `min_speech_ms` it is gently stretched up to that
    /// floor (capped at 1.5x to avoid artifacts). Speech that is already at or
    /// above the floor keeps its natural duration. A pause fills the remainder
    /// of `beat_interval_ms` so onset-to-onset timing stays constant.
    fn generate_speech_for_beat(
        &mut self,
        text: impl Display,
        effects: &[Effect],
        music_gain: f32,
        min_speech_ms: u32,
        beat_interval_ms: u32,
    ) -> Result<(), Report> {
        let text = format!(" {} ", text);
        self.qwen_tts.generate_speech(&text, &mut self.input_samples)?;

        // Trim leading/trailing silence from the raw TTS output (mono, 24 kHz).
        // Use a small fade (48 samples ≈ 2 ms at 24 kHz) to avoid clicks.
        trim_silence(&mut self.input_samples, 0.01, 48);

        let trimmed_len = self.input_samples.len();
        let trimmed_ms = (trimmed_len as f64 / 24.0) as u32; // 24 samples/ms at 24 kHz

        // Only stretch if the natural speech is shorter than the minimum.
        // Cap the stretch ratio at 1.5x to keep the audio sounding natural.
        const MAX_STRETCH_RATIO: f32 = 1.5;
        let effective_target_ms = if trimmed_ms < min_speech_ms {
            let desired_ratio = min_speech_ms as f32 / trimmed_ms.max(1) as f32;
            let clamped_ratio = desired_ratio.min(MAX_STRETCH_RATIO);
            (trimmed_ms as f32 * clamped_ratio) as u32
        } else {
            trimmed_ms // keep natural duration
        };

        println!(
            "Breath beat: text={}, trimmed={}ms, effective={}ms, beat={}ms",
            text.trim(),
            trimmed_ms,
            effective_target_ms,
            beat_interval_ms,
        );

        // Time-stretch only when the effective target differs from the
        // trimmed duration (i.e. short utterances that need slowing down).
        if effective_target_ms != trimmed_ms && trimmed_len > 0 {
            let target_samples = (effective_target_ms as usize * 24000) / 1000;
            self.stretched_samples.resize(target_samples, 0.0);
            self.pacing_stretcher.reset();
            self.pacing_stretcher
                .process(&self.input_samples, &mut self.stretched_samples);
            std::mem::swap(&mut self.input_samples, &mut self.stretched_samples);
        }

        // Now apply the normal effect pipeline (effects, resample, normalize,
        // stereo, stereo effects, mix with music, write).
        self.mono_effect_processor
            .process(effects, &mut self.input_samples);
        self.resampler.reset();
        self.input_samples = self.resampler.resample(&self.input_samples)?;

        let lufs = self
            .ebur128
            .normalize_by_short_term(&mut self.input_samples, -20.0, -1.0)?;
        println!("lufs (short-term): {:?}", lufs);

        mono_to_stereo(&mut self.input_samples);
        self.stereo_effect_processor
            .process(effects, &mut self.input_samples);

        // Mix with music
        self.music_samples.resize(self.input_samples.len(), 0.0);
        self.music.read_exact_f32(&mut self.music_samples)?;
        for (i, sample) in self.input_samples.iter_mut().enumerate() {
            *sample += self.music_samples[i] * music_gain;
        }
        self.flac_file.write_f32_dither(&self.input_samples)?;

        // Append silence to fill the rest of the beat interval
        let speech_output_ms =
            (self.input_samples.len() as f32 / 88.2) as u32; // stereo 44.1kHz
        let remaining_ms = beat_interval_ms.saturating_sub(speech_output_ms);
        if remaining_ms > 0 {
            self.generate_pause(remaining_ms)?;
        }

        Ok(())
    } */

}
fn main() -> Result<(), Report> {
    tracing_subscriber::registry()
        .with(
            EnvFilter::builder()
                .with_default_directive(LevelFilter::INFO.into())
                .from_env_lossy(),
        )
        .with(ErrorLayer::default())
        .with(tracing_subscriber::fmt::layer())
        .init();

    color_eyre::install()?;

    let manuscript_file = env::args().nth(1).unwrap_or(String::from("manuscript.yaml"));
    let manuscript: Manuscript = serde_yaml_ng::from_reader(File::open(&manuscript_file)?)?;
    println!("Rendering manuscript {}", manuscript.title);

    let mut music_paths = Vec::new();
    for pattern in &manuscript.music.files {
        for entry in glob(pattern)? {
            music_paths.push(entry?);
        }
    }
    // Album order comes from the embedded TRACKNUMBER tag; untagged files
    // sort last, keeping alphabetical order (stable sort).
    // music_paths.sort_by_key(|path| flac::track_number(path).unwrap_or(u32::MAX));

    println!("{:#?}", music_paths);

    let music_gains = calculate_gains(&music_paths, manuscript.music.no_voice_loudness_db)?;

    // let history = build_history(&config, &manuscript)?;

    // let spec = WavSpec {
    //     channels: 1,
    //     sample_rate: 24000,
    //     bits_per_sample: 16,
    //     sample_format: SampleFormat::Int,
    // };
    // let mut writer = WavWriter::create("test.wav", spec).expect("Failed to create output");

    let qwen_tts = QwenTTS::new("german", &manuscript.voice.speaker, &manuscript.voice.options)?;

    let music_readers = music_paths.iter().zip(music_gains).map(|(path, gain)| FlacReader::new(path, gain)).collect::<Result<Vec<_>, _>>()?;
    let music = LoopAudioReader::new(music_readers)?;

    // let mut reader: Box<dyn Read> = Box::new(FlacReader::new(&manuscript.music[0])?);
    // for path in &manuscript.music[1..] {
    //     reader = Box::new(reader.chain(FlacReader::new(path)?));
    // }

    // let mut music = {
    //     let mut reader: Box<dyn Read> = if manuscript.music.is_empty() {
    //         Box::new(std::io::repeat(0))
    //     } else {
    //         let mut reader: Box<dyn Read> = Box::new(FlacReader::new(&manuscript.music[0])?);
    //         for path in &manuscript.music[1..] {
    //             reader = Box::new(reader.chain(FlacReader::new(path)?));
    //         }
    //         reader
    //     };
    //     AudioReader::new(reader)
    // };
    //let mut music_samples = Vec::new();
    let resampler = Resampler::new(24000, 44100)?;
    let flac_file = AudioWriter::new(FlacWriter::new("test.flac", 44100, 16, 2)?);
    let stretcher = TimeStretcher::new(24000, 1);
    let ebur128 = Ebur128::new(44100, 1);
    let mono_effect_processor = MonoEffectProcessor::new(stretcher);
    //let mut input_samples = Vec::new();
    //let mut output_samples = Vec::new();
    let mut hypnosis_manuscript_generator = HypnosisManuscriptGenerator::new(qwen_tts, mono_effect_processor, resampler, ebur128, music, flac_file);
    //let pause_detector = PauseDetector::new(22400);
    // let mut highpass_filter = highpass_hz(60.0, 0.707);
    //let voicebox = Voicebox::new("http://localhost:17493")?;
    for segment in manuscript.segments {
        // let generated_speech = history
        //     .get(&segment.text)
        //     .unwrap_or_else(|| panic!("No generated speech found for segment: {}", segment.text));

        // Generate speech from prev segment, current segment and next segment, if available:

        // let prev_text = if segment_index > 0 {
        //     format!("{} ", &manuscript.segments[segment_index - 1].text)
        // } else {
        //     String::new()
        // };
        // let next_text = if segment_index < manuscript.segments.len() - 1 {
        //     format!(" {}", &manuscript.segments[segment_index + 1].text)
        // } else {
        //     String::new()
        // };
        // let text = format!("{prev_text}{}{next_text}", segment.text);
        // println!("Generating speech for segment {}: {}", segment_index, text);
        match segment.text {
            Speech::Text(text) => {
                hypnosis_manuscript_generator.generate_speech(&text, &segment.effects, manuscript.music.with_voice_gain())?;
                hypnosis_manuscript_generator.generate_pause(segment.pause)?;
            },
            /* Speech::Breath { inhale_word, inhale_duration, exhale_word, exhale_duration, total_duration } => {
                println!("Breath");
                // Beat interval: each utterance (word or countdown number)
                // occupies one 2-second beat.
                const BEAT_INTERVAL_MS: u32 = 2000;

                // Minimum speech durations within a beat.  Utterances that
                // are naturally shorter than this floor get gently stretched
                // (up to 1.5x) so they don't sound clipped.  Utterances
                // that are already longer (e.g. "zwei") keep their natural
                // duration — only the trailing silence shrinks.
                const WORD_MIN_MS: u32 = 800;
                const NUMBER_MIN_MS: u32 = 400;

                let music_gain = manuscript.music.with_voice_gain();
                let cycle_duration = inhale_duration + exhale_duration;

                for _ in 0..total_duration.div_ceil(cycle_duration) {
                    // --- Inhale phase ---
                    // First beat: the inhale word (e.g. "Einatmen")
                    hypnosis_manuscript_generator.generate_speech_for_beat(
                        &inhale_word, &segment.effects, music_gain,
                        WORD_MIN_MS, BEAT_INTERVAL_MS,
                    )?;
                    // Remaining beats: countdown numbers
                    for i in (1..inhale_duration.div_ceil(BEAT_INTERVAL_MS)).rev() {
                        hypnosis_manuscript_generator.generate_speech_for_beat(
                            i, &segment.effects, music_gain,
                            NUMBER_MIN_MS, BEAT_INTERVAL_MS,
                        )?;
                    }

                    // --- Exhale phase ---
                    // First beat: the exhale word (e.g. "Ausatmen")
                    hypnosis_manuscript_generator.generate_speech_for_beat(
                        &exhale_word, &segment.effects, music_gain,
                        WORD_MIN_MS, BEAT_INTERVAL_MS,
                    )?;
                    // Remaining beats: countdown numbers
                    for i in (1..exhale_duration.div_ceil(BEAT_INTERVAL_MS)).rev() {
                        hypnosis_manuscript_generator.generate_speech_for_beat(
                            i, &segment.effects, music_gain,
                            NUMBER_MIN_MS, BEAT_INTERVAL_MS,
                        )?;
                    }
                }/
            } */
        };

        // let splitted = pause_detector.split(&input_samples);
        // println!("Split into {} chunks", splitted.len());
        // let expected_chunks = if segment_index == 0 {
        //     2
        // } else if segment_index == manuscript.segments.len() - 1 {
        //     2
        // } else {
        //     3
        // };
        // let voice_samples = if splitted.len() == expected_chunks {
        //     splitted[1];
        // } else {
        //     return Err(eyre!("Expected {} chunks, got {}", expected_chunks, splitted.len()));
        // };

        // let mut output_samples = vec![0i16; input_samples.len()];
        /*for (i, sample) in input_samples.iter().enumerate() {
            // Scale to i16 range with proper offset
            let scaled = sample * 32768.0;

            // Add TPDF dither before quantization (reduces distortion)
            let dithered = scaled; //  + tpdf_dither();

            // Round to nearest integer and clamp to i16 range
            output_samples[i] = dithered.round().clamp(-32768.0, 32767.0) as i16;
            //writer.write_sample(s).unwrap();
        }
        flac_file.write(&output_samples)?;*/
        //enc.send_frame(Frame::Audio(output_samples));

        /*output_samples.resize(input_samples.len(), 0.0);
        mono_effect_processor.process(&segment.effects, &mut input_samples, &mut output_samples);
        resampler.reset();
        input_samples = resampler.resample(&input_samples)?;
        // let dc = input_samples.iter().sum::<f32>() / input_samples.len() as f32;
        // println!("DC offset: {}", dc);

        // Remove DC offset before normalization
        // for sample in input_samples.iter_mut() {
        //     *sample -= dc;
        // }

        // Normalize while still mono using short-term LUFS for better handling of short clips
        let ebur128 = Ebur128::new(44100, 1);
        let lufs = ebur128.normalize_by_short_term(&mut input_samples, -20.0, -1.0)?;
        println!("lufs (short-term): {:?}", lufs);

        mono_to_stereo(&mut input_samples);

        stereo_effect_processor.process(&segment.effects, &mut input_samples, &mut output_samples);*/

        //
        //
        /* music_samples.resize(input_samples.len(), 0.0);
        music.read_exact_f32(&mut music_samples)?;
        let music_gain = manuscript.music.with_voice_gain();
        for (i, sample) in input_samples.iter_mut().enumerate() {
            *sample += music_samples[i] * music_gain;
        }
        flac_file.write_f32_dither(&input_samples)?;
        // Write silence after each segment
        println!("Writing silence of {}ms", segment.pause);
        // let silence = vec![0i16; segment.pause * 44100 * 2 as usize / 1000];
        // flac_file.write_i16(&silence)?;
        // music_samples.resize(segment.pause * 44100 * 2 / 1000 * 2, 0.0);
        music_samples.resize(segment.pause * 44100 * 2 / 1000, 0.0);
        music.read_exact_f32(&mut music_samples)?;
        //for i in 0..music_samples.len() {
        //    music_samples[i] *= 0.4;
        //}
        flac_file.write_f32_dither(&music_samples)?;*/
    }
    //flac_file.flush()?;
    //enc.flush();

    //writer.finalize().expect("Failed to finalize");
    /*let result = list_stories_stories_get(&config).unwrap();
    let stories = result
        .into_iter()
        .map(|story| (story.name.clone(), story.id))
        .collect::<HashMap<String, String>>();

    if stories.contains_key(&manuscript.title) {
        let story_id = stories.get(&manuscript.title).unwrap();
        delete_story_stories_story_id_delete(&config, story_id).unwrap();
        println!("Deleted existing story: {}", manuscript.title);
    }

    let story_create = StoryCreate {
        name: manuscript.title,
        ..Default::default()
    };
    let result = create_story_stories_post(&config, story_create).unwrap();
    let story_id = result.id;
    println!("Created story: {}", story_id);

    let mut start_time = 0.0;
    for segment in &manuscript.segments {
        if let Some(generated_speech) = history.get(&segment.text) {
            println!(
                "Adding segment to story: {} {} {} @ {}",
                segment.text, story_id, generated_speech.generation_id, start_time
            );
            let story_item_create = StoryItemCreate {
                generation_id: generated_speech.generation_id.clone(),
                start_time_ms: Some(Some((start_time * 1000.0) as i32)),
                ..Default::default()
            };
            let result =
                add_story_item_stories_story_id_items_post(&config, &story_id, story_item_create)
                    .unwrap();
            println!("Added segment to story: {}", result.id);
            start_time += generated_speech.duration.unwrap_or(0.0) + segment.pause;
        }
    }*/

    Ok(())
}

fn mono_to_stereo(samples: &mut Vec<f32>) {
    let n = samples.len();
    samples.resize(n * 2, 0.0);
    for i in (0..n).rev() {
        let s = samples[i];
        samples[i * 2] = s;
        samples[i * 2 + 1] = s;
    }
}
