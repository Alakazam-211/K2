//! The system device backend (cpal). Lives entirely on the control thread:
//! it opens and closes streams, polls the device list for hot-plug, and
//! reopens or falls back when a device goes away.

use crate::device::{pop_playback, push_capture, ring_capacity, CaptureIo, PlayIo};
use crate::engine::{lock, Ctrl, Shared};
use crate::error::{AudioError, ErrorCode};
use crate::events::{DeviceChoice, DeviceInfo, DeviceList, DeviceSelection, Event};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{BufferSize, ErrorKind, FromSample, SampleFormat, SizedSample, SupportedBufferSize};
use crossbeam_queue::ArrayQueue;
use std::sync::Arc;
use std::time::Instant;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Dir {
    In,
    Out,
}

struct StreamErr {
    dir: Dir,
    generation: u64,
    err: cpal::Error,
}

struct Open {
    // Held to keep the stream running; dropping it stops the stream.
    _stream: cpal::Stream,
    info: DeviceInfo,
    generation: u64,
}

pub(crate) struct System {
    host: cpal::Host,
    want_in: DeviceChoice,
    want_out: DeviceChoice,
    input: Option<Open>,
    output: Option<Open>,
    errors: Arc<ArrayQueue<StreamErr>>,
    generation: u64,
    last_poll: Instant,
    list: DeviceList,
}

/// Map an error from opening a device.
fn open_err(e: cpal::Error) -> AudioError {
    let code = match e.kind() {
        ErrorKind::DeviceBusy => ErrorCode::DeviceBusy,
        ErrorKind::PermissionDenied => ErrorCode::NoPermission,
        ErrorKind::DeviceNotAvailable | ErrorKind::HostUnavailable => ErrorCode::NoDevice,
        ErrorKind::UnsupportedConfig | ErrorKind::UnsupportedOperation => {
            ErrorCode::UnsupportedConfig
        }
        _ => ErrorCode::Backend,
    };
    AudioError::new(code, e.to_string())
}

fn device_info(d: &cpal::Device, default_id: Option<&str>) -> Option<DeviceInfo> {
    let id = d.id().ok()?.to_string();
    let name = d
        .description()
        .map(|x| x.name().to_string())
        .unwrap_or_else(|_| id.clone());
    Some(DeviceInfo {
        is_default: default_id == Some(id.as_str()),
        id,
        name,
    })
}

fn list_devices(host: &cpal::Host) -> DeviceList {
    let def_in = host
        .default_input_device()
        .and_then(|d| d.id().ok())
        .map(|i| i.to_string());
    let def_out = host
        .default_output_device()
        .and_then(|d| d.id().ok())
        .map(|i| i.to_string());
    let inputs = host
        .input_devices()
        .map(|it| it.filter_map(|d| device_info(&d, def_in.as_deref())).collect())
        .unwrap_or_default();
    let outputs = host
        .output_devices()
        .map(|it| it.filter_map(|d| device_info(&d, def_out.as_deref())).collect())
        .unwrap_or_default();
    DeviceList { inputs, outputs }
}

fn find_device(host: &cpal::Host, choice: &DeviceChoice, dir: Dir) -> Result<cpal::Device, AudioError> {
    match choice {
        DeviceChoice::Default => match dir {
            Dir::In => host.default_input_device(),
            Dir::Out => host.default_output_device(),
        }
        .ok_or_else(|| AudioError::new(ErrorCode::NoDevice, "no default device")),
        DeviceChoice::Id(s) => {
            let id: cpal::DeviceId = s
                .parse()
                .map_err(|e: cpal::Error| AudioError::new(ErrorCode::NoDevice, e.to_string()))?;
            host.device_by_id(&id)
                .ok_or_else(|| AudioError::new(ErrorCode::NoDevice, format!("no device {s}")))
        }
    }
}

const FORMATS: [SampleFormat; 3] = [SampleFormat::F32, SampleFormat::I16, SampleFormat::I32];

