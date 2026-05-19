//! glibc legacy compatible shared library.

use std::ffi::{CStr, c_char, c_int, c_void};
#[cfg(target_arch = "x86")]
use std::ffi::{c_long, c_uint};
#[cfg(any(target_arch = "x86", test))]
use std::mem::MaybeUninit;
use std::sync::OnceLock;

type ShmOpenFn = unsafe extern "C" fn(*const c_char, c_int, u32) -> c_int;
type ShmUnlinkFn = unsafe extern "C" fn(*const c_char) -> c_int;

#[cfg(target_arch = "x86")]
#[allow(non_camel_case_types)]
type mode_t = c_uint;

const RTLD_NEXT: *mut c_void = -1isize as *mut c_void;
const SHM_NAME_MAX: usize = 255;
#[cfg(any(target_arch = "x86", test))]
const DEV_SHM_PREFIX: &[u8] = b"/dev/shm/";
#[cfg(any(target_arch = "x86", test))]
const DEV_SHM_PREFIX_LEN: usize = DEV_SHM_PREFIX.len();
const ENOSYS: c_int = 38;
#[cfg(target_arch = "x86")]
const EFAULT: c_int = 14;
#[cfg(target_arch = "x86")]
const EEXIST: c_int = 17;

#[cfg(target_arch = "x86")]
mod syscall_nr {
    pub const STAT64: u32 = 195;
    pub const LSTAT64: u32 = 196;
    pub const FSTAT64: u32 = 197;
    pub const MKDIR: u32 = 39;
}

unsafe extern "C" {
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
    fn __errno_location() -> *mut c_int;
}

#[cfg(target_arch = "x86")]
unsafe fn raw_syscall2(nr: u32, a1: u32, a2: u32) -> i32 {
    let ret: i32;
    unsafe {
        std::arch::asm!(
            "int $0x80",
            inout("eax") nr as i32 => ret,
            in("ebx") a1,
            in("ecx") a2,
            options(nostack)
        );
    }
    ret
}

#[cfg(target_arch = "x86")]
unsafe fn raw_syscall3(nr: u32, a1: u32, a2: u32, a3: u32) -> i32 {
    let ret: i32;
    unsafe {
        std::arch::asm!(
            "int $0x80",
            inout("eax") nr as i32 => ret,
            in("ebx") a1,
            in("ecx") a2,
            in("edx") a3,
            options(nostack)
        );
    }
    ret
}

fn set_errno(val: c_int) {
    unsafe { *__errno_location() = val }
}

// Optional syscall trace, on when `DNF_SHM_TRACE` is a writable file path.
// Raw syscalls avoid re-entering the hooked libc symbols.
mod trace {
    #[cfg(target_arch = "x86")]
    use super::*;
    #[cfg(target_arch = "x86")]
    use std::sync::OnceLock;

    #[cfg(target_arch = "x86")]
    static PATH: OnceLock<Option<Box<[u8]>>> = OnceLock::new();

    #[cfg(target_arch = "x86")]
    fn trace_path() -> Option<&'static [u8]> {
        PATH.get_or_init(|| match std::env::var("DNF_SHM_TRACE") {
            Ok(v) if !v.is_empty() => {
                let mut b = v.into_bytes();
                b.push(0);
                Some(b.into_boxed_slice())
            }
            _ => None,
        })
        .as_deref()
    }

    /// Append one record. No-op unless `DNF_SHM_TRACE` is set.
    #[cfg(target_arch = "x86")]
    pub fn emit(tag: &[u8], path_ptr: *const c_char, flags: i32, ret: i32) {
        let file = match trace_path() {
            Some(f) => f,
            None => return,
        };
        // 32-bit x86 syscall numbers: 78 gettimeofday, 20 getpid,
        // 5 open, 4 write, 6 close.
        let mut tv = [0i32; 2];
        unsafe { raw_syscall2(78, tv.as_mut_ptr() as u32, 0) };
        let pid = unsafe { raw_syscall2(20, 0, 0) } as u32;
        let pbytes: &[u8] = if path_ptr.is_null() {
            b"(null)"
        } else {
            unsafe { CStr::from_ptr(path_ptr) }.to_bytes()
        };
        let mut line = [0u8; 512];
        let n = build_trace_line(
            &mut line,
            tv[0] as u64,
            tv[1] as u32,
            pid,
            tag,
            flags,
            ret,
            pbytes,
        );
        // O_WRONLY|O_CREAT|O_APPEND = 1|0o100|0o2000 = 1089
        let fd = unsafe { raw_syscall3(5, file.as_ptr() as u32, 1089, 0o644) };
        if fd >= 0 {
            unsafe { raw_syscall3(4, fd as u32, line.as_ptr() as u32, n as u32) };
            unsafe { raw_syscall2(6, fd as u32, 0) };
        }
    }

    #[cfg(not(target_arch = "x86"))]
    pub fn emit(_tag: &[u8], _path: *const std::ffi::c_char, _flags: i32, _ret: i32) {}
}

#[cfg(target_arch = "x86")]
fn set_errno_from_ret(ret: i32) -> c_int {
    if ret < 0 {
        set_errno(-ret);
        -1
    } else {
        ret
    }
}

// shm_open / shm_unlink hooks

static REAL_SHM_OPEN: OnceLock<Option<ShmOpenFn>> = OnceLock::new();
static REAL_SHM_UNLINK: OnceLock<Option<ShmUnlinkFn>> = OnceLock::new();

