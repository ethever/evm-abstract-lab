//! The sole manual Rust/C ABI boundary.
//!
//! Session pointers come only from `evmbw_new`, have exclusive Rust ownership,
//! and are deleted exactly once. The native shim catches every exception and
//! validates owner-tagged term IDs before using them. Slices and C strings live
//! through their calls; native output strings are copied before another call.
//! No Rust function pointer or callback crosses the boundary. `Rc` phantom
//! ownership prevents concurrent access and moving sessions between threads.

use crate::{Error, Status};
use std::ffi::{CStr, CString, c_char, c_void};
use std::marker::PhantomData;
use std::ptr::NonNull;
use std::rc::Rc;

#[derive(Clone, Copy)]
#[repr(C)]
pub(super) struct TermRef {
    pub(super) owner: u64,
    pub(super) id: u64,
}

unsafe extern "C" {
    fn evmbw_new(owner: u64, rlimit: u32) -> *mut c_void;
    fn evmbw_creation_error() -> *const c_char;
    fn evmbw_delete(session: *mut c_void);
    fn evmbw_interrupt_new(session: *mut c_void) -> *mut c_void;
    fn evmbw_interrupt_set(handle: *mut c_void);
    fn evmbw_interrupt_delete(handle: *mut c_void);
    fn evmbw_error(session: *mut c_void) -> *const c_char;
    fn evmbw_bool(session: *mut c_void, value: u8, out: *mut u64) -> i32;
    fn evmbw_bv(session: *mut c_void, width: u32, hex: *const c_char, out: *mut u64) -> i32;
    fn evmbw_variable(session: *mut c_void, width: u32, name: *const c_char, out: *mut u64) -> i32;
    fn evmbw_apply(
        session: *mut c_void,
        kind: u32,
        args: *const TermRef,
        argc: usize,
        indices: *const u32,
        indexc: usize,
        out: *mut u64,
    ) -> i32;
    fn evmbw_assert(session: *mut c_void, term: TermRef) -> i32;
    fn evmbw_check(
        session: *mut c_void,
        status: *mut u32,
        exhausted: *mut u8,
        polls: *mut u64,
    ) -> i32;
    fn evmbw_model(session: *mut c_void, term: TermRef, out: *mut *const c_char) -> i32;
}

pub(super) struct Session {
    pointer: NonNull<c_void>,
    _single_threaded: PhantomData<Rc<()>>,
}

pub(super) struct Interrupt {
    pointer: NonNull<c_void>,
}
// SAFETY: This handle owns an immutable C++ shared_ptr whose pointee is an
// atomic<bool>. It contains no solver pointer; stores can race only other atomic
// operations. Rust ownership ensures Drop cannot race a borrowed interrupt().
unsafe impl Send for Interrupt {}
// SAFETY: Concurrent interrupt() calls only perform atomic stores.
unsafe impl Sync for Interrupt {}
impl Interrupt {
    pub(super) fn interrupt(&self) {
        // SAFETY: The owned non-null handle remains alive for this borrow.
        unsafe {
            evmbw_interrupt_set(self.pointer.as_ptr());
        }
    }
}
impl Drop for Interrupt {
    fn drop(&mut self) {
        // SAFETY: Exactly one owner releases this C++ shared_ptr handle; other
        // handles and the solver retain their own references to the atomic.
        unsafe {
            evmbw_interrupt_delete(self.pointer.as_ptr());
        }
    }
}

impl Session {
    pub(super) fn interrupt_handle(&self) -> Result<Interrupt, Error> {
        // SAFETY: Session is live and accessed on its owning thread. The new
        // handle owns a separate reference to an independent atomic signal.
        let pointer = unsafe { evmbw_interrupt_new(self.pointer.as_ptr()) };
        NonNull::new(pointer)
            .map(|pointer| Interrupt { pointer })
            .ok_or_else(|| Error::Native("allocating cancellation handle failed".into()))
    }
    pub(super) fn new(owner: u64, rlimit: u32) -> Result<Self, Error> {
        // SAFETY: Primitive arguments are validated by both callers and shim;
        // success transfers exclusive ownership of the allocated session.
        let pointer = unsafe { evmbw_new(owner, rlimit) };
        let Some(pointer) = NonNull::new(pointer) else {
            // SAFETY: The thread-local error is always a non-null, terminated
            // string. Copy it before another construction can replace it.
            let message = unsafe { copy_string(evmbw_creation_error()) };
            return Err(Error::Native(message));
        };
        Ok(Self {
            pointer,
            _single_threaded: PhantomData,
        })
    }