/// Prefer 48 kHz (no resampling) in f32, then i16, then i32, with the
/// fewest channels; else the device's default configuration.
fn pick_config(
    dev: &cpal::Device,
    dir: Dir,
) -> Result<(cpal::StreamConfig, SampleFormat), AudioError> {
    let ranges: Vec<cpal::SupportedStreamConfigRange> = match dir {
        Dir::In => dev.supported_input_configs().map_err(open_err)?.collect(),
        Dir::Out => dev.supported_output_configs().map_err(open_err)?.collect(),
    };
    for fmt in FORMATS {
        let best = ranges
            .iter()
            .filter(|r| r.sample_format() == fmt && r.contains_rate(crate::frame::SAMPLE_RATE))
            .min_by_key(|r| r.channels());
        if let Some(r) = best {
            let buffer = *r.buffer_size();
            let mut cfg = (*r).with_sample_rate(crate::frame::SAMPLE_RATE).config();
            cfg.buffer_size = match buffer {
                SupportedBufferSize::Range { min, max } if min <= 480 && 480 <= max => {
                    BufferSize::Fixed(480)
                }
                _ => BufferSize::Default,
            };
            return Ok((cfg, fmt));
        }
    }
    let d = match dir {
        Dir::In => dev.default_input_config(),
        Dir::Out => dev.default_output_config(),
    }
    .map_err(open_err)?;
    if !FORMATS.contains(&d.sample_format()) {
        return Err(AudioError::new(
            ErrorCode::UnsupportedConfig,
            format!("sample format {:?}", d.sample_format()),
        ));
    }
    let mut cfg = d.config();
    cfg.buffer_size = BufferSize::Default;
    Ok((cfg, d.sample_format()))
}

fn build_input<T>(
    dev: &cpal::Device,
    cfg: &cpal::StreamConfig,
    mut tx: rtrb::Producer<f32>,
    shared: Arc<Shared>,
    errors: Arc<ArrayQueue<StreamErr>>,
    generation: u64,
) -> Result<cpal::Stream, cpal::Error>
where
    T: SizedSample + Send + 'static,
    f32: FromSample<T>,
{
    let ch = cfg.channels as usize;
    dev.build_input_stream::<T, _, _>(
        *cfg,
        move |data: &[T], _: &cpal::InputCallbackInfo| {
            push_capture(&mut tx, data, ch, <f32 as FromSample<T>>::from_sample_, &shared.io)
        },
        move |err| {
            let _ = errors.push(StreamErr {
                dir: Dir::In,
                generation,
                err,
            });
        },
        None,
    )
}

fn build_output<T>(
    dev: &cpal::Device,
    cfg: &cpal::StreamConfig,
    mut rx: rtrb::Consumer<f32>,
    shared: Arc<Shared>,
    errors: Arc<ArrayQueue<StreamErr>>,
    generation: u64,
) -> Result<cpal::Stream, cpal::Error>
where
    T: SizedSample + FromSample<f32> + Send + 'static,
{
    let ch = cfg.channels as usize;
    dev.build_output_stream::<T, _, _>(
        *cfg,
        move |data: &mut [T], _: &cpal::OutputCallbackInfo| {
            pop_playback(&mut rx, data, ch, <T as FromSample<f32>>::from_sample_, &shared.io)
        },
        move |err| {
            let _ = errors.push(StreamErr {
                dir: Dir::Out,
                generation,
                err,
            });
        },
        None,
    )
}

impl System {
    pub(crate) fn start(shared: &Arc<Shared>, input: DeviceChoice, output: DeviceChoice) -> System {
        let host = cpal::default_host();
        let mut s = System {
            host,
            want_in: input,
            want_out: output,
            input: None,
            output: None,
            errors: Arc::new(ArrayQueue::new(64)),
            generation: 0,
            last_poll: Instant::now(),
            list: DeviceList::default(),
        };
        s.refresh_list(shared);
        let _ = s.open(shared, Dir::In);
        let _ = s.open(shared, Dir::Out);
        s
    }

    fn slot(&mut self, dir: Dir) -> &mut Option<Open> {
        match dir {
            Dir::In => &mut self.input,
            Dir::Out => &mut self.output,
        }
    }