fn resolve_shm_open() -> Option<ShmOpenFn> {
    *REAL_SHM_OPEN.get_or_init(|| {
        let ptr = unsafe { dlsym(RTLD_NEXT, c"shm_open".as_ptr()) };
        if ptr.is_null() {
            None
        } else {
            Some(unsafe { std::mem::transmute::<*mut c_void, ShmOpenFn>(ptr) })
        }
    })
}

fn resolve_shm_unlink() -> Option<ShmUnlinkFn> {
    *REAL_SHM_UNLINK.get_or_init(|| {
        let ptr = unsafe { dlsym(RTLD_NEXT, c"shm_unlink".as_ptr()) };
        if ptr.is_null() {
            None
        } else {
            Some(unsafe { std::mem::transmute::<*mut c_void, ShmUnlinkFn>(ptr) })
        }
    })
}

/// # Safety
/// `name` must point to a valid null-terminated C string.
unsafe fn sanitize_on_stack(
    name: *const c_char,
    buf: &mut [u8; SHM_NAME_MAX + 1],
) -> *const c_char {
    let bytes = unsafe { CStr::from_ptr(name) }.to_bytes();

    if !bytes.iter().skip(1).any(|&b| b == b'/') {
        return name;
    }

    if bytes.len() > SHM_NAME_MAX {
        return name;
    }

    for (i, &byte) in bytes.iter().enumerate() {
        buf[i] = if byte == b'/' && i > 0 { b'_' } else { byte };
    }
    buf[bytes.len()] = 0;
    buf.as_ptr() as *const c_char
}

/// Rewrites a `/dev/shm/<a>/<b>` path to `/dev/shm/<a>_<b>`.
/// Returns `path` unchanged when it is not under `/dev/shm/`.
///
/// # Safety
/// `path` must point to a valid null-terminated C string.
#[cfg(any(target_arch = "x86", test))]
unsafe fn sanitize_dev_shm_path(
    path: *const c_char,
    buf: &mut MaybeUninit<[u8; DEV_SHM_PREFIX_LEN + SHM_NAME_MAX + 1]>,
) -> *const c_char {
    let src = path as *const u8;

    // A NUL in the first DEV_SHM_PREFIX_LEN bytes differs from the prefix,
    // so the scan never reads past the terminating NUL.
    let mut i = 0;
    while i < DEV_SHM_PREFIX_LEN {
        if unsafe { *src.add(i) } != DEV_SHM_PREFIX[i] {
            return path;
        }
        i += 1;
    }

    let out = buf.as_mut_ptr() as *mut u8;
    unsafe {
        std::ptr::copy_nonoverlapping(DEV_SHM_PREFIX.as_ptr(), out, DEV_SHM_PREFIX_LEN);
    }
    let mut n = 0usize;
    let mut had_slash = false;
    loop {
        let c = unsafe { *src.add(DEV_SHM_PREFIX_LEN + n) };
        if c == 0 {
            break;
        }
        if n > SHM_NAME_MAX - 2 {
            return path;
        }
        let w = if c == b'/' {
            had_slash = true;
            b'_'
        } else {
            c
        };
        unsafe { *out.add(DEV_SHM_PREFIX_LEN + n) = w };
        n += 1;
    }
    if !had_slash {
        return path;
    }
    unsafe { *out.add(DEV_SHM_PREFIX_LEN + n) = 0 };
    out as *const c_char
}

#[cfg(any(target_arch = "x86", test))]
const O_CREAT: c_int = 0o100;
#[cfg(any(target_arch = "x86", test))]
const O_TMPFILE_BIT: c_int = 0o20_000_000;
#[cfg(test)]
const O_DIRECTORY: c_int = 0o200_000;

/// Mirrors glibc `__OPEN_NEEDS_MODE`: `open` takes `mode` only when the
/// flags create a file.
#[cfg(any(target_arch = "x86", test))]
fn open_needs_mode(flags: c_int) -> bool {
    flags & O_CREAT != 0 || flags & O_TMPFILE_BIT == O_TMPFILE_BIT
}

/// True for any non-empty name under `/dev/shm/`. `mkdir`/`mkdirat` use it
/// to fake success with no real directory. Wider than `sanitize_dev_shm_path`
/// on purpose: every `/dev/shm` mkdir is faked, only embedded-slash names
/// are rewritten.
#[cfg(any(target_arch = "x86", test))]
fn is_dev_shm_path(bytes: &[u8]) -> bool {
    bytes.len() > DEV_SHM_PREFIX_LEN && bytes.starts_with(DEV_SHM_PREFIX)
}

/// Append the decimal digits of `v` to `buf` at `pos`, returning the new
/// position. Stops when `buf` is full.
#[cfg(any(target_arch = "x86", test))]
fn put_dec(buf: &mut [u8], pos: usize, v: u64) -> usize {
    let mut tmp = [0u8; 20];
    let mut n = 0;
    let mut x = v;
    loop {
        tmp[n] = b'0' + (x % 10) as u8;
        x /= 10;
        n += 1;
        if x == 0 {
            break;
        }
    }
    let mut p = pos;
    while n > 0 && p < buf.len() {
        n -= 1;
        buf[p] = tmp[n];
        p += 1;
    }
    p
}

