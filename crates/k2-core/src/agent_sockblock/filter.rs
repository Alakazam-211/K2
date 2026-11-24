//! The session's seccomp filter, classic BPF built here (review section 2,
//! "the filter, complete"):
//! 1. not the native architecture (or an x32 number on x86-64): refused;
//! 2. `io_uring_setup`: `ENOSYS`;
//! 3. `seccomp()` with the new-listener flag: refused (`EPERM`);
//! 4. `socket()` / `socketpair()` with `AF_UNIX` and any type but stream
//!    or seq-packet (type flags masked off): refused (`EPERM`). Datagram,
//!    and raw too: the kernel makes a raw Unix socket a datagram one
//!    (code review K1);
//! 5. `connect()`: user notification (the supervisor decides);
//! 6. everything else: allowed.
//!
//! In record mode rules 2–4 notify instead of refusing: the supervisor logs
//! what enforce would refuse and lets the call run. Rule 1's calls run
//! without the block in record mode (a 32-bit program would otherwise
//! notify on every call; code review K2): the supervisor says once, at
//! start, that they aren't covered.
//!
//! [`run`] is a small interpreter for the instructions this module emits,
//! so the tests check decisions, not only the instruction list.

use super::Mode;

/// One classic BPF instruction (`struct sock_filter`).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SockFilter {
    pub code: u16,
    pub jt: u8,
    pub jf: u8,
    pub k: u32,
}

/// `struct seccomp_data`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SeccompData {
    pub nr: i32,
    pub arch: u32,
    pub instruction_pointer: u64,
    pub args: [u64; 6],
}

// Opcodes (linux/bpf_common.h).
/// `BPF_LD | BPF_W | BPF_ABS`.
pub const LD_W_ABS: u16 = 0x20;
/// `BPF_JMP | BPF_JEQ | BPF_K`.
pub const JMP_JEQ_K: u16 = 0x15;
/// `BPF_JMP | BPF_JGE | BPF_K`.
pub const JMP_JGE_K: u16 = 0x35;
/// `BPF_JMP | BPF_JSET | BPF_K`.
pub const JMP_JSET_K: u16 = 0x45;
/// `BPF_ALU | BPF_AND | BPF_K`.
pub const ALU_AND_K: u16 = 0x54;
/// `BPF_RET | BPF_K`.
pub const RET_K: u16 = 0x06;

// Return values (linux/seccomp.h).
pub const RET_ERRNO: u32 = 0x0005_0000;
pub const RET_USER_NOTIF: u32 = 0x7fc0_0000;
pub const RET_ALLOW: u32 = 0x7fff_0000;
pub const RET_DATA: u32 = 0x0000_ffff;

/// `seccomp()` flags.
pub const FILTER_FLAG_NEW_LISTENER: u32 = 1 << 3;
pub const FILTER_FLAG_WAIT_KILLABLE_RECV: u32 = 1 << 5;
/// x86-64's x32 call numbers carry this bit.
pub const X32_SYSCALL_BIT: u32 = 0x4000_0000;

pub const AF_UNIX: u32 = 1;
pub const SOCK_DGRAM: u32 = 2;
pub const SOCK_STREAM: u32 = 1;
pub const SOCK_SEQPACKET: u32 = 5;
/// `SOCK_TYPE_MASK`: the type without `SOCK_NONBLOCK` / `SOCK_CLOEXEC`.
pub const SOCK_TYPE_MASK: u32 = 0xf;

pub const ENOSYS: u32 = 38;
pub const EPERM: u32 = 1;

/// The architectures the block runs on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Arch {
    X86_64,
    Aarch64,
}

/// The call numbers the filter names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Nrs {
    pub socket: u32,
    pub socketpair: u32,
    pub connect: u32,
    pub seccomp: u32,
    pub io_uring_setup: u32,
}

impl Arch {
    /// `AUDIT_ARCH_*`.
    pub const fn audit(self) -> u32 {
        match self {
            Arch::X86_64 => 0xC000_003E,
            Arch::Aarch64 => 0xC000_00B7,
        }
    }

    pub const fn nrs(self) -> Nrs {
        match self {
            Arch::X86_64 => Nrs { socket: 41, socketpair: 53, connect: 42, seccomp: 317, io_uring_setup: 425 },
            Arch::Aarch64 => Nrs { socket: 198, socketpair: 199, connect: 203, seccomp: 277, io_uring_setup: 425 },
        }
    }

    /// This build's architecture, when the block supports it.
    pub const fn native() -> Option<Arch> {
        if cfg!(target_arch = "x86_64") {
            Some(Arch::X86_64)
        } else if cfg!(target_arch = "aarch64") {
            Some(Arch::Aarch64)
        } else {
            None
        }
    }

    /// A call from another architecture's entry (32-bit), or an x32 number.
    pub fn foreign(self, d: &SeccompData) -> bool {
        d.arch != self.audit() || (self == Arch::X86_64 && (d.nr as u32) >= X32_SYSCALL_BIT)
    }
}

/// Offsets into `seccomp_data` (little-endian: an argument's low word first).
const OFF_NR: u32 = 0;
const OFF_ARCH: u32 = 4;
const fn arg_lo(i: u32) -> u32 {
    16 + 8 * i
}

