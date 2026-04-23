//! Platform-specific memory allocation and setjmp/longjmp.
//!
//! Provides RWX executable memory and a portable
//! setjmp/longjmp pair for the JIT overflow-abort path.
//! All abstractions are pure Rust — no C source files.

// ── Memory allocation ─────────────────────────────────────────────

/// Allocate a region of read-write-execute memory.
///
/// Returns a raw pointer on success.
///
/// # Safety
/// The returned pointer must be freed with [`free_rwx`] using the
/// same `size`.
#[cfg(unix)]
pub unsafe fn alloc_rwx(size: usize) -> std::io::Result<*mut u8> {
    let ptr = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            size,
            libc::PROT_READ | libc::PROT_WRITE | libc::PROT_EXEC,
            libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
            -1,
            0,
        )
    };
    if ptr == libc::MAP_FAILED {
        return Err(std::io::Error::last_os_error());
    }
    Ok(ptr as *mut u8)
}

/// Free memory allocated by [`alloc_rwx`].
///
/// # Safety
/// `ptr` and `size` must match a prior [`alloc_rwx`] call.
#[cfg(unix)]
pub unsafe fn free_rwx(ptr: *mut u8, size: usize) {
    unsafe { libc::munmap(ptr as *mut libc::c_void, size) };
}

