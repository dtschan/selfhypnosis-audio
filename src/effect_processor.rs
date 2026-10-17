use std::mem;

use crate::{manuscript::Effect, time_stretcher::TimeStretcher};

pub struct MonoEffectProcessor {
    stretcher: TimeStretcher,
    output_samples: Vec<f32>,
}

impl MonoEffectProcessor {
    pub fn new(stretcher: TimeStretcher) -> Self {
        Self {
            stretcher,
            output_samples: Vec::new(),
        }
    }

    pub fn process(&mut self, effects: &[Effect], input_samples: &mut Vec<f32>) {
        for effect in effects {
            match effect {
                Effect::Stretch { factor } => {
                    let out_len = (input_samples.len() as f32 * *factor).round() as usize;
                    self.output_samples.resize(out_len, 0.0);
                    self.stretcher.reset();
                    self.stretcher.process(input_samples, &mut self.output_samples);
                    mem::swap(&mut self.output_samples, input_samples);
                }
            }
        }
    }
}
