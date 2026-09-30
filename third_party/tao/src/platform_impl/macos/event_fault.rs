// K2 patch: this whole file is K2's, not upstream tao.
//
// `TaoApp::send_event` and `TaoWindow::send_event` are `extern "C"` IMPs.
// An Objective-C exception, or a Rust panic from a wry `extern "C-unwind"`
// method, can come up out of AppKit's `sendEvent:` into those frames. The
// runtime then aborts with no reason in the crash report (K2 0.41.0 and
// 0.41.4 long-run aborts). `guard_send_event` runs the body of each
// `send_event` so nothing unwinds into the `extern "C"` frame: the event is
// dropped (no retry) and one fault record is reported.
//
// Nesting order: `catch_unwind` is the outer layer and
// `objc2::exception::catch` the inner one.
// - An ObjC exception is caught by the inner `@catch (id)` before any Rust
//   `catch_unwind` frame sees it. Rust cannot catch a foreign exception: a
//   `catch_unwind` that sees one aborts, so the ObjC catch must be closest
//   to the call.
// - A Rust panic is not an ObjC exception. objc2's `@catch (id)` lets it
//   pass through, and the outer `catch_unwind` stops it.
//
// tao stays generic. The embedder installs a hook with
// `set_event_fault_hook` (K2's src-tauri writes the field log). With no
// hook, the record goes to `log::error!`.

use std::{
  any::Any,
  fmt,
  panic::{catch_unwind, AssertUnwindSafe},
  sync::OnceLock,
};

use objc2::{exception::Exception, rc::Retained};
use objc2_app_kit::NSEvent;

/// What unwound out of `sendEvent:`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventFaultKind {
  /// An Objective-C exception (usually an `NSException`).
  ObjcException,
  /// A Rust panic (for example from an `extern "C-unwind"` method).
  Panic,
}

impl fmt::Display for EventFaultKind {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    match self {
      EventFaultKind::ObjcException => f.write_str("objc-exception"),
      EventFaultKind::Panic => f.write_str("panic"),
    }
  }
}

/// One event that tao dropped because handling it raised or panicked.
#[derive(Debug, Clone)]
pub struct EventFault<'a> {
  /// `"TaoApp"` or `"TaoWindow"`.
  pub source: &'static str,
  /// The raw `NSEventType` value.
  pub event_type: usize,
  /// The event's `windowNumber` (0 when the event has no window).
  pub window_number: isize,
  pub kind: EventFaultKind,
  /// Exception name and reason, or the panic payload.
  pub detail: &'a str,
}

impl fmt::Display for EventFault<'_> {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    write!(
      f,
      "{} dropped NSEvent type={} window={} {}: {}",
      self.source, self.event_type, self.window_number, self.kind, self.detail
    )
  }
}

/// A fault hook. It runs on the main thread inside `sendEvent:`. A panic in
/// it is caught and ignored.
pub type EventFaultHook = fn(&EventFault<'_>);

static EVENT_FAULT_HOOK: OnceLock<EventFaultHook> = OnceLock::new();

/// Install the fault hook once. Returns `false` if one was already set.
pub fn set_event_fault_hook(hook: EventFaultHook) -> bool {
  EVENT_FAULT_HOOK.set(hook).is_ok()
}

/// Run a `send_event` body. An ObjC exception or a Rust panic inside `body`
/// is caught, reported once, and the event is dropped.
pub(crate) fn guard_send_event(source: &'static str, event: &NSEvent, body: impl FnOnce()) {
  let outcome = catch_unwind(AssertUnwindSafe(|| {
    objc2::exception::catch(AssertUnwindSafe(body))
  }));
  match outcome {
    Ok(Ok(())) => {}
    Ok(Err(exception)) => {
      report_quietly(|| {
        let detail = describe_exception(exception.as_deref());
        report(source, event, EventFaultKind::ObjcException, &detail);
      });
      release_quietly(exception);
    }
    Err(payload) => {
      report_quietly(|| {
        let detail = describe_panic(&*payload);
        report(source, event, EventFaultKind::Panic, &detail);
      });
      drop_quietly(payload);
    }
  }
}

/// Run the reporting step so that it can never unwind: nothing it does may
/// reach the `extern "C"` frame either.
fn report_quietly(f: impl FnOnce()) {
  let outcome = catch_unwind(AssertUnwindSafe(|| {
    let _ = objc2::exception::catch(AssertUnwindSafe(f));
  }));
  if let Err(payload) = outcome {
    // The report itself panicked. Leak that payload rather than risk a
    // panicking `Drop`.
    std::mem::forget(payload);
  }
}

fn release_quietly(exception: Option<Retained<Exception>>) {
  report_quietly(move || drop(exception));
}

fn drop_quietly(payload: Box<dyn Any + Send>) {
  if let Err(again) = catch_unwind(AssertUnwindSafe(move || drop(payload))) {
    std::mem::forget(again);
  }
}

fn describe_exception(exception: Option<&Exception>) -> String {
  match exception {
    // objc2's Debug prints the object, the NSException name and the reason.
    Some(exception) => format!("{exception:?}"),
    None => "nil exception".to_owned(),
  }
}

fn describe_panic(payload: &(dyn Any + Send)) -> String {
  if let Some(s) = payload.downcast_ref::<&'static str>() {
    (*s).to_owned()
  } else if let Some(s) = payload.downcast_ref::<String>() {
    s.clone()
  } else {
    "non-string panic payload".to_owned()
  }
}

