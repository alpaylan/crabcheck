//! Panic capture that plays well with `cargo test` and with concurrent
//! runs.
//!
//! The hook is installed once per process and chains to whatever hook was
//! there before. While a thread is inside [`catch`], panics on that thread
//! are recorded in a thread-local slot instead of being printed, so a
//! shrinking run does not spray hundreds of panic dumps into the test
//! output. Other threads are unaffected.

use std::{
    any::Any,
    cell::{
        Cell,
        RefCell,
    },
    panic::{
        self,
        AssertUnwindSafe,
    },
    sync::Once,
};

use crate::report::Location;

/// Message and location of a captured panic.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PanicInfo {
    pub message: String,
    pub location: Option<Location>,
}

thread_local! {
    static SLOT: RefCell<Option<PanicInfo>> = const { RefCell::new(None) };
    static DEPTH: Cell<u32> = const { Cell::new(0) };
}

static INSTALL: Once = Once::new();

fn install() {
    INSTALL.call_once(|| {
        let previous = panic::take_hook();
        panic::set_hook(Box::new(move |info| {
            if DEPTH.with(Cell::get) == 0 {
                previous(info);
                return;
            }
            let message = payload_message(info.payload());
            let location = info.location().map(|l| {
                // Forward slashes on every platform, so reports, targets and
                // snapshots read the same on Windows.
                Location { file: l.file().replace('\\', "/"), line: l.line(), column: l.column() }
            });
            SLOT.with(|slot| *slot.borrow_mut() = Some(PanicInfo { message, location }));
        }));
    });
}

/// Run `f`, converting a panic into an `Err` carrying its message and
/// location. Nested calls are fine.
pub(crate) fn catch<R>(f: impl FnOnce() -> R) -> Result<R, PanicInfo> {
    install();
    DEPTH.with(|d| d.set(d.get() + 1));
    let result = panic::catch_unwind(AssertUnwindSafe(f));
    DEPTH.with(|d| d.set(d.get() - 1));
    match result {
        Ok(value) => Ok(value),
        Err(payload) => {
            let captured = SLOT.with(|slot| slot.borrow_mut().take());
            Err(captured.unwrap_or_else(|| {
                PanicInfo { message: payload_message(&payload), location: None }
            }))
        },
    }
}

fn payload_message(payload: &(dyn Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "<non-string panic payload>".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn captures_message_and_location() {
        let err = catch(|| -> () { panic!("boom {}", 42) }).unwrap_err();
        assert_eq!(err.message, "boom 42");
        let location = err.location.expect("location captured by the hook");
        assert!(location.file.ends_with("panic.rs"), "{}", location.file);
        assert!(location.line > 0);
        assert_eq!(catch(|| 7), Ok(7));
    }

    #[test]
    fn nested_catch_and_depth_restore() {
        let outer = catch(|| {
            let inner = catch(|| -> () { panic!("inner") });
            assert_eq!(inner.unwrap_err().message, "inner");
            panic!("outer")
        });
        assert_eq!(outer.unwrap_err().message, "outer");
        assert_eq!(DEPTH.with(Cell::get), 0);
    }

    #[test]
    fn threads_do_not_share_slots() {
        let handles: Vec<_> = (0..8)
            .map(|i| {
                std::thread::spawn(move || {
                    for _ in 0..50 {
                        let err = catch(|| -> () { panic!("thread {i}") }).unwrap_err();
                        assert_eq!(err.message, format!("thread {i}"));
                    }
                })
            })
            .collect();
        for handle in handles {
            handle.join().unwrap();
        }
    }
}
