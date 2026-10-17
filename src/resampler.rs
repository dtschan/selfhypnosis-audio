use color_eyre::eyre::Report;
use rubato::{
    Fft, FixedSync, Resampler as RubatoResampler,
    audioadapter::Adapter,
    audioadapter_buffers::{direct::InterleavedSlice, owned::InterleavedOwned},
};

/// Resamples 16-bit PCM audio from 24000 Hz to 44100 Hz using an FFT-based
/// synchronous resampler.  The ratio is fixed at construction time, which is
/// appropriate for offline (disk-to-disk) processing.
pub struct Resampler {
    resampler: Fft<f32>,
}

impl Resampler {
    pub fn new(source_rate: usize, target_rate: usize) -> Result<Self, Report> {
        Ok(Self {
            resampler: Fft::<f32>::new(source_rate, target_rate, 4096, 1, FixedSync::Both)?,
        })
    }

    pub fn reset(&mut self) {
        self.resampler.reset();
    }

    /// Resample `input` (24000 Hz, mono, i16) into `output` (44100 Hz, mono, i16).
    ///
    /// `output` must be large enough to hold the resampled data.  The required
    /// length is at least `ceil(input.len() * 44100 / 24000)` samples.
    pub fn resample(&mut self, input: &[f32]) -> Result<Vec<f32>, Report> {
        // Convert i16 → f32 into a channel-separated buffer (1 channel).
        // let input_f32: Vec<f32> = input.iter().map(|&s| s as f32 / 32768.0).collect();
        // let input_frames = input_f32.len();

        // let mut resampler = Fft::<f32>::new(SOURCE_RATE, TARGET_RATE, 4096, 1, 1, FixedSync::Both)
        //     .expect("failed to create rubato resampler");

        let output_len = self.resampler.process_all_needed_output_len(input.len());
        let input = InterleavedSlice::new(input, 1, input.len())?;
        let mut output = InterleavedOwned::new(0.0, 1, output_len);
        // let in_channels = vec![input_f32];
        // let mut out_channels = vec![vec![0.0f32; output_len]];

        // let in_buf = SequentialSliceOfVecs::new(&in_channels, 1, input_frames)
        //     .expect("input buffer creation failed");
        // let mut out_buf = SequentialSliceOfVecs::new_mut(&mut out_channels, 1, output_len)
        //     .expect("output buffer creation failed");

        let (_in_frames, out_frames) =
            self.resampler
                .process_all_into_buffer(&input, &mut output, input.frames(), None)?;

        // Convert f32 → i16 and write into caller-supplied slice.
        // let resampled = &out_channels[0][..out_frames.min(output.len())];
        // for (dst, &src) in output.iter_mut().zip(resampled.iter()) {
        //     *dst = (src * 32768.0).round().clamp(-32768.0, 32767.0) as i16;
        // }
        let mut data = output.take_data();
        data.truncate(out_frames);
        Ok(data)
    }
}