/// Change permissions to read-execute only.
#[cfg(unix)]
pub fn set_rx(ptr: *mut u8, size: usize) -> std::io::Result<()> {
    let ret = unsafe {
        libc::mprotect(
            ptr as *mut libc::c_void,
            size,
            libc::PROT_READ | libc::PROT_EXEC,
        )
    };
    if ret != 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

/// Change permissions to read-write only.
#[cfg(unix)]
pub fn set_rw(ptr: *mut u8, size: usize) -> std::io::Result<()> {
    let ret = unsafe {
        libc::mprotect(
            ptr as *mut libc::c_void,
            size,
            libc::PROT_READ | libc::PROT_WRITE,
        )
    };
    if ret != 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

/// Return the OS page size.
#[cfg(unix)]
pub fn page_size() -> usize {
    // SAFETY: sysconf is always safe to call.
    let sz = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    if sz <= 0 {
        4096
    } else {
        sz as usize
    }
}

/// # Safety
/// The returned pointer must be freed with [`free_rwx`] using the
/// same `size`.
#[cfg(windows)]
pub unsafe fn alloc_rwx(size: usize) -> std::io::Result<*mut u8> {
    use windows_sys::Win32::System::Memory::{
        VirtualAlloc, MEM_COMMIT, MEM_RESERVE, PAGE_EXECUTE_READWRITE,
    };
    let ptr = unsafe {
        VirtualAlloc(
            std::ptr::null(),
            size,
            MEM_COMMIT | MEM_RESERVE,
            PAGE_EXECUTE_READWRITE,
        )
    };
    if ptr.is_null() {
        return Err(std::io::Error::last_os_error());
    }
    Ok(ptr as *mut u8)
}

/// # Safety
/// `ptr` must match a prior [`alloc_rwx`] call.
#[cfg(windows)]
pub unsafe fn free_rwx(ptr: *mut u8, _size: usize) {
    use windows_sys::Win32::System::Memory::{VirtualFree, MEM_RELEASE};
    unsafe { VirtualFree(ptr as *mut _, 0, MEM_RELEASE) };
}

#[cfg(windows)]
pub fn set_rx(ptr: *mut u8, size: usize) -> std::io::Result<()> {
    use windows_sys::Win32::System::Memory::{
        VirtualProtect, PAGE_EXECUTE_READ,
    };
    let mut old = 0u32;
    let ok = unsafe {
        VirtualProtect(ptr as *mut _, size, PAGE_EXECUTE_READ, &mut old)
    };
    if ok == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(windows)]
pub fn set_rw(ptr: *mut u8, size: usize) -> std::io::Result<()> {
    use windows_sys::Win32::System::Memory::{VirtualProtect, PAGE_READWRITE};
    let mut old = 0u32;
    let ok = unsafe {
        VirtualProtect(ptr as *mut _, size, PAGE_READWRITE, &mut old)
    };
    if ok == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(windows)]
pub fn page_size() -> usize {
    use windows_sys::Win32::System::SystemInformation::{
        GetSystemInfo, SYSTEM_INFO,
    };
    let mut info: SYSTEM_INFO = unsafe { std::mem::zeroed() };
    unsafe { GetSystemInfo(&mut info) };
    info.dwPageSize as usize
}

// ── setjmp / longjmp ─────────────────────────────────────────────

/// Opaque buffer for [`do_setjmp`] / [`do_longjmp`].
///
/// On Unix x86-64 this holds `sigjmp_buf` (200 bytes, 8-byte
/// aligned).  On Windows x64 this holds the 9 non-volatile
/// integer registers (RBP, RBX, RDI, RSI, RSP, R12-R15), the
/// return address (RIP), and the 10 non-volatile XMM registers
/// (XMM6-XMM15) required by the Win64 ABI (240 bytes, 16-byte
/// aligned).
#[cfg(unix)]
#[repr(C, align(8))]
pub struct JmpBuf([u8; 200]);

#[cfg(windows)]
#[repr(C, align(16))]
pub struct JmpBuf([u8; 240]);

/// Save the current register state; return 0 on direct call.
/// When [`do_longjmp`] targets this buffer, returns the supplied
/// non-zero value (matching POSIX longjmp guarantee).
///
/// # Safety
/// `env` must be a valid, non-null pointer to a `JmpBuf`.
#[cfg(unix)]
#[inline(always)]
pub unsafe fn do_setjmp(env: *mut JmpBuf) -> i32 {
    unsafe extern "C" {
        #[link_name = "__sigsetjmp"]
        fn sigsetjmp(env: *mut JmpBuf, savemask: i32) -> i32;
    }
    unsafe { sigsetjmp(env, 0) }
}

/// # Safety
/// `env` must be a valid, non-null pointer to a `JmpBuf`.
#[cfg(windows)]
#[inline(always)]
pub unsafe fn do_setjmp(env: *mut JmpBuf) -> i32 {
    unsafe extern "C" {
        fn machina_setjmp(env: *mut JmpBuf) -> i32;
    }
    unsafe { machina_setjmp(env) }
}

/// Restore register state saved by [`do_setjmp`] and make it look
/// like `do_setjmp` returned `val` (or 1 if `val` is 0).
///
/// # Safety
/// `env` must have been filled by a preceding [`do_setjmp`] call
/// whose stack frame is still live.
#[cfg(unix)]
pub unsafe fn do_longjmp(env: *mut JmpBuf, val: i32) -> ! {
    unsafe extern "C" {
        fn siglongjmp(env: *mut JmpBuf, val: i32) -> !;
    }
    unsafe { siglongjmp(env, val) }
}

/// # Safety
/// `env` must have been filled by a preceding [`do_setjmp`] call
/// whose stack frame is still live.
#[cfg(windows)]
pub unsafe fn do_longjmp(env: *mut JmpBuf, val: i32) -> ! {
    unsafe extern "C" {
        fn machina_longjmp(env: *mut JmpBuf, val: i32);
    }
    unsafe { machina_longjmp(env, val) };
    // SAFETY: machina_longjmp never returns.
    unsafe { std::hint::unreachable_unchecked() }
}

// ── Windows x64 setjmp / longjmp in pure Rust assembly ──────────
//
// Layout of the 240-byte JmpBuf (offsets in bytes):
//   0   RBP        8   RBX       16  RDI       24  RSI
//  32   RSP(+8)   40   R12       48  R13       56  R14
//  64   R15       72   RIP
//  80   XMM6     96   XMM7     112  XMM8     128  XMM9
// 144   XMM10   160   XMM11    176  XMM12    192  XMM13
// 208   XMM14   224   XMM15
//
// Windows x64 calling convention passes the first argument in RCX.
#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
use std::arch::global_asm;

#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
global_asm!(
    ".globl machina_setjmp",
    "machina_setjmp:",
    // RCX = *mut JmpBuf — save integer nonvolatiles
    "movq %rbp,  0(%rcx)",
    "movq %rbx,  8(%rcx)",
    "movq %rdi, 16(%rcx)",
    "movq %rsi, 24(%rcx)",
    "leaq 8(%rsp), %rax", // post-ret RSP = entry RSP + 8
    "movq %rax, 32(%rcx)",
    "movq %r12, 40(%rcx)",
    "movq %r13, 48(%rcx)",
    "movq %r14, 56(%rcx)",
    "movq %r15, 64(%rcx)",
    "movq (%rsp), %rax", // return address = saved RIP
    "movq %rax, 72(%rcx)",
    // save XMM nonvolatiles (Win64 ABI: XMM6-XMM15)
    "movdqu %xmm6,   80(%rcx)",
    "movdqu %xmm7,   96(%rcx)",
    "movdqu %xmm8,  112(%rcx)",
    "movdqu %xmm9,  128(%rcx)",
    "movdqu %xmm10, 144(%rcx)",
    "movdqu %xmm11, 160(%rcx)",
    "movdqu %xmm12, 176(%rcx)",
    "movdqu %xmm13, 192(%rcx)",
    "movdqu %xmm14, 208(%rcx)",
    "movdqu %xmm15, 224(%rcx)",
    "xorl %eax, %eax", // return 0
    "ret",
    ".globl machina_longjmp",
    "machina_longjmp:",
    // RCX = *mut JmpBuf, EDX = val
    "movl %edx, %eax",
    "testl %eax, %eax",
    "jnz 1f",
    "movl $1, %eax", // longjmp(env,0) must return 1
    "1:",
    // restore integer nonvolatiles
    "movq  0(%rcx), %rbp",
    "movq  8(%rcx), %rbx",
    "movq 16(%rcx), %rdi",
    "movq 24(%rcx), %rsi",
    "movq 32(%rcx), %rsp",
    "movq 40(%rcx), %r12",
    "movq 48(%rcx), %r13",
    "movq 56(%rcx), %r14",
    "movq 64(%rcx), %r15",
    // restore XMM nonvolatiles
    "movdqu  80(%rcx), %xmm6",
    "movdqu  96(%rcx), %xmm7",
    "movdqu 112(%rcx), %xmm8",
    "movdqu 128(%rcx), %xmm9",
    "movdqu 144(%rcx), %xmm10",
    "movdqu 160(%rcx), %xmm11",
    "movdqu 176(%rcx), %xmm12",
    "movdqu 192(%rcx), %xmm13",
    "movdqu 208(%rcx), %xmm14",
    "movdqu 224(%rcx), %xmm15",
    "jmp *72(%rcx)", // jump to saved RIP
    options(att_syntax),
);
