use std::io::{self, BufReader, Read, Seek, SeekFrom, Write};
use std::{fs::File, io::BufWriter, path::Path};

use bytemuck::cast_slice_mut;
use color_eyre::eyre::Report;
use flac_codec::decode::FlacByteReader;
use flac_codec::decode::Metadata;
use flac_codec::encode::{FlacByteWriter, Options};
use flac_codec::metadata::{self, VorbisComment};
use tracing::instrument;

#[cfg(target_endian = "big")]
use flac_codec::byteorder::BigEndian as NativeEndian;
#[cfg(target_endian = "little")]
use flac_codec::byteorder::LittleEndian as NativeEndian;

use crate::audio::AudioRead;

pub struct FlacReader {
    reader: FlacByteReader<BufReader<File>, NativeEndian>,
    gain: f32,
}

impl FlacReader {
    #[instrument(skip(path), fields(path = %path.as_ref().display()))]
    pub fn new<P: AsRef<Path>>(path: P, gain: f32) -> Result<Self, Report> {
        Ok(Self {
            reader: FlacByteReader::open(path, NativeEndian)?,
            gain,
        })
    }

    pub fn sample_rate(&self) -> u32 {
        self.reader.metadata().sample_rate()
    }
}

impl AudioRead for FlacReader {
    #[instrument(skip(self))]
    fn read_f32(&mut self, buf: &mut [f32]) -> Result<usize, io::Error> {
        let mut buf_i16 = vec![0i16; buf.len()];
        let bytes_read = self.reader.read(cast_slice_mut::<i16, u8>(&mut buf_i16))?;
        let samples_read = bytes_read / 2;

        for (&src, dst) in buf_i16[..samples_read].iter().zip(buf.iter_mut()) {
            *dst = (src as f32 / i16::MAX as f32) * self.gain;
        }

        Ok(samples_read)
    }

    /*#[instrument(skip(self))]
    fn read_exact_f32(&mut self, buf: &mut [f32]) -> Result<usize, io::Error> {
        let mut buf_i16 = vec![0i16; buf.len()];
        self.read_exact_i16(&mut buf_i16)?;
        for (s, &i) in buf.iter_mut().zip(buf_i16.iter()) {
            *s = i as f32 / i16::MAX as f32;
        }
        Ok(buf.len())
    }*/

    /*#[instrument(skip(self))]
    fn read_exact_i16(&mut self, buf: &mut [i16]) -> Result<usize, io::Error> {
        // let metadata = self.reader.metadata().streaminfo();
        // if metadata.bits_per_sample.try_into().unwrap_or(0) != 16 {
        //     return Err(eyre!("Unsupported bits per sample: {}", metadata.bits_per_sample.try_into().unwrap_or(0)));
        // }
        // let total_samples = metadata.total_samples.ok_or(eyre!("Can't determine file size"))?;
        // let mut samples = vec![0i16; total_samples.get() as usize];
        self.reader.read_exact(cast_slice_mut::<i16, u8>(buf))?;

        Ok(buf.len())
    }

    #[instrument(skip(self))]
    fn read_to_end_i16(&mut self, buf: &mut Vec<i16>) -> Result<usize, io::Error> {
        let mut len_bytes = buf.len() * 2;
        loop {
            buf.resize(len_bytes / 2 + 2048, 0);
            let read_bytes = self
                .reader
                .read(&mut cast_slice_mut::<i16, u8>(buf.as_mut_slice())[len_bytes..])?;
            if read_bytes == 0 {
                // This drops the last incomplete sample in case of odd number of bytes, which is what we want
                buf.truncate(len_bytes / 2);
                return Ok(buf.len());
            }

            len_bytes += read_bytes;
        }*/

        /*  let start_len = buf.len();
        let mut pending: Option<u8> = None;

        loop {
            const CHUNK_SAMPLES: usize = 1024;
            let old_len = buf.len();
            buf.resize(old_len + CHUNK_SAMPLES, 0);

            let (bytes_read, pending_byte) = {
                let u8_view = cast_slice_mut::<i16, u8>(&mut buf[old_len..]);
                let bytes_read = if let Some(b) = pending.take() {
                    u8_view[0] = b;
                    match self.reader.read(&mut u8_view[1..])? {
                        0 => 0,
                        n => 1 + n,
                    }
                } else {
                    self.reader.read(u8_view)?
                };

                let pending_byte =
                    (bytes_read > 0 && bytes_read % 2 == 1).then(|| u8_view[bytes_read - 1]);
                (bytes_read, pending_byte)
            };

            if bytes_read == 0 {
                buf.resize(old_len, 0);
                break;
            }

            let samples_read = bytes_read / 2;
            buf.resize(old_len + samples_read, 0);
            pending = pending_byte;
        }

        Ok(buf.len() - start_len) */

    /*#[instrument(skip(self))]
    fn read_to_end_f32(&mut self, buf: &mut Vec<f32>) -> Result<usize, io::Error> {
        let mut buf_i16 = Vec::new();
        self.read_to_end_i16(&mut buf_i16)?;
        buf.resize(buf_i16.len(), 0.0);

        for (s, &i) in buf.iter_mut().zip(buf_i16.iter()) {
            *s = i as f32 / i16::MAX as f32;
        }

        Ok(buf.len())
    }*/

    fn seek(&mut self, pos: SeekFrom) -> Result<u64, io::Error> {
       Ok(self.reader.seek(pos)?)
    }
}

/*impl Read for FlacReader {
    #[instrument(skip(self))]
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.reader.read(buf)
    }
}

impl Seek for FlacReader {
    #[instrument(skip(self, pos))]
    fn seek(&mut self, pos: std::io::SeekFrom) -> std::io::Result<u64> {
        self.reader.seek(pos)
    }
}*/

/// Reads the TRACKNUMBER Vorbis comment (e.g. "24" or "24/30").
pub fn track_number<P: AsRef<Path>>(path: P) -> Option<u32> {
    let comment = metadata::block::<_, VorbisComment>(path).ok()??;
    comment
        .get("TRACKNUMBER")?
        .split('/')
        .next()?
        .trim()
        .parse()
        .ok()
}

pub struct FlacWriter {
    writer: FlacByteWriter<BufWriter<File>, NativeEndian>,
}

impl FlacWriter {
    #[instrument(skip(path), fields(path = %path.as_ref().display()))]
    pub fn new<P: AsRef<Path>>(
        path: P,
        sample_rate: u32,
        bits_per_sample: u32,
        channels: u8,
    ) -> Result<Self, Report> {
        Ok(Self {
            writer: FlacByteWriter::create(
                path,
                Options::best().overwrite(),
                sample_rate,
                bits_per_sample,
                channels,
                None,
            )?,
        })
    }
}

impl Write for FlacWriter {
    #[instrument(skip(self, buf), fields(buf_len = buf.len()))]
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.writer.write(buf)
    }

    #[instrument(skip(self))]
    fn flush(&mut self) -> std::io::Result<()> {
        self.writer.flush()
    }
}
