use color_eyre::eyre::Report;
use ebur128_stream::{
    AnalyzerBuilder, Channel, Mode,
    normalize::{NormalizeReport, Normalizer},
};

pub struct Ebur128 {
    sample_rate: u32,
    channels: u32,
}

impl Ebur128 {
    pub fn new(sample_rate: u32, channels: u32) -> Self {
        Self {
            sample_rate,
            channels,
        }
    }

    pub fn normalize(
        &self,
        buf: &mut [f32],
        target_lufs: f32,
        true_peak_ceiling: f32,
    ) -> Result<NormalizeReport, Report> {
        let channels: &[Channel] = if self.channels == 1 {
            &[Channel::Center]
        } else {
            &[Channel::Left, Channel::Right]
        };
        let normalizer = Normalizer::new(self.sample_rate, channels)
            .target_lufs(target_lufs as f64)
            .true_peak_ceiling_dbtp(true_peak_ceiling as f64);
        let report = normalizer.normalize_in_place(buf)?;
        // return actual LUFS
        // applied_gain = target_lufs - measured_integrated_lufs
        // target_lufs = applied_gain + measured_integrated_lufs
        // Ok(report.measured_integrated_lufs.map(|lufs| (lufs + report.applied_gain_db) as f32))
        Ok(report)
    }

    /// Normalize using short-term LUFS instead of integrated LUFS.
    /// This provides better results for short audio clips with low loudness range.
    pub fn normalize_by_short_term(
        &self,
        buf: &mut [f32],
        target_lufs: f64,
        true_peak_ceiling: f64,
    ) -> Result<NormalizeReport, Report> {
        let channels: &[Channel] = if self.channels == 1 {
            &[Channel::Center]
        } else {
            &[Channel::Left, Channel::Right]
        };

        // 1. Analyze with short-term mode enabled
        let mut analyzer = AnalyzerBuilder::new()
            .sample_rate(self.sample_rate)
            .channels(channels)
            .modes(Mode::ShortTerm | Mode::TruePeak)
            .build()?;

        analyzer.push_interleaved(buf)?;
        let report = analyzer.finalize();

        // 2. Use short_term_max_lufs() for gain calculation
        if let Some(measured_lufs) = report.short_term_max_lufs() {
            let delta_db = target_lufs - measured_lufs;
            let gain = 10.0f64.powf(delta_db / 20.0);

            // 3. Apply gain
            for sample in buf.iter_mut() {
                *sample *= gain as f32;
            }

            Ok(NormalizeReport {
                measured_integrated_lufs: report.integrated_lufs(),
                measured_true_peak_dbtp: report.true_peak_dbtp(),
                target_lufs,
                true_peak_ceiling_dbtp: Some(true_peak_ceiling as f64),
                applied_gain_db: delta_db,
                limited_by_true_peak: false,
            })
        } else {
            // No short-term measurement available (very short clip), fall back to integrated
            self.normalize(buf, target_lufs as f32, true_peak_ceiling as f32)
        }
    }
}
