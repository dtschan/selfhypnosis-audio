use std::io::{self, SeekFrom, Write};

use bytemuck::{cast_slice};
use color_eyre::eyre::Report;
use tracing::{instrument, warn};

/// Add TPDF dithering (triangular probability density function)
/// This reduces quantization distortion when converting to lower bit depth
pub fn tpdf_dither() -> f32 {
    let r1 = fastrand::f32();
    let r2 = fastrand::f32();
    (r1 - r2) / i16::MAX as f32
}

fn f32_to_i16_tpdf(sample: f32) -> i16 {
    let s = sample + tpdf_dither();

    (s * 32768.0)
        .round()
        .clamp(i16::MIN as f32, i16::MAX as f32) as i16
}

fn f32_to_i16(sample: f32) -> i16 {
    (sample * 32768.0)
        .round()
        .clamp(i16::MIN as f32, i16::MAX as f32) as i16
}

pub trait AudioRead {
    fn read_f32(&mut self, buf: &mut [f32]) -> Result<usize, io::Error>;

    fn read_exact_f32(&mut self, buf: &mut [f32]) -> Result<(), io::Error> {
        let mut total_read = 0;

         while total_read < buf.len() {
             match self.read_f32(&mut buf[total_read..]) {
                 Ok(0) => break,
                 Ok(n) => total_read += n,
                 Err(ref e) if e.kind() == io::ErrorKind::Interrupted => {}
                 Err(e) => return Err(e),
             }
         }

        Ok(())
    }

    fn read_to_end_f32(&mut self, buf: &mut Vec<f32>) -> Result<usize, io::Error> {
        let mut total_read = 0;

        loop {
            buf.resize(total_read + 4096, 0.0);
            match self.read_f32(&mut buf[total_read..]) {
                Ok(0) => {
                    buf.truncate(total_read);
                    break;
                }
                Ok(n) => {
                    total_read += n;
                }
                Err(ref e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e),
            }
        }

        Ok(total_read)
    }

    fn seek(&mut self, pos: SeekFrom) -> Result<u64, io::Error>;
}

pub struct AudioWriter<W: Write> {
    writer: W,
}

impl<W: Write> AudioWriter<W> {
    #[instrument(skip(writer))]
    pub fn new(writer: W) -> Self {
        Self { writer }
    }

    #[instrument(skip(self, samples), fields(num_samples = samples.len()))]
    pub fn write_i16(&mut self, samples: &[i16]) -> Result<(), Report> {
        Ok(self.writer.write_all(cast_slice::<i16, u8>(samples))?)
    }

    #[instrument(skip(self, samples), fields(num_samples = samples.len()))]
    pub fn write_f32_dither(&mut self, samples: &[f32]) -> Result<(), Report> {
        let samples = samples
            .into_iter()
            .map(|s| f32_to_i16_tpdf(*s))
            .collect::<Vec<_>>();
        self.write_i16(&samples)
    }

    pub fn write_f32(&mut self, samples: &[f32]) -> Result<(), Report> {
        let samples = samples
            .into_iter()
            .map(|s| f32_to_i16(*s))
            .collect::<Vec<_>>();
        self.write_i16(&samples)
    }

    pub fn flush(&mut self) -> Result<(), Report> {
        Ok(self.writer.flush()?)
    }
}

pub struct LoopAudioReader<R: AudioRead> {
    readers: Vec<R>,
    current: usize,
}

impl<R: AudioRead> LoopAudioReader<R> {
    pub fn new(readers: Vec<R>) -> Result<Self, Report> {
        Ok(Self {
            readers,
            current: 0,
        })
    }
}

impl<R: AudioRead> AudioRead for LoopAudioReader<R> {
    fn read_f32(&mut self, buf: &mut [f32]) -> Result<usize, io::Error> {
        // If we have no readers to loop over, just produce silence
        if self.readers.is_empty() {
            buf.fill(0.0);
            return Ok(buf.len());
        }

        let read = self.readers[self.current].read_f32(buf)?;
        if read == 0 {
            self.current = (self.current + 1) % self.readers.len();
            self.readers[self.current].seek(SeekFrom::Start(0))?;
            return self.readers[self.current].read_f32(buf);
        }

        return Ok(read);
    }

    fn read_to_end_f32(&mut self, _buf: &mut Vec<f32>) -> Result<usize, io::Error> {
        // Reading to end would produce endless loop, abort
        assert!(false);

        Ok(0)
    }