/// Build one trace record into `buf`, returning its length. Format:
/// `<sec>.<usec6> <pid> <tag> fl=0x<hex> r=<dec> <path>\n`, truncated to fit.
#[allow(clippy::too_many_arguments)]
#[cfg(any(target_arch = "x86", test))]
fn build_trace_line(
    buf: &mut [u8],
    sec: u64,
    usec: u32,
    pid: u32,
    tag: &[u8],
    flags: i32,
    ret: i32,
    path: &[u8],
) -> usize {
    let cap = buf.len();
    if cap == 0 {
        return 0;
    }
    let mut p = put_dec(buf, 0, sec);
    if p < cap {
        buf[p] = b'.';
        p += 1;
    }
    let mut div = 100_000u32;
    loop {
        if p < cap {
            buf[p] = b'0' + ((usec / div) % 10) as u8;
            p += 1;
        }
        if div == 1 {
            break;
        }
        div /= 10;
    }
    let lit = |buf: &mut [u8], p: &mut usize, s: &[u8]| {
        for &b in s {
            if *p < cap {
                buf[*p] = b;
                *p += 1;
            }
        }
    };
    if p < cap {
        buf[p] = b' ';
        p += 1;
    }
    p = put_dec(buf, p, pid as u64);
    if p < cap {
        buf[p] = b' ';
        p += 1;
    }
    lit(buf, &mut p, tag);
    lit(buf, &mut p, b" fl=0x");
    let v = flags as u32;
    let mut started = false;
    let mut shift: i32 = 28;
    while shift >= 0 {
        let nyb = ((v >> shift) & 0xf) as u8;
        if nyb != 0 || started || shift == 0 {
            started = true;
            let c = if nyb < 10 {
                b'0' + nyb
            } else {
                b'a' + nyb - 10
            };
            if p < cap {
                buf[p] = c;
                p += 1;
            }
        }
        shift -= 4;
    }
    lit(buf, &mut p, b" r=");
    if ret < 0 {
        if p < cap {
            buf[p] = b'-';
            p += 1;
        }
        p = put_dec(buf, p, (-(ret as i64)) as u64);
    } else {
        p = put_dec(buf, p, ret as u64);
    }
    if p < cap {
        buf[p] = b' ';
        p += 1;
    }
    for &b in path {
        if p < cap - 1 {
            buf[p] = b;
            p += 1;
        } else {
            break;
        }
    }
    if p < cap {
        buf[p] = b'\n';
        p += 1;
    } else {
        buf[cap - 1] = b'\n';
        p = cap;
    }
    p
}

/// Opens a POSIX shared memory object.
///
/// # Safety
/// `name` must be NULL or a valid pointer to a null-terminated C string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn shm_open(name: *const c_char, oflag: c_int, mode: u32) -> c_int {
    let real_fn = match resolve_shm_open() {
        Some(f) => f,
        None => {
            set_errno(ENOSYS);
            return -1;
        }
    };

    // Forward null to the real function and let glibc produce the appropriate error.
    if name.is_null() {
        return unsafe { real_fn(name, oflag, mode) };
    }

    let mut buf = [0u8; SHM_NAME_MAX + 1];
    let patched = unsafe { sanitize_on_stack(name, &mut buf) };
    let r = unsafe { real_fn(patched, oflag, mode) };
    trace::emit(b"shm_open", patched, oflag, r);
    r
}

/// Removes a POSIX shared memory object.
///
/// # Safety
/// `name` must be NULL or a valid pointer to a null-terminated C string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn shm_unlink(name: *const c_char) -> c_int {
    let real_fn = match resolve_shm_unlink() {
        Some(f) => f,
        None => {
            set_errno(ENOSYS);
            return -1;
        }
    };

    // Forward null to the real function and let glibc produce the appropriate error.
    if name.is_null() {
        return unsafe { real_fn(name) };
    }

    let mut buf = [0u8; SHM_NAME_MAX + 1];
    let patched = unsafe { sanitize_on_stack(name, &mut buf) };
    let r = unsafe { real_fn(patched) };
    trace::emit(b"shm_unlink", patched, 0, r);
    r
}

// stat family hooks — use raw stat64/fstat64/lstat64 syscalls

#[cfg(target_arch = "x86")]
mod stat_hooks {
    use super::*;

    const _STAT_VER_LINUX: c_int = 3;

    /// Hook for __xstat64
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn __xstat64(ver: c_int, path: *const c_char, buf: *mut c_void) -> c_int {
        if ver != _STAT_VER_LINUX || path.is_null() || buf.is_null() {
            type XstatFn = unsafe extern "C" fn(c_int, *const c_char, *mut c_void) -> c_int;
            let ptr = unsafe { dlsym(RTLD_NEXT, c"__xstat64".as_ptr()) };
            if ptr.is_null() {
                set_errno(ENOSYS);
                return -1;
            }
            let real_fn: XstatFn = unsafe { std::mem::transmute(ptr) };
            return unsafe { real_fn(ver, path, buf) };
        }
        let mut pbuf = MaybeUninit::<[u8; DEV_SHM_PREFIX_LEN + SHM_NAME_MAX + 1]>::uninit();
        let patched = unsafe { sanitize_dev_shm_path(path, &mut pbuf) };
        let ret = unsafe { raw_syscall2(syscall_nr::STAT64, patched as u32, buf as u32) };
        trace::emit(b"xstat64", patched, 0, ret);
        set_errno_from_ret(ret)
    }

    /// Hook for __fxstat64
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn __fxstat64(ver: c_int, fd: c_int, buf: *mut c_void) -> c_int {
        if ver != _STAT_VER_LINUX || buf.is_null() {
            type FxstatFn = unsafe extern "C" fn(c_int, c_int, *mut c_void) -> c_int;
            let ptr = unsafe { dlsym(RTLD_NEXT, c"__fxstat64".as_ptr()) };
            if ptr.is_null() {
                set_errno(ENOSYS);
                return -1;
            }
            let real_fn: FxstatFn = unsafe { std::mem::transmute(ptr) };
            return unsafe { real_fn(ver, fd, buf) };
        }
        let ret = unsafe { raw_syscall2(syscall_nr::FSTAT64, fd as u32, buf as u32) };
        set_errno_from_ret(ret)
    }