#[derive(Clone, Copy)]
enum To {
    Next,
    L(&'static str),
}

enum Ins {
    Ld(u32),
    J(u16, u32, To, To),
    And(u32),
    Ret(u32),
    Mark(&'static str),
}

fn assemble(ins: &[Ins]) -> Vec<SockFilter> {
    let mut at = std::collections::HashMap::new();
    let mut pos = 0usize;
    for i in ins {
        match i {
            Ins::Mark(m) => {
                at.insert(*m, pos);
            }
            _ => pos += 1,
        }
    }
    let mut out = Vec::with_capacity(pos);
    for i in ins {
        let here = out.len();
        let off = |t: &To| -> u8 {
            let target = match t {
                To::Next => here + 1,
                To::L(m) => *at.get(m).unwrap_or_else(|| panic!("filter label {m} is missing")),
            };
            assert!(target > here, "filter jumps only forward");
            u8::try_from(target - here - 1).expect("filter jump fits in a byte")
        };
        match i {
            Ins::Mark(_) => {}
            Ins::Ld(k) => out.push(SockFilter { code: LD_W_ABS, jt: 0, jf: 0, k: *k }),
            Ins::J(code, k, t, f) => out.push(SockFilter { code: *code, jt: off(t), jf: off(f), k: *k }),
            Ins::And(k) => out.push(SockFilter { code: ALU_AND_K, jt: 0, jf: 0, k: *k }),
            Ins::Ret(k) => out.push(SockFilter { code: RET_K, jt: 0, jf: 0, k: *k }),
        }
    }
    out
}

/// The filter for `arch` in `mode`.
pub fn program(arch: Arch, mode: Mode) -> Vec<SockFilter> {
    let n = arch.nrs();
    let refuse = |errno: u32| match mode {
        Mode::Enforce => RET_ERRNO | (errno & RET_DATA),
        Mode::Record => RET_USER_NOTIF,
    };
    let foreign = match mode {
        Mode::Enforce => RET_ERRNO | ENOSYS,
        Mode::Record => RET_ALLOW,
    };
    let mut p = vec![
        Ins::Ld(OFF_ARCH),
        Ins::J(JMP_JEQ_K, arch.audit(), To::Next, To::L("foreign")),
        Ins::Ld(OFF_NR),
    ];
    if arch == Arch::X86_64 {
        p.push(Ins::J(JMP_JGE_K, X32_SYSCALL_BIT, To::L("foreign"), To::Next));
    }
    p.extend([
        Ins::J(JMP_JEQ_K, n.connect, To::L("notify"), To::Next),
        Ins::J(JMP_JEQ_K, n.io_uring_setup, To::L("uring"), To::Next),
        Ins::J(JMP_JEQ_K, n.seccomp, To::L("seccomp"), To::Next),
        Ins::J(JMP_JEQ_K, n.socket, To::L("socket"), To::Next),
        Ins::J(JMP_JEQ_K, n.socketpair, To::L("socket"), To::Next),
        Ins::Ret(RET_ALLOW),
        Ins::Mark("socket"),
        Ins::Ld(arg_lo(0)),
        Ins::J(JMP_JEQ_K, AF_UNIX, To::Next, To::L("allow")),
        Ins::Ld(arg_lo(1)),
        Ins::And(SOCK_TYPE_MASK),
        Ins::J(JMP_JEQ_K, SOCK_STREAM, To::L("allow"), To::Next),
        Ins::J(JMP_JEQ_K, SOCK_SEQPACKET, To::L("allow"), To::L("unixtype")),
        Ins::Mark("seccomp"),
        Ins::Ld(arg_lo(1)),
        Ins::J(JMP_JSET_K, FILTER_FLAG_NEW_LISTENER, To::L("listener"), To::L("allow")),
        Ins::Mark("allow"),
        Ins::Ret(RET_ALLOW),
        Ins::Mark("notify"),
        Ins::Ret(RET_USER_NOTIF),
        Ins::Mark("uring"),
        Ins::Ret(refuse(ENOSYS)),
        Ins::Mark("unixtype"),
        Ins::Ret(refuse(EPERM)),
        Ins::Mark("listener"),
        Ins::Ret(refuse(EPERM)),
        Ins::Mark("foreign"),
        Ins::Ret(foreign),
    ]);
    assemble(&p)
}

fn word(d: &SeccompData, off: u32) -> u32 {
    match off {
        0 => d.nr as u32,
        4 => d.arch,
        8 => d.instruction_pointer as u32,
        12 => (d.instruction_pointer >> 32) as u32,
        16..=63 if off.is_multiple_of(4) => {
            let a = d.args[((off - 16) / 8) as usize];
            if (off - 16).is_multiple_of(8) {
                a as u32
            } else {
                (a >> 32) as u32
            }
        }
        _ => panic!("load outside seccomp_data at {off}"),
    }
}

/// Run `prog` on `d` the way the kernel would, for the instructions
/// [`program`] emits. Panics on anything else or a jump out of the program.
pub fn run(prog: &[SockFilter], d: &SeccompData) -> u32 {
    let mut a: u32 = 0;
    let mut pc = 0usize;
    loop {
        let i = prog.get(pc).unwrap_or_else(|| panic!("the filter ran off its end at {pc}"));
        let jump = |cond: bool| pc + 1 + usize::from(if cond { i.jt } else { i.jf });
        pc = match i.code {
            LD_W_ABS => {
                a = word(d, i.k);
                pc + 1
            }
            JMP_JEQ_K => jump(a == i.k),
            JMP_JGE_K => jump(a >= i.k),
            JMP_JSET_K => jump(a & i.k != 0),
            ALU_AND_K => {
                a &= i.k;
                pc + 1
            }
            RET_K => return i.k,
            c => panic!("opcode {c:#x} at {pc} is not one this filter uses"),
        };
    }
}
