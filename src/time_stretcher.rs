use signalsmith_stretch::Stretch;

pub struct TimeStretcher {
    stretch: Stretch,
}

impl TimeStretcher {
    pub fn new(sample_rate: u32, channels: u32) -> Self {
        Self {
            stretch: Stretch::preset_default(channels, sample_rate),
        }
    }

    pub fn reset(&mut self) {
        self.stretch.reset();
    }

    pub fn process(&mut self, input_samples: &[f32], output_samples: &mut [f32]) {
        self.stretch.exact(&input_samples, output_samples);
    }
}