    /// Hook for __lxstat64
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn __lxstat64(
        ver: c_int,
        path: *const c_char,
        buf: *mut c_void,
    ) -> c_int {
        if ver != _STAT_VER_LINUX || path.is_null() || buf.is_null() {
            type LxstatFn = unsafe extern "C" fn(c_int, *const c_char, *mut c_void) -> c_int;
            let ptr = unsafe { dlsym(RTLD_NEXT, c"__lxstat64".as_ptr()) };
            if ptr.is_null() {
                set_errno(ENOSYS);
                return -1;
            }
            let real_fn: LxstatFn = unsafe { std::mem::transmute(ptr) };
            return unsafe { real_fn(ver, path, buf) };
        }
        let mut pbuf = MaybeUninit::<[u8; DEV_SHM_PREFIX_LEN + SHM_NAME_MAX + 1]>::uninit();
        let patched = unsafe { sanitize_dev_shm_path(path, &mut pbuf) };
        let ret = unsafe { raw_syscall2(syscall_nr::LSTAT64, patched as u32, buf as u32) };
        set_errno_from_ret(ret)
    }

    /// Hook for __xstat
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn __xstat(ver: c_int, path: *const c_char, buf: *mut c_void) -> c_int {
        if ver == _STAT_VER_LINUX && !path.is_null() && !buf.is_null() {
            let mut pbuf = MaybeUninit::<[u8; DEV_SHM_PREFIX_LEN + SHM_NAME_MAX + 1]>::uninit();
            let patched = unsafe { sanitize_dev_shm_path(path, &mut pbuf) };
            let ret = unsafe { raw_syscall2(syscall_nr::STAT64, patched as u32, buf as u32) };
            return set_errno_from_ret(ret);
        }
        type XstatFn = unsafe extern "C" fn(c_int, *const c_char, *mut c_void) -> c_int;
        let ptr = unsafe { dlsym(RTLD_NEXT, c"__xstat".as_ptr()) };
        if ptr.is_null() {
            set_errno(ENOSYS);
            return -1;
        }
        let real_fn: XstatFn = unsafe { std::mem::transmute(ptr) };
        unsafe { real_fn(ver, path, buf) }
    }

    /// Hook for __fxstat
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn __fxstat(ver: c_int, fd: c_int, buf: *mut c_void) -> c_int {
        if ver == _STAT_VER_LINUX && !buf.is_null() {
            let ret = unsafe { raw_syscall2(syscall_nr::FSTAT64, fd as u32, buf as u32) };
            return set_errno_from_ret(ret);
        }
        type FxstatFn = unsafe extern "C" fn(c_int, c_int, *mut c_void) -> c_int;
        let ptr = unsafe { dlsym(RTLD_NEXT, c"__fxstat".as_ptr()) };
        if ptr.is_null() {
            set_errno(ENOSYS);
            return -1;
        }
        let real_fn: FxstatFn = unsafe { std::mem::transmute(ptr) };
        unsafe { real_fn(ver, fd, buf) }
    }

    /// Hook for __lxstat
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn __lxstat(ver: c_int, path: *const c_char, buf: *mut c_void) -> c_int {
        if ver == _STAT_VER_LINUX && !path.is_null() && !buf.is_null() {
            let mut pbuf = MaybeUninit::<[u8; DEV_SHM_PREFIX_LEN + SHM_NAME_MAX + 1]>::uninit();
            let patched = unsafe { sanitize_dev_shm_path(path, &mut pbuf) };
            let ret = unsafe { raw_syscall2(syscall_nr::LSTAT64, patched as u32, buf as u32) };
            return set_errno_from_ret(ret);
        }
        type LxstatFn = unsafe extern "C" fn(c_int, *const c_char, *mut c_void) -> c_int;
        let ptr = unsafe { dlsym(RTLD_NEXT, c"__lxstat".as_ptr()) };
        if ptr.is_null() {
            set_errno(ENOSYS);
            return -1;
        }
        let real_fn: LxstatFn = unsafe { std::mem::transmute(ptr) };
        unsafe { real_fn(ver, path, buf) }
    }

    /// Hook for `mkdir`. A `/dev/shm/...` target is faked per
    /// `is_dev_shm_path`; others treat `EEXIST` as success.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn mkdir(path: *const c_char, mode: mode_t) -> c_int {
        if path.is_null() {
            set_errno(EFAULT);
            return -1;
        }
        let bytes = unsafe { CStr::from_ptr(path) }.to_bytes();
        if is_dev_shm_path(bytes) {
            return 0;
        }
        let ret = unsafe { raw_syscall2(syscall_nr::MKDIR, path as u32, mode) };
        if ret == -EEXIST {
            return 0;
        }
        set_errno_from_ret(ret)
    }
}

// path-IO family hooks. The same `/dev/shm/a/b` -> `/dev/shm/a_b` rewrite,
// so every libc call reaching a shm bus file lands on the one object
// `shm_open` made. The literal-byte match covers absolute `/dev/shm/...`
// paths only; the target uses absolute paths.

#[cfg(target_arch = "x86")]
mod path_hooks {
    use super::*;

    use std::sync::atomic::{AtomicPtr, Ordering};

