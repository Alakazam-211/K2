//! A Job Object around one worker process (k2-export, k2-deckrender,
//! k2-scenerender): what `RLIMIT_AS`, `RLIMIT_CPU`, no children and
//! "dies with its host" mean on Windows.
//!
//! - **Memory:** `JOB_OBJECT_LIMIT_PROCESS_MEMORY` caps the process's
//!   committed memory, native allocations included (the worker's counting
//!   allocator only sees Rust's). A commit past it fails, so the worker
//!   aborts or its decoder reports out of memory; Windows posts the job a
//!   memory-limit message, and [`WorkerJob::hit_memory`] tells the host.
//! - **CPU:** `JOB_OBJECT_LIMIT_PROCESS_TIME`: past its user-mode CPU time
//!   Windows ends the process and posts an end-of-time message;
//!   [`WorkerJob::hit_cpu`] says so.
//! - **No children:** `JOB_OBJECT_LIMIT_ACTIVE_PROCESS` = 1, so a
//!   `CreateProcess` from the worker fails.
//! - **Dies with the host:** `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`: dropping
//!   the [`WorkerJob`] (or the host dying, which closes its handle) ends it.
//! - **No crash dialog:** `JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION`.
//!
//! The process is put in the job right after it starts. That is before the
//! host writes a byte of its input, so nothing hostile runs outside the job:
//! only the worker's own start-up does.
//!
//! The network is NOT limited here (a Job Object can't): see the hosts'
//! limit tables.

use std::io;
use std::os::windows::io::AsRawHandle;
use std::process::Child;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::System::IO::{CreateIoCompletionPort, GetQueuedCompletionStatus, OVERLAPPED};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JobObjectAssociateCompletionPortInformation,
    JobObjectBasicAccountingInformation, JobObjectExtendedLimitInformation, QueryInformationJobObject,
    SetInformationJobObject, JOBOBJECT_ASSOCIATE_COMPLETION_PORT, JOBOBJECT_BASIC_ACCOUNTING_INFORMATION,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_ACTIVE_PROCESS, JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION,
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOB_OBJECT_LIMIT_PROCESS_MEMORY, JOB_OBJECT_LIMIT_PROCESS_TIME,
};

/// The job messages read here (winnt.h; windows-sys keeps them elsewhere).
const JOB_OBJECT_MSG_END_OF_PROCESS_TIME: u32 = 1;
const JOB_OBJECT_MSG_PROCESS_MEMORY_LIMIT: u32 = 9;
const JOB_OBJECT_MSG_ACTIVE_PROCESS_ZERO: u32 = 4;

/// What a worker's job holds it to. `0` / zero duration: no such limit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JobLimits {
    /// Committed memory of the process, bytes.
    pub process_memory: u64,
    /// User-mode CPU time of the process.
    pub cpu: Duration,
}

/// The job one worker runs in. Dropping it ends the worker.
pub struct WorkerJob {
    handle: HANDLE,
    /// The completion port Windows posts the job's limit messages to.
    port: HANDLE,
    limits: JobLimits,
    memory_hit: AtomicBool,
    cpu_hit: AtomicBool,
}

// SAFETY: a job handle may be used from any thread.
unsafe impl Send for WorkerJob {}
unsafe impl Sync for WorkerJob {}

/// 100-ns ticks, as Windows counts CPU time.
fn ticks(d: Duration) -> i64 {
    i64::try_from(d.as_nanos() / 100).unwrap_or(i64::MAX)
}

