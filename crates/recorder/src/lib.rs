//! Raw feed recorder (V4 §26). Files are sequences of independent zstd frames, so a crash
//! loses at most the unflushed tail. Record layout inside a frame (little endian):
//! `u8 source | u64 recv_wall_ns | u64 recv_mono_ns | u32 len | payload[len]`.

use anyhow::{bail, Context, Result};
use events::{RawMsg, Source};
use std::fs::{File, OpenOptions};
use std::io::{BufReader, Read, Write};
use std::path::{Path, PathBuf};

const FRAME_BYTES: usize = 1 << 20;
const HEADER: usize = 1 + 8 + 8 + 4;

pub struct Recorder {
    dir: PathBuf,
    seq: u32,
    file: File,
    file_started_mono: u64,
    rotate_ns: u64,
    buf: Vec<u8>,
    last_flush_mono: u64,
    flush_ns: u64,
    pub bytes_raw: u64,
    pub bytes_written: u64,
    level: i32,
    frame_bytes: usize,
}

fn seg_path(dir: &Path, seq: u32) -> PathBuf {
    dir.join(format!("raw-{seq:06}.zst"))
}

impl Recorder {
    /// `rotate_secs`: start a new segment file after this long. `flush_ms`: max age of buffered data.
    pub fn create(dir: &Path, now_mono: u64, rotate_secs: u64, flush_ms: u64) -> Result<Self> {
        std::fs::create_dir_all(dir)?;
        if seg_path(dir, 0).exists() {
            bail!("{} already contains a recording", dir.display());
        }
        let file = OpenOptions::new().create_new(true).append(true).open(seg_path(dir, 0))?;
        Ok(Self {
            dir: dir.to_path_buf(),
            seq: 0,
            file,
            file_started_mono: now_mono,
            rotate_ns: rotate_secs * 1_000_000_000,
            buf: Vec::with_capacity(FRAME_BYTES + 4096),
            last_flush_mono: now_mono,
            flush_ns: flush_ms * 1_000_000,
            bytes_raw: 0,
            bytes_written: 0,
            level: 3,
            frame_bytes: FRAME_BYTES,
        })
    }

    /// Bigger frames and a higher zstd level for bulk (backfill) recording. Format is unchanged.
    pub fn with_compression(mut self, level: i32, frame_bytes: usize) -> Self {
        self.level = level;
        self.frame_bytes = frame_bytes;
        self
    }

    pub fn write(&mut self, m: &RawMsg) -> Result<()> {
        self.buf.push(m.source as u8);
        self.buf.extend_from_slice(&m.recv_wall_ns.to_le_bytes());
        self.buf.extend_from_slice(&m.recv_mono_ns.to_le_bytes());
        self.buf.extend_from_slice(&(m.payload.len() as u32).to_le_bytes());
        self.buf.extend_from_slice(&m.payload);
        self.bytes_raw += (HEADER + m.payload.len()) as u64;
        if self.buf.len() >= self.frame_bytes || m.recv_mono_ns.saturating_sub(self.last_flush_mono) >= self.flush_ns {
            self.flush(m.recv_mono_ns)?;
        }
        Ok(())
    }

    pub fn flush(&mut self, now_mono: u64) -> Result<()> {
        self.last_flush_mono = now_mono;
        if self.buf.is_empty() {
            return Ok(());
        }
        let frame = zstd::bulk::compress(&self.buf, self.level)?;
        self.file.write_all(&frame)?;
        self.bytes_written += frame.len() as u64;
        self.buf.clear();
        if now_mono.saturating_sub(self.file_started_mono) >= self.rotate_ns {
            self.file.sync_data()?;
            self.seq += 1;
            self.file = OpenOptions::new().create_new(true).append(true).open(seg_path(&self.dir, self.seq))?;
            self.file_started_mono = now_mono;
        }
        Ok(())
    }

    pub fn close(mut self, now_mono: u64) -> Result<()> {
        self.flush(now_mono)?;
        self.file.sync_all()?;
        Ok(())
    }
}

/// Reads every segment of a recording in order.
pub struct Reader {
    segments: Vec<PathBuf>,
    next_seg: usize,
    cur: Option<zstd::stream::read::Decoder<'static, BufReader<File>>>,
    /// Bytes of an incomplete trailing record/frame (crash tail), per segment.
    pub truncated_tails: u64,
}

