//! Statistics.

/// Receive statistics for one participant, cumulative since it was added.
///
/// The four headline fields (`jitter_ms`, `loss_pct`, `concealed_pct`,
/// `jb_delay_ms`) are what a stats beacon carries; the counters let a caller
/// compute its own windows by differencing two readings.
#[derive(Clone, Debug, Default, PartialEq)]
#[non_exhaustive]
pub struct ParticipantStats {
    /// Interarrival jitter (RFC 3550 estimator), milliseconds.
    pub jitter_ms: f32,
    /// Packets lost as a percentage of packets expected (from sequence
    /// numbers; late packets that arrived count as received).
    pub loss_pct: f32,
    /// Share of played audio that was synthesised by loss concealment
    /// (not counting DTX silence or FEC recovery), percent.
    pub concealed_pct: f32,
    /// Smoothed playout delay added by the jitter buffer, milliseconds.
    pub jb_delay_ms: f32,
    /// The jitter buffer's current target delay, milliseconds.
    pub jb_target_ms: f32,
    /// The longest run of concealed audio so far, milliseconds.
    pub longest_concealment_ms: f32,
    /// Unique packets received.
    pub packets_received: u64,
    /// Packets expected from the sequence-number range.
    pub packets_expected: u64,
    /// Packets that arrived after their playout time and were dropped.
    pub packets_late: u64,
    /// Duplicate packets dropped.
    pub packets_duplicate: u64,
    /// 20 ms frames decoded from packets.
    pub frames_decoded: u64,
    /// 20 ms frames rebuilt from the next packet's in-band FEC.
    pub frames_fec: u64,
    /// 20 ms frames synthesised by loss concealment.
    pub frames_concealed: u64,
    /// 20 ms frames of sender DTX (silence or comfort noise).
    pub frames_dtx: u64,
    /// 20 ms frames dropped to bring the delay back to the target.
    pub frames_dropped: u64,
}

/// Capture-side counters.
#[derive(Clone, Debug, Default, PartialEq)]
#[non_exhaustive]
pub struct CaptureStats {
    /// Frames pushed to the [`crate::FrameRx`] queue.
    pub frames_sent: u64,
    /// Frames the encoder marked as nothing-to-send in DTX (not pushed).
    pub frames_dtx_suppressed: u64,
    /// Frames dropped because the [`crate::FrameRx`] queue was full.
    pub frames_dropped: u64,
    /// Capture samples dropped because the processing thread fell behind.
    pub capture_overruns: u64,
    /// Playback samples played as silence because the processing thread
    /// fell behind.
    pub playout_underruns: u64,
}

/// A statistics snapshot ([`crate::AudioEngine::stats`]).
#[derive(Clone, Debug, Default, PartialEq)]
#[non_exhaustive]
pub struct Stats {
    /// Per participant, by id.
    pub participants: Vec<(u64, ParticipantStats)>,
    /// The echo canceller's measured delay between playout and capture,
    /// milliseconds (`None` until it has an estimate, or with echo
    /// cancellation off).
    pub aec_delay_ms: Option<i32>,
    /// The echo canceller's own echo-return-loss-enhancement estimate, dB.
    pub aec_erle_db: Option<f32>,
    /// Capture counters.
    pub capture: CaptureStats,
    /// Received frames dropped because the receive queue was full.
    pub recv_queue_drops: u64,
    /// Received frames for participants that were never added.
    pub unknown_participant_frames: u64,
}
