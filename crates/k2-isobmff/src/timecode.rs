//! QuickTime timecode tracks (`tmcd`): the sample entry and the
//! frame-number-to-timecode conversion (QuickTime File Format, "Timecode
//! sample description"). A tmcd track's first sample is a 32-bit frame
//! number: the source's start timecode, often 01:00:00:00.

use crate::bytes::Cur;
use crate::error::{Error, Result};
use std::fmt;

/// The `tmcd` sample entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TimecodeEntry {
    /// 0x01 drop frame, 0x02 24-hour max, 0x04 negative times OK,
    /// 0x08 counter.
    pub flags: u32,
    pub timescale: u32,
    pub frame_duration: u32,
    /// Nominal frames per second (30 for 29.97 drop frame).
    pub number_of_frames: u8,
}

impl TimecodeEntry {
    pub(crate) fn parse(c: &mut Cur<'_>) -> Result<TimecodeEntry> {
        c.skip(4)?;
        let flags = c.u32()?;
        let timescale = c.u32()?;
        let frame_duration = c.u32()?;
        let number_of_frames = c.u8()?;
        if number_of_frames == 0 {
            return Err(Error::Invalid { fourcc: c.fourcc, what: "tmcd number of frames is 0" });
        }
        Ok(TimecodeEntry { flags, timescale, frame_duration, number_of_frames })
    }
    pub fn drop_frame(&self) -> bool {
        self.flags & 1 != 0
    }
}

/// A SMPTE-style timecode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Timecode {
    pub negative: bool,
    pub hours: u32,
    pub minutes: u8,
    pub seconds: u8,
    pub frames: u8,
    pub drop_frame: bool,
    /// The frame number the sample held.
    pub frame_number: i64,
}

impl Timecode {
    /// Converts a tmcd frame number with the entry's rate and flags.
    /// Drop-frame drops `round(fps / 15)` frame numbers each minute except
    /// every tenth (2 at 30, 4 at 60).
    pub fn from_frame_number(frame_number: i64, entry: &TimecodeEntry) -> Timecode {
        let fps = entry.number_of_frames.max(1) as u64;
        let drop_frame = entry.drop_frame();
        let negative = frame_number < 0;
        let mut f = frame_number.unsigned_abs();
        if drop_frame {
            let drop = (fps + 7) / 15; // round(fps / 15): 2 at 30, 4 at 60
            let per_min = fps * 60 - drop;
            let per_10min = fps * 600 - drop * 9;
            let d = f / per_10min;
            let m = f % per_10min;
            let extra = if m > drop { drop * ((m - drop) / per_min) } else { 0 };
            f = f.saturating_add(drop * 9 * d + extra);
        }
        let frames = (f % fps) as u8;
        let total_secs = f / fps;
        let mut hours = total_secs / 3600;
        if entry.flags & 2 != 0 {
            hours %= 24;
        }
        Timecode {
            negative,
            hours: hours.min(u32::MAX as u64) as u32,
            minutes: ((total_secs / 60) % 60) as u8,
            seconds: (total_secs % 60) as u8,
            frames,
            drop_frame,
            frame_number,
        }
    }
}

impl fmt::Display for Timecode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let sep = if self.drop_frame { ';' } else { ':' };
        write!(f, "{}{:02}:{:02}:{:02}{}{:02}", if self.negative { "-" } else { "" }, self.hours, self.minutes, self.seconds, sep, self.frames)
    }
}