impl Reader {
    pub fn open(dir: &Path) -> Result<Self> {
        let mut segments: Vec<PathBuf> = std::fs::read_dir(dir)
            .with_context(|| format!("read {}", dir.display()))?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("raw-") && n.ends_with(".zst")))
            .collect();
        segments.sort();
        if segments.is_empty() {
            bail!("no segments in {}", dir.display());
        }
        Ok(Self { segments, next_seg: 0, cur: None, truncated_tails: 0 })
    }

    fn read_exact_or_eof(&mut self, buf: &mut [u8]) -> Result<bool> {
        let dec = self.cur.as_mut().expect("open segment");
        let mut got = 0;
        while got < buf.len() {
            match dec.read(&mut buf[got..]) {
                Ok(0) => break,
                Ok(n) => got += n,
                // Incomplete final zstd frame (crash): treat as end of segment.
                Err(_) => {
                    self.truncated_tails += 1;
                    return Ok(false);
                }
            }
        }
        if got == 0 {
            return Ok(false);
        }
        if got < buf.len() {
            self.truncated_tails += 1;
            return Ok(false);
        }
        Ok(true)
    }

    pub fn next_msg(&mut self) -> Result<Option<RawMsg>> {
        loop {
            if self.cur.is_none() {
                if self.next_seg >= self.segments.len() {
                    return Ok(None);
                }
                let f = File::open(&self.segments[self.next_seg])?;
                self.next_seg += 1;
                self.cur = Some(zstd::stream::read::Decoder::new(f)?);
            }
            let mut h = [0u8; HEADER];
            if !self.read_exact_or_eof(&mut h)? {
                self.cur = None;
                continue;
            }
            let source = Source::from_u8(h[0]).with_context(|| format!("bad source byte {}", h[0]))?;
            let recv_wall_ns = u64::from_le_bytes(h[1..9].try_into().expect("8 bytes"));
            let recv_mono_ns = u64::from_le_bytes(h[9..17].try_into().expect("8 bytes"));
            let len = u32::from_le_bytes(h[17..21].try_into().expect("4 bytes")) as usize;
            let mut payload = vec![0u8; len];
            if len > 0 && !self.read_exact_or_eof(&mut payload)? {
                self.cur = None;
                continue;
            }
            return Ok(Some(RawMsg { source, recv_wall_ns, recv_mono_ns, payload }));
        }
    }
}

impl Iterator for Reader {
    type Item = Result<RawMsg>;
    fn next(&mut self) -> Option<Self::Item> {
        self.next_msg().transpose()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("rec-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn roundtrip_with_rotation_and_crash_tail() {
        let d = tmpdir("rt");
        let mut r = Recorder::create(&d, 0, 1, 10).unwrap();
        let mut msgs = Vec::new();
        for i in 0..5000u64 {
            let m = RawMsg {
                source: if i % 2 == 0 { Source::Grpc } else { Source::Ws },
                recv_wall_ns: 1_000 + i,
                recv_mono_ns: i * 1_000_000, // 1 ms apart -> flush every 10 ms, rotate every 1 s
                payload: format!("payload-{i}").into_bytes(),
            };
            r.write(&m).unwrap();
            msgs.push(m);
        }
        r.close(5_000_000_000).unwrap();
        let segs = std::fs::read_dir(&d).unwrap().count();
        assert!(segs >= 4, "expected rotation, got {segs}");
        let got: Vec<RawMsg> = Reader::open(&d).unwrap().map(|m| m.unwrap()).collect();
        assert_eq!(got, msgs);

        // Simulate a crash: chop bytes off the last segment.
        let mut last: Vec<_> = std::fs::read_dir(&d).unwrap().map(|e| e.unwrap().path()).collect();
        last.sort();
        let p = last.iter().rev().find(|p| std::fs::metadata(p).unwrap().len() > 20).unwrap().clone();
        let data = std::fs::read(&p).unwrap();
        std::fs::write(&p, &data[..data.len() - 7]).unwrap();
        let mut rd = Reader::open(&d).unwrap();
        let mut n = 0;
        while let Some(m) = rd.next_msg().unwrap() {
            assert_eq!(m, msgs[n]);
            n += 1;
        }
        assert!(n < msgs.len());
        assert!(rd.truncated_tails >= 1);
        std::fs::remove_dir_all(&d).unwrap();
    }
}