fn report(source: &'static str, event: &NSEvent, kind: EventFaultKind, detail: &str) {
  let fault = EventFault {
    source,
    event_type: event.r#type().0,
    window_number: event.windowNumber(),
    kind,
    detail,
  };
  match EVENT_FAULT_HOOK.get() {
    Some(hook) => hook(&fault),
    None => log::error!("{fault}"),
  }
}

#[cfg(test)]
mod tests {
  //! Every test here fails loudly. If a raise or a panic escapes the guard
  //! into an `extern "C"` IMP, the test process aborts.

  use std::{
    ffi::CStr,
    sync::{
      atomic::{AtomicUsize, Ordering},
      LazyLock, Mutex,
    },
  };

  use objc2::{
    rc::Retained,
    runtime::{AnyClass, AnyObject, ClassBuilder, NSObject, Sel},
    ClassType,
  };
  use objc2_app_kit::{NSEvent, NSEventModifierFlags, NSEventType};
  use objc2_foundation::{NSPoint, NSString};

  use super::*;

  // Mode in `data1` of an ApplicationDefined event, read by the base class.
  const MODE_PASS: isize = 0;
  const MODE_RAISE: isize = 1;
  const MODE_PANIC: isize = 2;

  static RECORDS: Mutex<Vec<String>> = Mutex::new(Vec::new());
  static BASE_CALLS: AtomicUsize = AtomicUsize::new(0);

  fn record(fault: &EventFault<'_>) {
    RECORDS.lock().unwrap().push(fault.to_string());
  }

  fn install_hook() {
    // Set once per test process. Every test uses the same recorder.
    let _ = set_event_fault_hook(record);
    assert_eq!(
      EVENT_FAULT_HOOK.get().copied().map(|h| h as usize),
      Some(record as EventFaultHook as usize),
      "another hook is installed"
    );
  }

  fn records_with(marker: &str) -> Vec<String> {
    RECORDS
      .lock()
      .unwrap()
      .iter()
      .filter(|line| line.contains(marker))
      .cloned()
      .collect()
  }

  fn raise_nsexception(reason: &str) -> ! {
    let name = NSString::from_str("K2TestException");
    let reason = NSString::from_str(reason);
    unsafe {
      let exception: *mut AnyObject = msg_send![
        class!(NSException),
        exceptionWithName: &*name,
        reason: &*reason,
        userInfo: None::<&AnyObject>
      ];
      assert!(!exception.is_null(), "NSException was not created");
      let _: () = msg_send![exception, raise];
    }
    unreachable!("-[NSException raise] returned");
  }

  fn app_defined_event(mode: isize, marker: isize) -> Retained<NSEvent> {
    app_defined_event_in(mode, marker, 7)
  }

  fn app_defined_event_in(mode: isize, marker: isize, window: isize) -> Retained<NSEvent> {
    let event: Option<Retained<NSEvent>> = unsafe {
      msg_send![
        NSEvent::class(),
        otherEventWithType: NSEventType::ApplicationDefined,
        location: NSPoint::new(0.0, 0.0),
        modifierFlags: NSEventModifierFlags(0),
        timestamp: 0.0f64,
        windowNumber: window,
        context: None::<&AnyObject>,
        subtype: 0i16,
        data1: mode,
        data2: marker
      ]
    };
    event.expect("NSEvent otherEventWithType returned nil")
  }

  fn cmd_key_up_event() -> Retained<NSEvent> {
    let chars = NSString::from_str("k");
    let event: Option<Retained<NSEvent>> = unsafe {
      msg_send![
        NSEvent::class(),
        keyEventWithType: NSEventType::KeyUp,
        location: NSPoint::new(0.0, 0.0),
        modifierFlags: NSEventModifierFlags::Command,
        timestamp: 0.0f64,
        windowNumber: 9isize,
        context: None::<&AnyObject>,
        characters: &*chars,
        charactersIgnoringModifiers: &*chars,
        isARepeat: false,
        keyCode: 40u16
      ]
    };
    event.expect("NSEvent keyEventWithType returned nil")
  }