    fn want(&self, dir: Dir) -> &DeviceChoice {
        match dir {
            Dir::In => &self.want_in,
            Dir::Out => &self.want_out,
        }
    }

    fn close(&mut self, shared: &Arc<Shared>, dir: Dir) {
        if self.slot(dir).take().is_some() {
            match dir {
                Dir::In => shared.send_ctrl(Ctrl::CaptureIo(None)),
                Dir::Out => shared.send_ctrl(Ctrl::PlayIo(None)),
            };
        }
    }

    fn open(&mut self, shared: &Arc<Shared>, dir: Dir) -> Result<(), AudioError> {
        self.close(shared, dir);
        self.generation += 1;
        let generation = self.generation;
        let r = self.try_open(shared, dir, generation);
        match r {
            Ok(open) => {
                let info = open.info.clone();
                *self.slot(dir) = Some(open);
                shared.set_in_use(|u| match dir {
                    Dir::In => u.input = Some(info),
                    Dir::Out => u.output = Some(info),
                });
                Ok(())
            }
            Err(e) => {
                shared.emit(Event::Error(e.clone()));
                shared.set_in_use(|u| match dir {
                    Dir::In => u.input = None,
                    Dir::Out => u.output = None,
                });
                Err(e)
            }
        }
    }

    fn try_open(&mut self, shared: &Arc<Shared>, dir: Dir, generation: u64) -> Result<Open, AudioError> {
        let choice = self.want(dir).clone();
        let dev = find_device(&self.host, &choice, dir)?;
        let default_id = match dir {
            Dir::In => self.host.default_input_device(),
            Dir::Out => self.host.default_output_device(),
        }
        .and_then(|d| d.id().ok())
        .map(|i| i.to_string());
        let info = device_info(&dev, default_id.as_deref())
            .ok_or_else(|| AudioError::new(ErrorCode::DeviceLost, "device went away"))?;
        let (cfg, fmt) = pick_config(&dev, dir)?;
        let rate = cfg.sample_rate;
        let (tx, rx) = rtrb::RingBuffer::<f32>::new(ring_capacity(rate, shared.playout_buffer_ms));
        let errors = self.errors.clone();
        let stream = match dir {
            Dir::In => {
                let io = CaptureIo::new(rx, rate)?;
                let s = shared.clone();
                let stream = match fmt {
                    SampleFormat::F32 => build_input::<f32>(&dev, &cfg, tx, s, errors, generation),
                    SampleFormat::I16 => build_input::<i16>(&dev, &cfg, tx, s, errors, generation),
                    _ => build_input::<i32>(&dev, &cfg, tx, s, errors, generation),
                }
                .map_err(open_err)?;
                shared.send_ctrl(Ctrl::CaptureIo(Some(Box::new(io))));
                stream
            }
            Dir::Out => {
                let io = PlayIo::new(tx, rate, shared.playout_buffer_ms)?;
                let s = shared.clone();
                let stream = match fmt {
                    SampleFormat::F32 => build_output::<f32>(&dev, &cfg, rx, s, errors, generation),
                    SampleFormat::I16 => build_output::<i16>(&dev, &cfg, rx, s, errors, generation),
                    _ => build_output::<i32>(&dev, &cfg, rx, s, errors, generation),
                }
                .map_err(open_err)?;
                shared.send_ctrl(Ctrl::PlayIo(Some(Box::new(io))));
                stream
            }
        };
        stream.play().map_err(open_err)?;
        Ok(Open {
            _stream: stream,
            info,
            generation,
        })
    }

