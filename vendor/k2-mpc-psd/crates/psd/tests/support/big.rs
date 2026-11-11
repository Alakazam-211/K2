//! A PSB of any size generated on the fly, never on disk: for the memory tests.

use std::io::Read;

/// A PSB generated on the fly: prefix, then zeroed channel bytes, then the end of the section.
pub struct BigPsb {
    prefix: Vec<u8>,
    at_prefix: usize,
    zeros: u64,
    suffix: Vec<u8>,
    at_suffix: usize,
}

impl Read for BigPsb {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        if self.at_prefix < self.prefix.len() {
            let n = out.len().min(self.prefix.len() - self.at_prefix);
            out[..n].copy_from_slice(&self.prefix[self.at_prefix..self.at_prefix + n]);
            self.at_prefix += n;
            return Ok(n);
        }
        if self.zeros > 0 {
            let n = (out.len() as u64).min(self.zeros) as usize;
            out[..n].fill(0);
            self.zeros -= n as u64;
            return Ok(n);
        }
        let n = out.len().min(self.suffix.len() - self.at_suffix);
        out[..n].copy_from_slice(&self.suffix[self.at_suffix..self.at_suffix + n]);
        self.at_suffix += n;
        Ok(n)
    }
}

pub fn big_psb(layers: u16, channels_per_layer: u16, channel_bytes: u64) -> (BigPsb, u64) {
    let be32 = |v: u32| v.to_be_bytes();
    let be64 = |v: u64| v.to_be_bytes();
    let mut li = Vec::new();
    li.extend((layers as i16).to_be_bytes());
    for l in 0..layers {
        li.extend([0u8; 16]);
        li.extend(channels_per_layer.to_be_bytes());
        for c in 0..channels_per_layer {
            li.extend((c as i16 - 1).to_be_bytes());
            li.extend(be64(channel_bytes + 2));
        }
        li.extend_from_slice(b"8BIMnorm");
        li.extend([255u8, 0, 0, 0]);
        let mut extra = vec![0u8; 8];
        extra.push(4);
        extra.extend_from_slice(format!("L{l:03}").as_bytes());
        while !extra.len().is_multiple_of(4) {
            extra.push(0);
        }
        li.extend(be32(extra.len() as u32));
        li.extend(extra);
    }
    let total_channels = u64::from(layers) * u64::from(channels_per_layer);
    let li_len = li.len() as u64 + total_channels * (channel_bytes + 2);
    let lm_len = 8 + li_len + 4;
    let mut prefix = b"8BPS".to_vec();
    prefix.extend([0u8, 2]);
    prefix.extend([0u8; 6]);
    prefix.extend([0u8, 1]);
    prefix.extend(be32(30_000));
    prefix.extend(be32(30_000));
    prefix.extend([0u8, 16]);
    prefix.extend([0u8, 3]);
    prefix.extend(be32(0));
    prefix.extend(be32(0));
    prefix.extend(be64(lm_len));
    prefix.extend(be64(li_len));
    prefix.extend_from_slice(&li);
    let zeros = total_channels * (channel_bytes + 2);
    let total = prefix.len() as u64 + zeros + 4;
    (
        BigPsb {
            prefix,
            at_prefix: 0,
            zeros,
            suffix: be32(0).to_vec(),
            at_suffix: 0,
        },
        total,
    )
}