    pub(super) fn bool(&mut self, value: bool) -> Result<u64, Error> {
        let mut out = 0;
        // SAFETY: `self` owns the live session; `out` is a valid writable ID.
        let result = unsafe { evmbw_bool(self.pointer.as_ptr(), u8::from(value), &mut out) };
        self.succeeded(result)?;
        Ok(out)
    }

    pub(super) fn bv(&mut self, width: u32, hex: &str) -> Result<u64, Error> {
        let hex = string(hex)?;
        let mut out = 0;
        // SAFETY: The C string and writable ID outlive this synchronous call.
        let result = unsafe { evmbw_bv(self.pointer.as_ptr(), width, hex.as_ptr(), &mut out) };
        self.succeeded(result)?;
        Ok(out)
    }

    pub(super) fn variable(&mut self, width: u32, name: &str) -> Result<u64, Error> {
        let name = string(name)?;
        let mut out = 0;
        // SAFETY: Session, C string and output live through the call. The shim
        // copies the label, and does not retain the provided string pointer.
        let result =
            unsafe { evmbw_variable(self.pointer.as_ptr(), width, name.as_ptr(), &mut out) };
        self.succeeded(result)?;
        Ok(out)
    }

    pub(super) fn apply(
        &mut self,
        kind: u32,
        args: &[TermRef],
        indices: &[u32],
    ) -> Result<u64, Error> {
        let mut out = 0;
        // SAFETY: Slice pointers have the exact paired lengths and remain
        // valid; the shim never dereferences an empty slice or retains it.
        let result = unsafe {
            evmbw_apply(
                self.pointer.as_ptr(),
                kind,
                args.as_ptr(),
                args.len(),
                indices.as_ptr(),
                indices.len(),
                &mut out,
            )
        };
        self.succeeded(result)?;
        Ok(out)
    }

    pub(super) fn assert(&mut self, term: TermRef) -> Result<(), Error> {
        // SAFETY: The live session owns all validated term IDs; the shim also
        // checks the owner, ID range and Boolean sort before asserting.
        let result = unsafe { evmbw_assert(self.pointer.as_ptr(), term) };
        self.succeeded(result)
    }

    pub(super) fn check(&mut self) -> Result<Status, Error> {
        let mut status = 0;
        let mut exhausted = 0;
        let mut polls = 0;
        // SAFETY: All output pointers are initialized, aligned and exclusively
        // writable. The native callback is entirely C++, without Rust reentry.
        let result = unsafe {
            evmbw_check(
                self.pointer.as_ptr(),
                &mut status,
                &mut exhausted,
                &mut polls,
            )
        };
        self.succeeded(result)?;
        Ok(match status {
            10 => Status::Sat,
            20 => Status::Unsat,
            0 => Status::Unknown {
                exhausted: exhausted != 0,
                polls,
            },
            _ => return Err(Error::Native("invalid native check status".into())),
        })
    }

    pub(super) fn model(&mut self, term: TermRef) -> Result<String, Error> {
        let mut out = std::ptr::null();
        // SAFETY: `out` receives a terminated string borrowed from the session;
        // it is copied immediately, before any further mutation or deletion.
        let result = unsafe { evmbw_model(self.pointer.as_ptr(), term, &mut out) };
        self.succeeded(result)?;
        // SAFETY: Success guarantees the output is a non-null native C string.
        Ok(unsafe { copy_string(out) })
    }

    fn succeeded(&self, result: i32) -> Result<(), Error> {
        if result != 0 {
            return Ok(());
        }
        // SAFETY: Every failed call writes a terminated session-local error.
        // The returned string is copied while the session remains live.
        Err(Error::Native(unsafe {
            copy_string(evmbw_error(self.pointer.as_ptr()))
        }))
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        // SAFETY: This is the unique owner; deletion catches all exceptions.
        unsafe { evmbw_delete(self.pointer.as_ptr()) };
    }
}

fn string(value: &str) -> Result<CString, Error> {
    CString::new(value).map_err(|_| Error::InvalidInput("embedded NUL in native string"))
}

unsafe fn copy_string(pointer: *const c_char) -> String {
    // SAFETY: Caller proves non-null pointer to a native terminated string;
    // converting to an owned String avoids exposing a borrowed native lifetime.
    unsafe { CStr::from_ptr(pointer) }
        .to_string_lossy()
        .into_owned()
}