    pub(crate) fn set_device(&mut self, shared: &Arc<Shared>, sel: DeviceSelection) -> Result<(), AudioError> {
        let mut first_err = None;
        if let Some(c) = sel.input {
            self.want_in = c;
            if let Err(e) = self.open(shared, Dir::In) {
                first_err.get_or_insert(e);
            }
        }
        if let Some(c) = sel.output {
            self.want_out = c;
            if let Err(e) = self.open(shared, Dir::Out) {
                first_err.get_or_insert(e);
            }
        }
        match first_err {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }

    fn refresh_list(&mut self, shared: &Arc<Shared>) -> bool {
        let list = list_devices(&self.host);
        if list == self.list {
            return false;
        }
        self.list = list.clone();
        *lock(&shared.devices) = list.clone();
        shared.emit(Event::Devices(list));
        true
    }

    /// The device went away: report it, fall back to the default if a
    /// specific device was chosen, and reopen.
    fn lost(&mut self, shared: &Arc<Shared>, dir: Dir, detail: String) {
        shared.emit(Event::Error(AudioError::new(ErrorCode::DeviceLost, detail)));
        match dir {
            Dir::In => self.want_in = DeviceChoice::Default,
            Dir::Out => self.want_out = DeviceChoice::Default,
        }
        let _ = self.open(shared, dir);
    }

    pub(crate) fn tick(&mut self, shared: &Arc<Shared>) {
        while let Some(StreamErr {
            dir,
            generation,
            err,
        }) = self.errors.pop()
        {
            let current = match dir {
                Dir::In => self.input.as_ref().map(|o| o.generation),
                Dir::Out => self.output.as_ref().map(|o| o.generation),
            };
            if current != Some(generation) {
                continue;
            }
            match err.kind() {
                ErrorKind::DeviceNotAvailable | ErrorKind::StreamInvalidated => {
                    self.lost(shared, dir, err.to_string())
                }
                ErrorKind::DeviceChanged | ErrorKind::Xrun | ErrorKind::RealtimeDenied => {}
                ErrorKind::DeviceBusy => shared.emit(Event::Error(AudioError::new(
                    ErrorCode::DeviceBusy,
                    err.to_string(),
                ))),
                ErrorKind::PermissionDenied => shared.emit(Event::Error(AudioError::new(
                    ErrorCode::NoPermission,
                    err.to_string(),
                ))),
                _ => shared.emit(Event::Error(AudioError::new(
                    ErrorCode::Backend,
                    err.to_string(),
                ))),
            }
        }

        if shared.device_poll.is_zero() || self.last_poll.elapsed() < shared.device_poll {
            return;
        }
        self.last_poll = Instant::now();
        if !self.refresh_list(shared) {
            return;
        }
        for dir in [Dir::In, Dir::Out] {
            enum Act {
                Lost(String),
                Reopen,
                Nothing,
            }
            let act = {
                let (list, open) = match dir {
                    Dir::In => (&self.list.inputs, &self.input),
                    Dir::Out => (&self.list.outputs, &self.output),
                };
                match (open, self.want(dir)) {
                    // The chosen device disappeared without a stream error.
                    (Some(o), DeviceChoice::Id(_)) if !list.iter().any(|d| d.id == o.info.id) => {
                        Act::Lost(format!("{} was removed", o.info.name))
                    }
                    // Following the default, and the default moved.
                    (Some(o), DeviceChoice::Default)
                        if list.iter().any(|d| d.is_default && d.id != o.info.id) =>
                    {
                        Act::Reopen
                    }
                    // Nothing open (an earlier failure): try again now the
                    // list changed.
                    (None, _) => Act::Reopen,
                    _ => Act::Nothing,
                }
            };
            match act {
                Act::Lost(detail) => self.lost(shared, dir, detail),
                Act::Reopen => {
                    let _ = self.open(shared, dir);
                }
                Act::Nothing => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_errors_map_to_stable_codes() {
        let cases = [
            (ErrorKind::DeviceBusy, ErrorCode::DeviceBusy),
            (ErrorKind::PermissionDenied, ErrorCode::NoPermission),
            (ErrorKind::DeviceNotAvailable, ErrorCode::NoDevice),
            (ErrorKind::HostUnavailable, ErrorCode::NoDevice),
            (ErrorKind::UnsupportedConfig, ErrorCode::UnsupportedConfig),
            (ErrorKind::BackendError, ErrorCode::Backend),
        ];
        for (kind, code) in cases {
            assert_eq!(open_err(cpal::Error::new(kind)).code, code, "{kind:?}");
        }
    }
}