    type PathBuf = MaybeUninit<[u8; DEV_SHM_PREFIX_LEN + SHM_NAME_MAX + 1]>;

    /// Resolve `sym` once and cache it. The address is stable and shares no
    /// data, so `Relaxed` is enough. Null is not cached, so a missing symbol
    /// is retried.
    #[inline]
    unsafe fn resolve(slot: &AtomicPtr<c_void>, sym: *const c_char) -> *mut c_void {
        let cached = slot.load(Ordering::Relaxed);
        if !cached.is_null() {
            return cached;
        }
        let r = unsafe { dlsym(RTLD_NEXT, sym) };
        slot.store(r, Ordering::Relaxed);
        r
    }

    /// Rewrite a `/dev/shm/<a>/<b>` `path` into `buf`; null passes through.
    #[inline]
    unsafe fn patch(path: *const c_char, buf: &mut PathBuf) -> *const c_char {
        if path.is_null() {
            path
        } else {
            unsafe { sanitize_dev_shm_path(path, buf) }
        }
    }

    // The open and creat family is variadic in C. With the 32-bit x86 cdecl
    // ABI all arguments go on the stack, so a fixed `mode` parameter is
    // safe: a 2-argument caller leaves `mode` unused and unforwarded, and
    // glibc reads it only when `open_needs_mode`. Relies on the cdecl stack
    // layout.

    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn open(path: *const c_char, flags: c_int, mode: mode_t) -> c_int {
        type Fn = unsafe extern "C" fn(*const c_char, c_int, mode_t) -> c_int;
        static REAL: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());
        let ptr = unsafe { resolve(&REAL, c"open".as_ptr()) };
        if ptr.is_null() {
            set_errno(ENOSYS);
            return -1;
        }
        let real: Fn = unsafe { std::mem::transmute(ptr) };
        let mut buf: PathBuf = MaybeUninit::uninit();
        let p = unsafe { patch(path, &mut buf) };
        let m = if open_needs_mode(flags) { mode } else { 0 };
        unsafe { real(p, flags, m) }
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn open64(path: *const c_char, flags: c_int, mode: mode_t) -> c_int {
        type Fn = unsafe extern "C" fn(*const c_char, c_int, mode_t) -> c_int;
        static REAL: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());
        let ptr = unsafe { resolve(&REAL, c"open64".as_ptr()) };
        if ptr.is_null() {
            set_errno(ENOSYS);
            return -1;
        }
        let real: Fn = unsafe { std::mem::transmute(ptr) };
        let mut buf: PathBuf = MaybeUninit::uninit();
        let p = unsafe { patch(path, &mut buf) };
        let m = if open_needs_mode(flags) { mode } else { 0 };
        unsafe { real(p, flags, m) }
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn openat(
        dirfd: c_int,
        path: *const c_char,
        flags: c_int,
        mode: mode_t,
    ) -> c_int {
        type Fn = unsafe extern "C" fn(c_int, *const c_char, c_int, mode_t) -> c_int;
        static REAL: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());
        let ptr = unsafe { resolve(&REAL, c"openat".as_ptr()) };
        if ptr.is_null() {
            set_errno(ENOSYS);
            return -1;
        }
        let real: Fn = unsafe { std::mem::transmute(ptr) };
        let mut buf: PathBuf = MaybeUninit::uninit();
        let p = unsafe { patch(path, &mut buf) };
        let m = if open_needs_mode(flags) { mode } else { 0 };
        unsafe { real(dirfd, p, flags, m) }
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn openat64(
        dirfd: c_int,
        path: *const c_char,
        flags: c_int,
        mode: mode_t,
    ) -> c_int {
        type Fn = unsafe extern "C" fn(c_int, *const c_char, c_int, mode_t) -> c_int;
        static REAL: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());
        let ptr = unsafe { resolve(&REAL, c"openat64".as_ptr()) };
        if ptr.is_null() {
            set_errno(ENOSYS);
            return -1;
        }
        let real: Fn = unsafe { std::mem::transmute(ptr) };
        let mut buf: PathBuf = MaybeUninit::uninit();
        let p = unsafe { patch(path, &mut buf) };
        let m = if open_needs_mode(flags) { mode } else { 0 };
        unsafe { real(dirfd, p, flags, m) }
    }

    /// Hook for path-first libc calls returning int.
    macro_rules! path_int_hook {
        ($name:ident, $sym:literal, ($($an:ident: $at:ty),*)) => {
            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn $name(path: *const c_char $(, $an: $at)*) -> c_int {
                type Fn = unsafe extern "C" fn(*const c_char $(, $at)*) -> c_int;
                static REAL: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());
                let ptr = unsafe { resolve(&REAL, $sym.as_ptr()) };
                if ptr.is_null() {
                    set_errno(ENOSYS);
                    return -1;
                }
                let real: Fn = unsafe { std::mem::transmute(ptr) };
                let mut buf: PathBuf = MaybeUninit::uninit();
                let p = unsafe { patch(path, &mut buf) };
                unsafe { real(p $(, $an)*) }
            }
        };
    }

    path_int_hook!(access, c"access", (mode: c_int));
    path_int_hook!(euidaccess, c"euidaccess", (mode: c_int));
    path_int_hook!(eaccess, c"eaccess", (mode: c_int));
    path_int_hook!(unlink, c"unlink", ());
    path_int_hook!(truncate, c"truncate", (length: c_long));
    path_int_hook!(truncate64, c"truncate64", (length: i64));

    // creat always creates a file, so it always has a `mode` argument.
    path_int_hook!(creat, c"creat", (mode: mode_t));
    path_int_hook!(creat64, c"creat64", (mode: mode_t));

    // `_FORTIFY_SOURCE` entry points for 2-argument open. Fortified callers
    // bypass `open`, so they need the rewrite too.
    path_int_hook!(__open_2, c"__open_2", (oflag: c_int));
    path_int_hook!(__open64_2, c"__open64_2", (oflag: c_int));

    /// Hook for libc calls shaped `dirfd, path, ... -> int`.
    macro_rules! fd_path_int_hook {
        ($name:ident, $sym:literal, ($($an:ident: $at:ty),*)) => {
            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn $name(
                dirfd: c_int,
                path: *const c_char
                $(, $an: $at)*
            ) -> c_int {
                type Fn = unsafe extern "C" fn(c_int, *const c_char $(, $at)*) -> c_int;
                static REAL: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());
                let ptr = unsafe { resolve(&REAL, $sym.as_ptr()) };
                if ptr.is_null() {
                    set_errno(ENOSYS);
                    return -1;
                }
                let real: Fn = unsafe { std::mem::transmute(ptr) };
                let mut buf: PathBuf = MaybeUninit::uninit();
                let p = unsafe { patch(path, &mut buf) };
                unsafe { real(dirfd, p $(, $an)*) }
            }
        };
    }

    fd_path_int_hook!(faccessat, c"faccessat", (mode: c_int, flags: c_int));
    fd_path_int_hook!(unlinkat, c"unlinkat", (flags: c_int));
    fd_path_int_hook!(__openat_2, c"__openat_2", (oflag: c_int));
    fd_path_int_hook!(__openat64_2, c"__openat64_2", (oflag: c_int));

    /// Hook for path-first libc calls returning a pointer.
    macro_rules! path_ptr_hook {
        ($name:ident, $sym:literal, ($($an:ident: $at:ty),*)) => {
            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn $name(path: *const c_char $(, $an: $at)*) -> *mut c_void {
                type Fn = unsafe extern "C" fn(*const c_char $(, $at)*) -> *mut c_void;
                static REAL: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());
                let ptr = unsafe { resolve(&REAL, $sym.as_ptr()) };
                if ptr.is_null() {
                    set_errno(ENOSYS);
                    return std::ptr::null_mut();
                }
                let real: Fn = unsafe { std::mem::transmute(ptr) };
                let mut buf: PathBuf = MaybeUninit::uninit();
                let p = unsafe { patch(path, &mut buf) };
                unsafe { real(p $(, $an)*) }
            }
        };
    }

    path_ptr_hook!(fopen, c"fopen", (mode: *const c_char));
    path_ptr_hook!(fopen64, c"fopen64", (mode: *const c_char));
    path_ptr_hook!(opendir, c"opendir", ());

    // `stat_hooks` already sanitizes the __xstat family; only statx is
    // added here.

    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn statx(
        dirfd: c_int,
        path: *const c_char,
        flags: c_int,
        mask: c_uint,
        stat_buf: *mut c_void,
    ) -> c_int {
        type Fn = unsafe extern "C" fn(c_int, *const c_char, c_int, c_uint, *mut c_void) -> c_int;
        static REAL: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());
        let ptr = unsafe { resolve(&REAL, c"statx".as_ptr()) };
        if ptr.is_null() {
            set_errno(ENOSYS);
            return -1;
        }
        let real: Fn = unsafe { std::mem::transmute(ptr) };
        let mut buf: PathBuf = MaybeUninit::uninit();
        let p = unsafe { patch(path, &mut buf) };
        unsafe { real(dirfd, p, flags, mask, stat_buf) }
    }

    /// Like `mkdir`: a `/dev/shm/...` target is faked per `is_dev_shm_path`;
    /// others forward to the real `mkdirat`.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn mkdirat(dirfd: c_int, path: *const c_char, mode: mode_t) -> c_int {
        if path.is_null() {
            set_errno(EFAULT);
            return -1;
        }
        let bytes = unsafe { CStr::from_ptr(path) }.to_bytes();
        if is_dev_shm_path(bytes) {
            return 0;
        }
        type Fn = unsafe extern "C" fn(c_int, *const c_char, mode_t) -> c_int;
        static REAL: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());
        let ptr = unsafe { resolve(&REAL, c"mkdirat".as_ptr()) };
        if ptr.is_null() {
            set_errno(ENOSYS);
            return -1;
        }
        let real: Fn = unsafe { std::mem::transmute(ptr) };
        unsafe { real(dirfd, path, mode) }
    }

    /// Rewrites both paths.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn rename(old: *const c_char, new: *const c_char) -> c_int {
        type Fn = unsafe extern "C" fn(*const c_char, *const c_char) -> c_int;
        static REAL: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());
        let ptr = unsafe { resolve(&REAL, c"rename".as_ptr()) };
        if ptr.is_null() {
            set_errno(ENOSYS);
            return -1;
        }
        let real: Fn = unsafe { std::mem::transmute(ptr) };
        let mut ob: PathBuf = MaybeUninit::uninit();
        let mut nb: PathBuf = MaybeUninit::uninit();
        let op = unsafe { patch(old, &mut ob) };
        let np = unsafe { patch(new, &mut nb) };
        unsafe { real(op, np) }
    }

    /// Rewrites both paths.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn renameat(
        olddirfd: c_int,
        old: *const c_char,
        newdirfd: c_int,
        new: *const c_char,
    ) -> c_int {
        type Fn = unsafe extern "C" fn(c_int, *const c_char, c_int, *const c_char) -> c_int;
        static REAL: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());
        let ptr = unsafe { resolve(&REAL, c"renameat".as_ptr()) };
        if ptr.is_null() {
            set_errno(ENOSYS);
            return -1;
        }
        let real: Fn = unsafe { std::mem::transmute(ptr) };
        let mut ob: PathBuf = MaybeUninit::uninit();
        let mut nb: PathBuf = MaybeUninit::uninit();
        let op = unsafe { patch(old, &mut ob) };
        let np = unsafe { patch(new, &mut nb) };
        unsafe { real(olddirfd, op, newdirfd, np) }
    }

    /// Rewrites both paths; `flags` is forwarded unchanged.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn renameat2(
        olddirfd: c_int,
        old: *const c_char,
        newdirfd: c_int,
        new: *const c_char,
        flags: c_uint,
    ) -> c_int {
        type Fn = unsafe extern "C" fn(c_int, *const c_char, c_int, *const c_char, c_uint) -> c_int;
        static REAL: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());
        let ptr = unsafe { resolve(&REAL, c"renameat2".as_ptr()) };
        if ptr.is_null() {
            set_errno(ENOSYS);
            return -1;
        }
        let real: Fn = unsafe { std::mem::transmute(ptr) };
        let mut ob: PathBuf = MaybeUninit::uninit();
        let mut nb: PathBuf = MaybeUninit::uninit();
        let op = unsafe { patch(old, &mut ob) };
        let np = unsafe { patch(new, &mut nb) };
        unsafe { real(olddirfd, op, newdirfd, np, flags) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;

    fn sanitize(input: &[u8]) -> Option<Vec<u8>> {
        let c_str = CString::new(input).unwrap();
        let mut buf = [0u8; SHM_NAME_MAX + 1];
        let result = unsafe { sanitize_on_stack(c_str.as_ptr(), &mut buf) };
        if result == c_str.as_ptr() {
            None
        } else {
            let out = unsafe { CStr::from_ptr(result) };
            Some(out.to_bytes().to_vec())
        }
    }

    #[test]
    fn no_embedded_slash() {
        assert_eq!(sanitize(b"/simple"), None);
    }

    #[test]
    fn embedded_slash_replaced() {
        assert_eq!(sanitize(b"/sec/tss"), Some(b"/sec_tss".to_vec()));
    }

    #[test]
    fn multiple_slashes_replaced() {
        assert_eq!(sanitize(b"/a/b/c"), Some(b"/a_b_c".to_vec()));
    }

    #[test]
    fn leading_slash_preserved() {
        let result = sanitize(b"/a/b/c").unwrap();
        assert_eq!(result[0], b'/');
    }

    #[test]
    fn no_leading_slash_no_embedded_slash() {
        assert_eq!(sanitize(b"noslash"), None);
    }

    #[test]
    fn single_slash() {
        assert_eq!(sanitize(b"/"), None);
    }

    #[test]
    fn empty_name() {
        assert_eq!(sanitize(b""), None);
    }

    #[test]
    fn exactly_max_length() {
        let mut name = vec![b'/'];
        name.extend(std::iter::repeat_n(b'a', SHM_NAME_MAX - 2));
        name.push(b'/');
        assert_eq!(name.len(), SHM_NAME_MAX);

        let result = sanitize(&name).unwrap();
        assert_eq!(result.len(), SHM_NAME_MAX);
        assert_eq!(result[0], b'/');
        assert_eq!(*result.last().unwrap(), b'_');
    }

    #[test]
    fn exceeds_max_length_passes_through() {
        let mut name = vec![b'/'];
        name.extend(std::iter::repeat_n(b'a', SHM_NAME_MAX));
        name.push(b'/');
        assert!(name.len() > SHM_NAME_MAX);

        assert_eq!(sanitize(&name), None);
    }

    fn sanitize_path(input: &[u8]) -> Option<Vec<u8>> {
        let c_str = CString::new(input).unwrap();
        let mut buf = MaybeUninit::<[u8; DEV_SHM_PREFIX_LEN + SHM_NAME_MAX + 1]>::uninit();
        let result = unsafe { sanitize_dev_shm_path(c_str.as_ptr(), &mut buf) };
        if result == c_str.as_ptr() {
            None
        } else {
            let out = unsafe { CStr::from_ptr(result) };
            Some(out.to_bytes().to_vec())
        }
    }

    #[test]
    fn dev_shm_embedded_slash_rewritten() {
        assert_eq!(
            sanitize_path(b"/dev/shm/sec/tss_sdk_bus_1"),
            Some(b"/dev/shm/sec_tss_sdk_bus_1".to_vec())
        );
    }

    #[test]
    fn dev_shm_multiple_slashes_rewritten() {
        assert_eq!(
            sanitize_path(b"/dev/shm/a/b/c"),
            Some(b"/dev/shm/a_b_c".to_vec())
        );
    }

    #[test]
    fn dev_shm_no_embedded_slash_passes_through() {
        assert_eq!(sanitize_path(b"/dev/shm/simple"), None);
    }

    #[test]
    fn non_dev_shm_path_passes_through() {
        assert_eq!(sanitize_path(b"/etc/passwd"), None);
        assert_eq!(
            sanitize_path(b"/home/neople/secsvr/zergsvr/zergsvr.pid"),
            None
        );
    }

    #[test]
    fn dev_shm_empty_suffix_passes_through() {
        assert_eq!(sanitize_path(b"/dev/shm/"), None);
    }

    #[test]
    fn dev_shm_prefix_only_passes_through() {
        assert_eq!(sanitize_path(b"/dev/shm"), None);
    }

    #[test]
    fn dev_shm_exceeds_max_length_passes_through() {
        let mut path = DEV_SHM_PREFIX.to_vec();
        path.push(b'a');
        path.push(b'/');
        path.extend(std::iter::repeat_n(b'b', SHM_NAME_MAX));
        assert!(path.len() - DEV_SHM_PREFIX_LEN > SHM_NAME_MAX);

        assert_eq!(sanitize_path(&path), None);
    }

    #[test]
    fn dev_shm_name_at_max_minus_one_rewritten() {
        let mut path = DEV_SHM_PREFIX.to_vec();
        path.push(b'a');
        path.push(b'/');
        path.extend(std::iter::repeat_n(b'b', SHM_NAME_MAX - 3));
        assert_eq!(path.len() - DEV_SHM_PREFIX_LEN, SHM_NAME_MAX - 1);

        let mut expected = DEV_SHM_PREFIX.to_vec();
        expected.push(b'a');
        expected.push(b'_');
        expected.extend(std::iter::repeat_n(b'b', SHM_NAME_MAX - 3));

        assert_eq!(sanitize_path(&path), Some(expected));
    }

    #[test]
    fn dev_shm_multiple_slashes_at_max_minus_one_rewritten() {
        let mut name = vec![b'a', b'/', b'b', b'/', b'c', b'/'];
        name.extend(std::iter::repeat_n(b'd', SHM_NAME_MAX - 1 - name.len()));
        assert_eq!(name.len(), SHM_NAME_MAX - 1);

        let mut path = DEV_SHM_PREFIX.to_vec();
        path.extend_from_slice(&name);

        let mut expected = DEV_SHM_PREFIX.to_vec();
        expected.extend(name.iter().map(|&b| if b == b'/' { b'_' } else { b }));

        assert_eq!(sanitize_path(&path), Some(expected));
    }

    #[test]
    fn open_rdonly_needs_no_mode() {
        assert!(!open_needs_mode(0));
    }

    #[test]
    fn open_creat_needs_mode() {
        assert!(open_needs_mode(O_CREAT));
    }

    #[test]
    fn open_wronly_creat_needs_mode() {
        assert!(open_needs_mode(1 | O_CREAT));
    }

    #[test]
    fn open_tmpfile_needs_mode() {
        assert!(open_needs_mode(O_TMPFILE_BIT | O_DIRECTORY));
    }

    #[test]
    fn open_directory_only_needs_no_mode() {
        assert!(!open_needs_mode(O_DIRECTORY));
    }

    #[test]
    fn dev_shm_dir_is_dev_shm_path() {
        assert!(is_dev_shm_path(b"/dev/shm/sec"));
    }

    #[test]
    fn dev_shm_file_is_dev_shm_path() {
        assert!(is_dev_shm_path(b"/dev/shm/sec/tss_sdk_bus_1"));
    }

    #[test]
    fn non_dev_shm_is_not_dev_shm_path() {
        assert!(!is_dev_shm_path(b"/etc/passwd"));
    }

    #[test]
    fn dev_shm_prefix_only_is_not_dev_shm_path() {
        assert!(!is_dev_shm_path(b"/dev/shm/"));
        assert!(!is_dev_shm_path(b"/dev/shm"));
    }

    #[test]
    fn dev_shm_lookalike_is_not_dev_shm_path() {
        assert!(!is_dev_shm_path(b"/dev/shmfoo"));
    }

    fn trace_line(sec: u64, usec: u32, pid: u32, tag: &[u8], fl: i32, r: i32, p: &[u8]) -> String {
        let mut buf = [0u8; 512];
        let n = build_trace_line(&mut buf, sec, usec, pid, tag, fl, r, p);
        String::from_utf8(buf[..n].to_vec()).unwrap()
    }

    #[test]
    fn trace_line_basic_shape() {
        assert_eq!(
            trace_line(
                1747640000,
                123456,
                4242,
                b"shmopen",
                0x242,
                7,
                b"sec/tss_sdk_bus_1"
            ),
            "1747640000.123456 4242 shmopen fl=0x242 r=7 sec/tss_sdk_bus_1\n"
        );
    }

    #[test]
    fn trace_line_pads_usec_to_six() {
        assert_eq!(
            trace_line(1000, 42, 1, b"x", 0, 0, b"p"),
            "1000.000042 1 x fl=0x0 r=0 p\n"
        );
    }

    #[test]
    fn trace_line_negative_ret() {
        assert_eq!(
            trace_line(5, 7, 9, b"stat64", 0, -2, b"/dev/shm/sec_tss_sdk_bus_1"),
            "5.000007 9 stat64 fl=0x0 r=-2 /dev/shm/sec_tss_sdk_bus_1\n"
        );
    }

    #[test]
    fn trace_line_truncates_and_terminates() {
        let long = vec![b'a'; 4096];
        let mut buf = [0u8; 128];
        let n = build_trace_line(&mut buf, 1, 2, 3, b"open64", 0o1102, 4, &long);
        assert!(n <= 128);
        assert_eq!(buf[n - 1], b'\n');
    }

    #[test]
    fn dev_shm_name_at_max_passes_through() {
        let mut path = DEV_SHM_PREFIX.to_vec();
        path.push(b'a');
        path.push(b'/');
        path.extend(std::iter::repeat_n(b'b', SHM_NAME_MAX - 2));
        assert_eq!(path.len() - DEV_SHM_PREFIX_LEN, SHM_NAME_MAX);

        assert_eq!(sanitize_path(&path), None);
    }
}
