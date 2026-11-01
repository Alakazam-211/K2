//! The null device backend: a clock thread stands in for the audio device.

use crate::device::{
    null_input_info, null_output_info, pop_playback, push_capture, ring_capacity, CaptureIo,
    NullDevice, PlayIo, NULL_INPUT_ID, NULL_OUTPUT_ID,
};
use crate::engine::{is_default, lock, Ctrl, Shared};
use crate::error::{AudioError, ErrorCode};
use crate::events::{DeviceChoice, DeviceList, DeviceSelection, Event};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

pub(crate) fn start(shared: &Arc<Shared>, dev: NullDevice) -> Result<JoinHandle<()>, AudioError> {
    let rate = dev.sample_rate;
    let cap = ring_capacity(rate, shared.playout_buffer_ms);
    let (cap_tx, cap_rx) = rtrb::RingBuffer::<f32>::new(cap);
    let (play_tx, play_rx) = rtrb::RingBuffer::<f32>::new(cap);
    let capture = CaptureIo::new(cap_rx, rate)?;
    let playback = PlayIo::new(play_tx, rate, shared.playout_buffer_ms)?;
    shared.send_ctrl(Ctrl::CaptureIo(Some(Box::new(capture))));
    shared.send_ctrl(Ctrl::PlayIo(Some(Box::new(playback))));
    let list = DeviceList {
        inputs: vec![null_input_info()],
        outputs: vec![null_output_info()],
    };
    *lock(&shared.devices) = list.clone();
    shared.emit(Event::Devices(list));
    shared.set_in_use(|u| {
        u.input = Some(null_input_info());
        u.output = Some(null_output_info());
    });

    let s = shared.clone();
    std::thread::Builder::new()
        .name("kessel-audio-null".into())
        .spawn(move || clock_main(s, dev, cap_tx, play_rx))
        .map_err(|e| AudioError::new(ErrorCode::Backend, format!("spawn: {e}")))
}

fn clock_main(
    shared: Arc<Shared>,
    mut dev: NullDevice,
    mut cap_tx: rtrb::Producer<f32>,
    mut play_rx: rtrb::Consumer<f32>,
) {
    let block = (dev.sample_rate / 100) as usize;
    let mut inbuf = vec![0.0f32; block];
    let mut outbuf = vec![0.0f32; block];
    let period = dev.block_period;
    let mut next = Instant::now();
    while !shared.stop.load(Ordering::Acquire) {
        match dev.input.as_mut() {
            Some(f) => f(&mut inbuf),
            None => inbuf.fill(0.0),
        }
        push_capture(&mut cap_tx, &inbuf, 1, |v| v, &shared.io);
        pop_playback(&mut play_rx, &mut outbuf, 1, |v| v, &shared.io);
        if let Some(f) = dev.output.as_mut() {
            f(&outbuf);
        }
        next += period;
        let now = Instant::now();
        if next > now {
            std::thread::sleep(next - now);
        } else if now - next > Duration::from_millis(200) {
            // Far behind (the machine was suspended): don't burst to catch up.
            next = now;
        }
    }
}

/// The null backend has one device each way.
pub(crate) fn check_selection(sel: &DeviceSelection) -> Result<(), AudioError> {
    let ok = |c: &Option<DeviceChoice>, id: &str| {
        is_default(c) || matches!(c, Some(DeviceChoice::Id(x)) if x == id)
    };
    if ok(&sel.input, NULL_INPUT_ID) && ok(&sel.output, NULL_OUTPUT_ID) {
        Ok(())
    } else {
        Err(AudioError::new(ErrorCode::NoDevice, "no such device"))
    }
}