  // The superclass of the test classes. Its `sendEvent:` stands in for
  // AppKit's: it raises, panics through an `extern "C-unwind"` method (as a
  // wry method would), or returns.
  extern "C-unwind" fn base_send_event(_this: &AnyObject, _sel: Sel, event: &NSEvent) {
    BASE_CALLS.fetch_add(1, Ordering::SeqCst);
    if event.r#type() != NSEventType::ApplicationDefined {
      raise_nsexception(&format!("k2-test-key-{}", event.keyCode()));
    }
    let marker = event.data2();
    match event.data1() {
      MODE_RAISE => raise_nsexception(&format!("k2-test-{marker}")),
      MODE_PANIC => panic!("k2-test-{marker}"),
      _ => {}
    }
  }

  static KEY_WINDOW: LazyLock<usize> = LazyLock::new(|| {
    let obj: Retained<AnyObject> = unsafe { msg_send![base_class(), new] };
    // Leaked on purpose: it lives for the whole test process.
    Retained::into_raw(obj) as usize
  });

  // Stands in for `-[NSApplication keyWindow]` for the Cmd+KeyUp branch.
  // Returns a +0 object that is never freed.
  extern "C-unwind" fn base_key_window(_this: &AnyObject, _sel: Sel) -> *mut AnyObject {
    *KEY_WINDOW as *mut AnyObject
  }

  struct ClassRef(&'static AnyClass);
  unsafe impl Send for ClassRef {}
  unsafe impl Sync for ClassRef {}

  static BASE_CLASS: LazyLock<ClassRef> = LazyLock::new(|| unsafe {
    let name = CStr::from_bytes_with_nul(b"K2EventFaultTestBase\0").unwrap();
    let mut decl = ClassBuilder::new(name, NSObject::class()).expect("class name taken");
    decl.add_method(
      sel!(sendEvent:),
      base_send_event as extern "C-unwind" fn(_, _, _),
    );
    decl.add_method(
      sel!(keyWindow),
      base_key_window as extern "C-unwind" fn(_, _) -> _,
    );
    ClassRef(decl.register())
  });

  fn base_class() -> &'static AnyClass {
    BASE_CLASS.0
  }

  // A class whose `sendEvent:` IMP is the real TaoApp `send_event`. Its
  // superclass is the throwing base, so `super sendEvent:` reaches it.
  static TEST_APP_CLASS: LazyLock<ClassRef> = LazyLock::new(|| unsafe {
    let name = CStr::from_bytes_with_nul(b"K2EventFaultTestApp\0").unwrap();
    let mut decl = ClassBuilder::new(name, base_class()).expect("class name taken");
    decl.add_method(
      sel!(sendEvent:),
      super::super::app::send_event as extern "C" fn(_, _, _),
    );
    ClassRef(decl.register())
  });

  // The same for the real TaoWindow `send_event`.
  static TEST_WINDOW_CLASS: LazyLock<ClassRef> = LazyLock::new(|| unsafe {
    let name = CStr::from_bytes_with_nul(b"K2EventFaultTestWindow\0").unwrap();
    let mut decl = ClassBuilder::new(name, base_class()).expect("class name taken");
    decl.add_method(
      sel!(sendEvent:),
      super::super::window::send_event as extern "C" fn(_, _, _),
    );
    ClassRef(decl.register())
  });

  fn new_instance(class: &'static AnyClass) -> Retained<AnyObject> {
    unsafe { msg_send![class, new] }
  }

  fn send(obj: &AnyObject, event: &NSEvent) {
    unsafe {
      let _: () = msg_send![obj, sendEvent: event];
    }
  }

  #[test]
  fn guard_catches_thrown_objc_exception_and_reports_once() {
    install_hook();
    let event = app_defined_event(MODE_PASS, 0);
    let mut ran_after_throw = false;
    guard_send_event("TaoWindow", &event, || {
      raise_nsexception("k2-test-guard-raise");
      #[allow(unreachable_code)]
      {
        ran_after_throw = true;
      }
    });
    assert!(!ran_after_throw);
    let lines = records_with("k2-test-guard-raise");
    assert_eq!(lines.len(), 1, "records: {lines:?}");
    let line = &lines[0];
    assert!(
      line.starts_with("TaoWindow dropped NSEvent type=15 window=7 objc-exception: "),
      "{line}"
    );
    assert!(
      line.contains("K2TestException"),
      "exception name missing: {line}"
    );
  }

  #[test]
  fn guard_catches_objc2_throw() {
    install_hook();
    let event = app_defined_event(MODE_PASS, 0);
    guard_send_event("TaoApp", &event, || {
      let obj = NSObject::new();
      let exception: Retained<Exception> = unsafe { Retained::cast_unchecked(obj) };
      objc2::exception::throw(exception);
    });
    let lines: Vec<String> = RECORDS
      .lock()
      .unwrap()
      .iter()
      .filter(|l| l.starts_with("TaoApp") && l.contains("exception <NSObject"))
      .cloned()
      .collect();
    assert_eq!(lines.len(), 1, "records: {lines:?}");
  }

  #[test]
  fn guard_catches_panic_and_reports_payload() {
    install_hook();
    let event = app_defined_event(MODE_PASS, 0);
    guard_send_event("TaoApp", &event, || panic!("k2-test-guard-panic {}", 42));
    let lines = records_with("k2-test-guard-panic 42");
    assert_eq!(lines.len(), 1, "records: {lines:?}");
    assert!(
      lines[0].starts_with("TaoApp dropped NSEvent type=15 window=7 panic: "),
      "{}",
      lines[0]
    );
  }

  #[test]
  fn guard_passes_a_clean_body_through_without_a_record() {
    install_hook();
    let event = app_defined_event_in(MODE_PASS, 0, 4201);
    let mut ran = false;
    guard_send_event("TaoApp", &event, || ran = true);
    assert!(ran);
    assert_eq!(records_with("window=4201").len(), 0);
  }

  #[test]
  fn tao_window_send_event_survives_super_raise() {
    install_hook();
    let window = new_instance(TEST_WINDOW_CLASS.0);
    send(&window, &app_defined_event(MODE_RAISE, 1101));
    let lines = records_with("k2-test-1101");
    assert_eq!(lines.len(), 1, "records: {lines:?}");
    assert!(
      lines[0].starts_with("TaoWindow dropped NSEvent type=15 window=7 objc-exception: "),
      "{}",
      lines[0]
    );
  }

  #[test]
  fn tao_window_send_event_survives_c_unwind_panic() {
    install_hook();
    let window = new_instance(TEST_WINDOW_CLASS.0);
    send(&window, &app_defined_event(MODE_PANIC, 1102));
    let lines = records_with("k2-test-1102");
    assert_eq!(lines.len(), 1, "records: {lines:?}");
    assert!(
      lines[0].starts_with("TaoWindow dropped NSEvent type=15 window=7 panic: "),
      "{}",
      lines[0]
    );
  }

  #[test]
  fn tao_window_send_event_forwards_a_clean_event() {
    install_hook();
    let window = new_instance(TEST_WINDOW_CLASS.0);
    let before = BASE_CALLS.load(Ordering::SeqCst);
    send(&window, &app_defined_event_in(MODE_PASS, 1103, 4202));
    assert!(
      BASE_CALLS.load(Ordering::SeqCst) > before,
      "super sendEvent: was not called"
    );
    assert_eq!(records_with("window=4202").len(), 0);
  }

  #[test]
  fn tao_app_send_event_survives_super_raise() {
    install_hook();
    let app = new_instance(TEST_APP_CLASS.0);
    send(&app, &app_defined_event(MODE_RAISE, 1201));
    let lines = records_with("k2-test-1201");
    assert_eq!(lines.len(), 1, "records: {lines:?}");
    assert!(
      lines[0].starts_with("TaoApp dropped NSEvent type=15 window=7 objc-exception: "),
      "{}",
      lines[0]
    );
  }

  #[test]
  fn tao_app_send_event_survives_c_unwind_panic() {
    install_hook();
    let app = new_instance(TEST_APP_CLASS.0);
    send(&app, &app_defined_event(MODE_PANIC, 1202));
    let lines = records_with("k2-test-1202");
    assert_eq!(lines.len(), 1, "records: {lines:?}");
    assert!(
      lines[0].starts_with("TaoApp dropped NSEvent type=15 window=7 panic: "),
      "{}",
      lines[0]
    );
  }

  #[test]
  fn tao_app_send_event_survives_key_window_raise_on_cmd_key_up() {
    install_hook();
    let app = new_instance(TEST_APP_CLASS.0);
    send(&app, &cmd_key_up_event());
    let lines = records_with("k2-test-key-40");
    assert_eq!(lines.len(), 1, "records: {lines:?}");
    // NSEventType::KeyUp is 11.
    assert!(
      lines[0].starts_with("TaoApp dropped NSEvent type=11 window=9 objc-exception: "),
      "{}",
      lines[0]
    );
  }
}