    fn seek(&mut self, pos: SeekFrom) -> Result<u64, io::Error> {
        self.readers[self.current].seek(pos)
    }
}

/*
#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::AudioReader;

    #[test]
    fn loopread_with_audioreader() {
        let path = "generated/ruhiger--f34b557a6c2791cb.flac";
        let mut direct = Vec::new();
        AudioReader::new(FlacReader::new(path).unwrap())
            .read_to_end_f32(&mut direct)
            .unwrap();

        let mut looped = vec![0.0f32; direct.len() * 2];
        AudioReader::new(LoopRead::new(vec![path.to_string()]).unwrap())
            .read_exact_f32(&mut looped)
            .unwrap();

        assert_eq!(&looped[..direct.len()], &direct[..]);
        assert_eq!(&looped[direct.len()..], &direct[..]);
    }

    #[test]
    fn loopread_boxed_with_audioreader() {
        let path = "generated/ruhiger--f34b557a6c2791cb.flac";
        let mut direct = Vec::new();
        AudioReader::new(FlacReader::new(path).unwrap())
            .read_to_end_f32(&mut direct)
            .unwrap();

        let mut looped = vec![0.0f32; direct.len() * 2];
        let reader: Box<dyn std::io::Read> =
            Box::new(LoopRead::new(vec![path.to_string()]).unwrap());
        AudioReader::new(reader)
            .read_exact_f32(&mut looped)
            .unwrap();

        assert_eq!(&looped[..direct.len()], &direct[..]);
        assert_eq!(&looped[direct.len()..], &direct[..]);
    }

    #[test]
    fn loopread_two_files_with_audioreader() {
        let path1 = "generated/ruhiger--f34b557a6c2791cb.flac";
        let path2 = "generated/vier--b46faa5c1479852c.flac";

        let mut file1 = Vec::new();
        AudioReader::new(FlacReader::new(path1).unwrap())
            .read_to_end_f32(&mut file1)
            .unwrap();
        let mut file2 = Vec::new();
        AudioReader::new(FlacReader::new(path2).unwrap())
            .read_to_end_f32(&mut file2)
            .unwrap();

        let mut looped = vec![0.0f32; file1.len() + file2.len() + file1.len()];
        let reader: Box<dyn std::io::Read> =
            Box::new(LoopRead::new(vec![path1.to_string(), path2.to_string()]).unwrap());
        AudioReader::new(reader)
            .read_exact_f32(&mut looped)
            .unwrap();

        assert_eq!(&looped[..file1.len()], &file1[..]);
        assert_eq!(&looped[file1.len()..file1.len() + file2.len()], &file2[..]);
        assert_eq!(&looped[file1.len() + file2.len()..], &file1[..]);
    }

    #[test]
    fn loopread_unaligned_with_audioreader() {
        let path = "generated/ruhiger--f34b557a6c2791cb.flac";
        let mut direct = Vec::new();
        AudioReader::new(FlacReader::new(path).unwrap())
            .read_to_end_f32(&mut direct)
            .unwrap();

        let extra = 12345usize;
        let mut looped = vec![0.0f32; direct.len() + extra];
        let reader: Box<dyn std::io::Read> =
            Box::new(LoopRead::new(vec![path.to_string()]).unwrap());
        AudioReader::new(reader)
            .read_exact_f32(&mut looped)
            .unwrap();

        assert_eq!(&looped[..direct.len()], &direct[..]);
        assert_eq!(&looped[direct.len()..], &direct[..extra]);
    }

    #[test]
    fn loopread_sequential_reads_with_audioreader() {
        let path = "generated/ruhiger--f34b557a6c2791cb.flac";
        let mut direct = Vec::new();
        AudioReader::new(FlacReader::new(path).unwrap())
            .read_to_end_f32(&mut direct)
            .unwrap();

        let sizes = [123usize, 4567, 89, 1024 * 1024, 3, 5, 7];
        let reader: Box<dyn std::io::Read> =
            Box::new(LoopRead::new(vec![path.to_string()]).unwrap());
        let mut audio = AudioReader::new(reader);
        let mut all = Vec::new();
        for &size in &sizes {
            let mut buf = vec![0.0f32; size];
            audio.read_exact_f32(&mut buf).unwrap();
            all.extend(buf);
        }

        for (i, &expected) in all.iter().enumerate() {
            assert!(
                (expected - direct[i % direct.len()]).abs() < 1e-6,
                "mismatch at index {i}: got {expected}, expected {}",
                direct[i % direct.len()]
            );
        }
    }
}
*/
