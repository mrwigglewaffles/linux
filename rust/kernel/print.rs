// SPDX-License-Identifier: GPL-2.0

//! Printing facilities.
//!
//! C header: [`include/linux/printk.h`](srctree/include/linux/printk.h)
//!
//! Reference: <https://docs.kernel.org/core-api/printk-basics.html>

use crate::{
    ffi::{c_char, c_void},
    prelude::*,
    str::RawFormatter,
};
use core::fmt;

// Called from `vsprintf` with format specifier `%pA`.
#[expect(clippy::missing_safety_doc)]
#[export]
unsafe extern "C" fn rust_fmt_argument(
    buf: *mut c_char,
    end: *mut c_char,
    ptr: *const c_void,
) -> *mut c_char {
    use fmt::Write;
    // SAFETY: The C contract guarantees that `buf` is valid if it's less than `end`.
    let mut w = unsafe { RawFormatter::from_ptrs(buf.cast(), end.cast()) };
    // SAFETY: TODO.
    let _ = w.write_fmt(unsafe { *(ptr as *const fmt::Arguments<'_>) });
    w.pos().cast()
}

/// Format strings.
///
/// Public but hidden since it should only be used from public macros.
#[doc(hidden)]
pub mod format_strings {
    /// The length we copy from the `KERN_*` kernel prefixes.
    const LENGTH_PREFIX: usize = 2;

    /// The length of the fixed format strings.
    pub const LENGTH: usize = 10;

    /// Generates a fixed format string for the kernel's [`_printk`].
    ///
    /// The format string is always the same for a given level, i.e. for a
    /// given `prefix`, which are the kernel's `KERN_*` constants.
    ///
    /// [`_printk`]: srctree/include/linux/printk.h
    const fn generate(is_cont: bool, prefix: &[u8; 3]) -> [u8; LENGTH] {
        // Ensure the `KERN_*` macros are what we expect.
        assert!(prefix[0] == b'\x01');
        if is_cont {
            assert!(prefix[1] == b'c');
        } else {
            assert!(prefix[1] >= b'0' && prefix[1] <= b'7');
        }
        assert!(prefix[2] == b'\x00');

        let suffix: &[u8; LENGTH - LENGTH_PREFIX] = if is_cont {
            b"%pA\0\0\0\0\0"
        } else {
            b"%s: %pA\0"
        };

        [
            prefix[0], prefix[1], suffix[0], suffix[1], suffix[2], suffix[3], suffix[4], suffix[5],
            suffix[6], suffix[7],
        ]
    }

    // Generate the format strings at compile-time.
    //
    // This avoids the compiler generating the contents on the fly in the stack.
    //
    // Furthermore, `static` instead of `const` is used to share the strings
    // for all the kernel.
    pub static EMERG: [u8; LENGTH] = generate(false, bindings::KERN_EMERG);
    pub static ALERT: [u8; LENGTH] = generate(false, bindings::KERN_ALERT);
    pub static CRIT: [u8; LENGTH] = generate(false, bindings::KERN_CRIT);
    pub static ERR: [u8; LENGTH] = generate(false, bindings::KERN_ERR);
    pub static WARNING: [u8; LENGTH] = generate(false, bindings::KERN_WARNING);
    pub static NOTICE: [u8; LENGTH] = generate(false, bindings::KERN_NOTICE);
    pub static INFO: [u8; LENGTH] = generate(false, bindings::KERN_INFO);
    pub static DEBUG: [u8; LENGTH] = generate(false, bindings::KERN_DEBUG);
    pub static CONT: [u8; LENGTH] = generate(true, bindings::KERN_CONT);
}

/// Prints a message via the kernel's [`_printk`].
///
/// Public but hidden since it should only be used from public macros.
///
/// # Safety
///
/// The format string must be one of the ones in [`format_strings`], and
/// the module name must be null-terminated.
///
/// [`_printk`]: srctree/include/linux/_printk.h
#[doc(hidden)]
#[cfg_attr(not(CONFIG_PRINTK), allow(unused_variables))]
pub unsafe fn call_printk(
    format_string: &[u8; format_strings::LENGTH],
    module_name: &[u8],
    args: fmt::Arguments<'_>,
) {
    // `_printk` does not seem to fail in any path.
    #[cfg(CONFIG_PRINTK)]
    // SAFETY: TODO.
    unsafe {
        bindings::_printk(
            format_string.as_ptr(),
            module_name.as_ptr(),
            &args as *const _ as *const c_void,
        );
    }
}

/// Prints a message via the kernel's [`_printk`] for the `CONT` level.
///
/// Public but hidden since it should only be used from public macros.
///
/// [`_printk`]: srctree/include/linux/printk.h
#[doc(hidden)]
#[cfg_attr(not(CONFIG_PRINTK), allow(unused_variables))]
pub fn call_printk_cont(args: fmt::Arguments<'_>) {
    // `_printk` does not seem to fail in any path.
    //
    // SAFETY: The format string is fixed.
    #[cfg(CONFIG_PRINTK)]
    unsafe {
        bindings::_printk(
            format_strings::CONT.as_ptr(),
            &args as *const _ as *const c_void,
        );
    }
}

/// Performs formatting and forwards the string to [`call_printk`].
///
/// Public but hidden since it should only be used from public macros.
#[doc(hidden)]
#[cfg(not(testlib))]
#[macro_export]
#[expect(clippy::crate_in_macro_def)]
macro_rules! print_macro (
    // The non-continuation cases (most of them, e.g. `INFO`).
    ($format_string:path, false, $($arg:tt)+) => (
        // To remain sound, `arg`s must be expanded outside the `unsafe` block.
        // Typically one would use a `let` binding for that; however, `format_args!`
        // takes borrows on the arguments, but does not extend the scope of temporaries.
        // Therefore, a `match` expression is used to keep them around, since
        // the scrutinee is kept until the end of the `match`.
        match format_args!($($arg)+) {
            // SAFETY: This hidden macro should only be called by the documented
            // printing macros which ensure the format string is one of the fixed
            // ones. All `__LOG_PREFIX`s are null-terminated as they are generated
            // by the `module!` proc macro or fixed values defined in a kernel
            // crate.
            args => unsafe {
                $crate::print::call_printk(
                    &$format_string,
                    crate::__LOG_PREFIX,
                    args,
                );
            }
        }
    );

    // The `CONT` case.
    ($format_string:path, true, $($arg:tt)+) => (
        $crate::print::call_printk_cont(
            format_args!($($arg)+),
        );
    );
);

/// Stub for doctests
#[cfg(testlib)]
#[macro_export]
macro_rules! print_macro (
    ($format_string:path, $e:expr, $($arg:tt)+) => (
        ()
    );
);

// We could use a macro to generate these macros. However, doing so ends
// up being a bit ugly: it requires the dollar token trick to escape `$` as
// well as playing with the `doc` attribute. Furthermore, they cannot be easily
// imported in the prelude due to [1]. So, for the moment, we just write them
// manually, like in the C side; while keeping most of the logic in another
// macro, i.e. [`print_macro`].
//
// [1]: https://github.com/rust-lang/rust/issues/52234

/// Prints an emergency-level message (level 0).
///
/// Use this level if the system is unusable.
///
/// Equivalent to the kernel's [`pr_emerg`] macro.
///
/// Mimics the interface of [`std::print!`]. See [`core::fmt`] and
/// `alloc::format!` for information about the formatting syntax.
///
/// [`pr_emerg`]: https://docs.kernel.org/core-api/printk-basics.html#c.pr_emerg
/// [`std::print!`]: https://doc.rust-lang.org/std/macro.print.html
///
/// # Examples
///
/// ```
/// pr_emerg!("hello {}\n", "there");
/// ```
#[macro_export]
macro_rules! pr_emerg (
    ($($arg:tt)*) => (
        $crate::print_macro!($crate::print::format_strings::EMERG, false, $($arg)*)
    )
);

/// Prints an alert-level message (level 1).
///
/// Use this level if action must be taken immediately.
///
/// Equivalent to the kernel's [`pr_alert`] macro.
///
/// Mimics the interface of [`std::print!`]. See [`core::fmt`] and
/// `alloc::format!` for information about the formatting syntax.
///
/// [`pr_alert`]: https://docs.kernel.org/core-api/printk-basics.html#c.pr_alert
/// [`std::print!`]: https://doc.rust-lang.org/std/macro.print.html
///
/// # Examples
///
/// ```
/// pr_alert!("hello {}\n", "there");
/// ```
#[macro_export]
macro_rules! pr_alert (
    ($($arg:tt)*) => (
        $crate::print_macro!($crate::print::format_strings::ALERT, false, $($arg)*)
    )
);

/// Prints a critical-level message (level 2).
///
/// Use this level for critical conditions.
///
/// Equivalent to the kernel's [`pr_crit`] macro.
///
/// Mimics the interface of [`std::print!`]. See [`core::fmt`] and
/// `alloc::format!` for information about the formatting syntax.
///
/// [`pr_crit`]: https://docs.kernel.org/core-api/printk-basics.html#c.pr_crit
/// [`std::print!`]: https://doc.rust-lang.org/std/macro.print.html
///
/// # Examples
///
/// ```
/// pr_crit!("hello {}\n", "there");
/// ```
#[macro_export]
macro_rules! pr_crit (
    ($($arg:tt)*) => (
        $crate::print_macro!($crate::print::format_strings::CRIT, false, $($arg)*)
    )
);

/// Prints an error-level message (level 3).
///
/// Use this level for error conditions.
///
/// Equivalent to the kernel's [`pr_err`] macro.
///
/// Mimics the interface of [`std::print!`]. See [`core::fmt`] and
/// `alloc::format!` for information about the formatting syntax.
///
/// [`pr_err`]: https://docs.kernel.org/core-api/printk-basics.html#c.pr_err
/// [`std::print!`]: https://doc.rust-lang.org/std/macro.print.html
///
/// # Examples
///
/// ```
/// pr_err!("hello {}\n", "there");
/// ```
#[macro_export]
macro_rules! pr_err (
    ($($arg:tt)*) => (
        $crate::print_macro!($crate::print::format_strings::ERR, false, $($arg)*)
    )
);

/// Prints a warning-level message (level 4).
///
/// Use this level for warning conditions.
///
/// Equivalent to the kernel's [`pr_warn`] macro.
///
/// Mimics the interface of [`std::print!`]. See [`core::fmt`] and
/// `alloc::format!` for information about the formatting syntax.
///
/// [`pr_warn`]: https://docs.kernel.org/core-api/printk-basics.html#c.pr_warn
/// [`std::print!`]: https://doc.rust-lang.org/std/macro.print.html
///
/// # Examples
///
/// ```
/// pr_warn!("hello {}\n", "there");
/// ```
#[macro_export]
macro_rules! pr_warn (
    ($($arg:tt)*) => (
        $crate::print_macro!($crate::print::format_strings::WARNING, false, $($arg)*)
    )
);

/// Prints a notice-level message (level 5).
///
/// Use this level for normal but significant conditions.
///
/// Equivalent to the kernel's [`pr_notice`] macro.
///
/// Mimics the interface of [`std::print!`]. See [`core::fmt`] and
/// `alloc::format!` for information about the formatting syntax.
///
/// [`pr_notice`]: https://docs.kernel.org/core-api/printk-basics.html#c.pr_notice
/// [`std::print!`]: https://doc.rust-lang.org/std/macro.print.html
///
/// # Examples
///
/// ```
/// pr_notice!("hello {}\n", "there");
/// ```
#[macro_export]
macro_rules! pr_notice (
    ($($arg:tt)*) => (
        $crate::print_macro!($crate::print::format_strings::NOTICE, false, $($arg)*)
    )
);

/// Prints an info-level message (level 6).
///
/// Use this level for informational messages.
///
/// Equivalent to the kernel's [`pr_info`] macro.
///
/// Mimics the interface of [`std::print!`]. See [`core::fmt`] and
/// `alloc::format!` for information about the formatting syntax.
///
/// [`pr_info`]: https://docs.kernel.org/core-api/printk-basics.html#c.pr_info
/// [`std::print!`]: https://doc.rust-lang.org/std/macro.print.html
///
/// # Examples
///
/// ```
/// pr_info!("hello {}\n", "there");
/// ```
#[macro_export]
#[doc(alias = "print")]
macro_rules! pr_info (
    ($($arg:tt)*) => (
        $crate::print_macro!($crate::print::format_strings::INFO, false, $($arg)*)
    )
);

/// Prints a debug-level message (level 7).
///
/// Use this level for debug messages.
///
/// Equivalent to the kernel's [`pr_debug`] macro, except that it doesn't support dynamic debug
/// yet.
///
/// Mimics the interface of [`std::print!`]. See [`core::fmt`] and
/// `alloc::format!` for information about the formatting syntax.
///
/// [`pr_debug`]: https://docs.kernel.org/core-api/printk-basics.html#c.pr_debug
/// [`std::print!`]: https://doc.rust-lang.org/std/macro.print.html
///
/// # Examples
///
/// ```
/// pr_debug!("hello {}\n", "there");
/// ```
#[macro_export]
#[doc(alias = "print")]
macro_rules! pr_debug (
    ($($arg:tt)*) => (
        #[cfg(any(DYNAMIC_DEBUG_MODULE, CONFIG_DYNAMIC_DEBUG))]
        { $crate::dynamic_pr_debug!($($arg)*); }

        #[cfg(not(any(DYNAMIC_DEBUG_MODULE, CONFIG_DYNAMIC_DEBUG)))]
        {
            if cfg!(debug_assertions) {
                $crate::print_macro!($crate::print::format_strings::DEBUG, false, $($arg)*)
            }
        }
    )
);

/// Continues a previous log message in the same line.
///
/// Use only when continuing a previous `pr_*!` macro (e.g. [`pr_info!`]).
///
/// Equivalent to the kernel's [`pr_cont`] macro.
///
/// Mimics the interface of [`std::print!`]. See [`core::fmt`] and
/// `alloc::format!` for information about the formatting syntax.
///
/// [`pr_info!`]: crate::pr_info!
/// [`pr_cont`]: https://docs.kernel.org/core-api/printk-basics.html#c.pr_cont
/// [`std::print!`]: https://doc.rust-lang.org/std/macro.print.html
///
/// # Examples
///
/// ```
/// # use kernel::pr_cont;
/// pr_info!("hello");
/// pr_cont!(" {}\n", "there");
/// ```
#[macro_export]
macro_rules! pr_cont (
    ($($arg:tt)*) => (
        $crate::print_macro!($crate::print::format_strings::CONT, true, $($arg)*)
    )
);

#[cfg(CONFIG_DYNAMIC_DEBUG_CORE)]
#[doc(hidden)]
pub mod dynamic_debug {

    pub use bindings::_ddebug;

    use core::fmt;
    use kernel::str::CStr;

    #[doc(hidden)]
    #[repr(transparent)]
    pub struct _Ddebug {
        pub inner: bindings::_ddebug,
    }

    impl _Ddebug {
        pub const fn new(
            modname: &'static CStr,
            function: &'static CStr,
            filename: &'static CStr,
            format: &'static CStr,
            line_num: u32,
            class_id: u32,
        ) -> Self {
            let truncated_line = line_num & 0x3FF;
            let truncated_class = (class_id & 0x3F) << 18;

            let bit_flags: u32 = truncated_class | truncated_line;
            let arr: [u8; 4] = bit_flags.to_ne_bytes();
            let bits = bindings::__BindgenBitfieldUnit::new(arr);
            Self {
                inner: bindings::_ddebug {
                    modname: modname.as_char_ptr(),
                    function: function.as_char_ptr(),
                    filename: filename.as_char_ptr(),
                    format: format.as_char_ptr(),
                    _bitfield_1: bits,
                    ..unsafe { core::mem::zeroed() }
                },
            }
        }
    }

    unsafe impl Send for _Ddebug {}
    unsafe impl Sync for _Ddebug {}

    #[doc(hidden)]
    pub unsafe fn dynamic_pr_debug(descriptor: &mut _Ddebug, fmt: &CStr, args: fmt::Arguments<'_>) {
        unsafe {
            bindings::__dynamic_pr_debug(
                &raw mut descriptor.inner,
                fmt.as_char_ptr(),
                (&raw const args).cast::<ffi::c_void>(),
            );
        }
    }

    /// TODO:
    /// - add docs
    /// - add SAFETY comments
    /// - fix clippy lints
    /// - dont make things public that should not be public
    /// - make sure that the correct items are always in scope. im pretty sure that im missing a few
    /// - test on other arches
    #[macro_export]
    macro_rules! dynamic_pr_debug {
        ($($f:tt)*) => {{
            use kernel::c_str;
            use kernel::str::CStr;
            use kernel::print::dynamic_debug::{_ddebug, _Ddebug};
            use core::fmt;

            const MOD_NAME: &CStr = c_str!(module_path!());
            // right now rust does not have a function! macro so hard code this to be
            // the name of the macro that is printing
            const FN_NAME: &CStr = c_str!("pr_debug!");
            const FILE_NAME: &CStr = c_str!(file!());
            const MESSAGE: &CStr = c_str!("%pA");
            const LINE: u32 = line!();
            const CLASS_ID: u32 = (1 << 6) - 1;


            #[link_section = "__dyndbg"]
            static mut DEBUG_INFO: _Ddebug =
                _Ddebug::new( MOD_NAME, FN_NAME, FILE_NAME, MESSAGE, LINE, CLASS_ID);


            let should_print = unsafe {
                ::kernel::jump_label::static_branch_unlikely!(
                    DEBUG_INFO,
                    _Ddebug,
                    inner.key.dd_key_false
                )
            };

            if should_print {
                unsafe {
                    $crate::print::dynamic_debug::dynamic_pr_debug(
                        &mut DEBUG_INFO,
                        MESSAGE,
                        format_args!($($f)*)
                    )
                };
            }
        }};
    }
}