impl WorkerJob {
    /// A new job with `limits`, no children, kill-on-close and no crash
    /// dialog; `child` goes in it. On an error the caller kills the child.
    pub fn contain(child: &Child, limits: JobLimits) -> io::Result<WorkerJob> {
        // SAFETY: no security attributes, no name.
        let handle = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if handle.is_null() {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: a new port, not tied to a file.
        let port = unsafe { CreateIoCompletionPort(INVALID_HANDLE_VALUE, std::ptr::null_mut(), 0, 1) };
        if port.is_null() {
            let e = io::Error::last_os_error();
            // SAFETY: our handle.
            unsafe { CloseHandle(handle) };
            return Err(e);
        }
        let job = WorkerJob { handle, port, limits, memory_hit: AtomicBool::new(false), cpu_hit: AtomicBool::new(false) };
        let assoc = JOBOBJECT_ASSOCIATE_COMPLETION_PORT { CompletionKey: handle, CompletionPort: port };
        // SAFETY: `assoc` is the struct the class names, with its size.
        let ok = unsafe {
            SetInformationJobObject(
                handle,
                JobObjectAssociateCompletionPortInformation,
                (&assoc as *const JOBOBJECT_ASSOCIATE_COMPLETION_PORT).cast(),
                std::mem::size_of::<JOBOBJECT_ASSOCIATE_COMPLETION_PORT>() as u32,
            )
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: plain data, zero is "no limit" for every field we don't set.
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
        let mut flags = JOB_OBJECT_LIMIT_ACTIVE_PROCESS | JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION;
        info.BasicLimitInformation.ActiveProcessLimit = 1;
        if limits.process_memory > 0 {
            flags |= JOB_OBJECT_LIMIT_PROCESS_MEMORY;
            info.ProcessMemoryLimit = usize::try_from(limits.process_memory).unwrap_or(usize::MAX);
        }
        if !limits.cpu.is_zero() {
            flags |= JOB_OBJECT_LIMIT_PROCESS_TIME;
            info.BasicLimitInformation.PerProcessUserTimeLimit = ticks(limits.cpu);
        }
        info.BasicLimitInformation.LimitFlags = flags;
        // SAFETY: `info` is the struct the class names, with its size.
        let ok = unsafe {
            SetInformationJobObject(
                handle,
                JobObjectExtendedLimitInformation,
                (&info as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: the child's process handle, alive while `child` is.
        if unsafe { AssignProcessToJobObject(handle, child.as_raw_handle() as HANDLE) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(job)
    }

    /// The limits this job holds its worker to.
    pub fn limits(&self) -> JobLimits {
        self.limits
    }

    /// The most committed memory any process of the job has had, bytes.
    pub fn peak_process_memory(&self) -> u64 {
        // SAFETY: plain data, filled by the query.
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
        // SAFETY: `info` is the struct the class names, with its size.
        let ok = unsafe {
            QueryInformationJobObject(
                self.handle,
                JobObjectExtendedLimitInformation,
                (&mut info as *mut JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 {
            return 0;
        }
        info.PeakProcessMemoryUsed as u64
    }

    /// The user-mode CPU time the job's processes have used.
    pub fn user_time(&self) -> Duration {
        // SAFETY: plain data, filled by the query.
        let mut info: JOBOBJECT_BASIC_ACCOUNTING_INFORMATION = unsafe { std::mem::zeroed() };
        // SAFETY: `info` is the struct the class names, with its size.
        let ok = unsafe {
            QueryInformationJobObject(
                self.handle,
                JobObjectBasicAccountingInformation,
                (&mut info as *mut JOBOBJECT_BASIC_ACCOUNTING_INFORMATION).cast(),
                std::mem::size_of::<JOBOBJECT_BASIC_ACCOUNTING_INFORMATION>() as u32,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 {
            return Duration::ZERO;
        }
        Duration::from_nanos(u64::try_from(info.TotalUserTime).unwrap_or(0).saturating_mul(100))
    }

    /// Read the messages Windows has posted so far (never waits).
    fn drain(&self) {
        loop {
            let mut msg: u32 = 0;
            let mut key: usize = 0;
            let mut ov: *mut OVERLAPPED = std::ptr::null_mut();
            // SAFETY: our port; out-pointers to locals; zero timeout.
            let ok = unsafe { GetQueuedCompletionStatus(self.port, &mut msg, &mut key, &mut ov, 0) };
            if ok == 0 {
                return;
            }
            match msg {
                JOB_OBJECT_MSG_PROCESS_MEMORY_LIMIT => self.memory_hit.store(true, Ordering::Release),
                JOB_OBJECT_MSG_END_OF_PROCESS_TIME => self.cpu_hit.store(true, Ordering::Release),
                _ => {}
            }
        }
    }

    /// After the worker has exited: wait (at most 500 ms) for the job's last
    /// message, so every limit message it posted has been read.
    pub fn settle(&self) {
        let deadline = std::time::Instant::now() + Duration::from_millis(500);
        loop {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            if left.is_zero() {
                return;
            }
            let mut msg: u32 = 0;
            let mut key: usize = 0;
            let mut ov: *mut OVERLAPPED = std::ptr::null_mut();
            // SAFETY: our port; out-pointers to locals.
            let ok = unsafe { GetQueuedCompletionStatus(self.port, &mut msg, &mut key, &mut ov, left.as_millis() as u32) };
            if ok == 0 {
                return;
            }
            match msg {
                JOB_OBJECT_MSG_PROCESS_MEMORY_LIMIT => self.memory_hit.store(true, Ordering::Release),
                JOB_OBJECT_MSG_END_OF_PROCESS_TIME => self.cpu_hit.store(true, Ordering::Release),
                JOB_OBJECT_MSG_ACTIVE_PROCESS_ZERO => return,
                _ => {}
            }
        }
    }

    /// The worker asked for memory past its cap (Windows refused the commit).
    pub fn hit_memory(&self) -> bool {
        self.drain();
        self.memory_hit.load(Ordering::Acquire)
    }

    /// The worker used up its CPU time (Windows ended it there). The job's
    /// message can land after the process is gone, so its CPU time counts too.
    pub fn hit_cpu(&self) -> bool {
        self.drain();
        self.cpu_hit.load(Ordering::Acquire) || (!self.limits.cpu.is_zero() && self.user_time() >= self.limits.cpu)
    }
}

impl Drop for WorkerJob {
    fn drop(&mut self) {
        // SAFETY: our handles; KILL_ON_JOB_CLOSE ends anything still in the job.
        unsafe {
            CloseHandle(self.handle);
            CloseHandle(self.port);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::{Command, Stdio};

    fn powershell(script: &str) -> Command {
        let mut c = Command::new("powershell.exe");
        c.args(["-NoProfile", "-NonInteractive", "-Command", script]).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
        c
    }

    #[test]
    fn a_contained_process_cannot_start_a_child() {
        // Waits for its input, then tries to start cmd.exe.
        let mut child = powershell("[Console]::In.ReadLine() | Out-Null; try { Start-Process cmd.exe -ArgumentList '/c','exit 0' -Wait -ErrorAction Stop; 'started' } catch { 'refused' }")
            .stdin(Stdio::piped())
            .spawn()
            .expect("powershell");
        let _job = WorkerJob::contain(&child, JobLimits { process_memory: 0, cpu: Duration::ZERO }).expect("contain");
        use std::io::Write;
        child.stdin.take().expect("stdin").write_all(b"go\n").expect("write");
        let out = child.wait_with_output().expect("wait");
        let said = String::from_utf8_lossy(&out.stdout);
        assert!(said.trim().ends_with("refused"), "the child must be refused: {said} / {}", String::from_utf8_lossy(&out.stderr));
    }

    #[test]
    fn dropping_the_job_ends_the_process() {
        let mut child = powershell("Start-Sleep -Seconds 600").spawn().expect("powershell");
        let job = WorkerJob::contain(&child, JobLimits { process_memory: 0, cpu: Duration::ZERO }).expect("contain");
        drop(job);
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            if child.try_wait().expect("try_wait").is_some() {
                break;
            }
            assert!(std::time::Instant::now() < deadline, "the process outlived its job");
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    #[test]
    fn a_spinning_process_is_ended_at_its_cpu_time() {
        let mut child = powershell("[Console]::In.ReadLine() | Out-Null; $x = 0; while ($true) { $x++ }").stdin(Stdio::piped()).spawn().expect("powershell");
        let job = WorkerJob::contain(&child, JobLimits { process_memory: 0, cpu: Duration::from_secs(2) }).expect("contain");
        use std::io::Write;
        child.stdin.take().expect("stdin").write_all(b"go\n").expect("write");
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        loop {
            if child.try_wait().expect("try_wait").is_some() {
                break;
            }
            assert!(std::time::Instant::now() < deadline, "the spin outlived its CPU time");
            std::thread::sleep(Duration::from_millis(100));
        }
        assert!(job.hit_cpu(), "the job reports its CPU time: used {:?}", job.user_time());
    }

    #[test]
    fn a_process_past_its_memory_cap_is_stopped_and_the_job_says_so() {
        // 64 MiB blocks until a commit fails.
        let cap: u64 = 256 << 20;
        let mut child = powershell("[Console]::In.ReadLine() | Out-Null; $keep = New-Object System.Collections.ArrayList; try { while ($true) { [void]$keep.Add((New-Object byte[] (64MB))) } } catch { 'out of memory'; exit 7 }")
            .stdin(Stdio::piped())
            .spawn()
            .expect("powershell");
        let job = WorkerJob::contain(&child, JobLimits { process_memory: cap, cpu: Duration::ZERO }).expect("contain");
        use std::io::Write;
        child.stdin.take().expect("stdin").write_all(b"go\n").expect("write");
        let out = child.wait_with_output().expect("wait");
        assert!(!out.status.success(), "the allocations past the cap must fail: {out:?}");
        assert!(job.hit_memory(), "the job reports its cap (peak {} of {cap})", job.peak_process_memory());
        assert!(job.peak_process_memory() <= cap, "never past the cap: peak {}", job.peak_process_memory());
    }
}
