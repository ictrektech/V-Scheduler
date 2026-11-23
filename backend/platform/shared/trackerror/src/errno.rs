// errno 常量定义
//
// 跨平台分组：Linux 与 Windows libc 均提供，直接引用 `libc::$name`。
// Linux 专有分组：Windows / macOS 等非 Linux 平台 libc 不提供（如 ECHRNG、EL2NSYNC），
//   用 `#[cfg(any(target_os = "linux", target_os = "android"))]` 门控，
//   避免在非 Linux 目标上引用不存在的常量导致编译失败。
//
// 常量归属依据 libc 0.2.183 的 `src/windows/mod.rs` 中实际定义的 errno 列表。

macro_rules! __def_errno {
    (cross_platform: [$($cp:ident),* $(,)?], linux_only: [$($lo:ident),* $(,)?]) => {
        #[repr(i32)]
        #[derive(Debug, Clone, strum::EnumString, strum::Display, num_enum::TryFromPrimitive)]
        pub enum Errno {
            OK = 0,
            $($cp = libc::$cp,)*
            $(
                #[cfg(any(target_os = "linux", target_os = "android"))]
                $lo = libc::$lo,
            )*
        }
    };
}

// https://www.chromium.org/chromium-os/developer-library/reference/linux-constants/errnos/
__def_errno!(
    cross_platform: [
        EPERM, ENOENT, ESRCH, EINTR, EIO, ENXIO, E2BIG, ENOEXEC, EBADF, ECHILD,
        EAGAIN, ENOMEM, EACCES, EFAULT, EBUSY, EEXIST, EXDEV, ENODEV, ENOTDIR,
        EISDIR, EINVAL, ENFILE, EMFILE, ENOTTY, ETXTBSY, EFBIG, ENOSPC, ESPIPE,
        EROFS, EMLINK, EPIPE, EDOM, ERANGE, EDEADLK, ENAMETOOLONG, ENOLCK,
        ENOSYS, ENOTEMPTY, ELOOP, ENOMSG, EIDRM, ENOSTR, ENODATA, ETIME, ENOSR,
        ENOLINK, EPROTO, EBADMSG, EOVERFLOW, EILSEQ, ENOTSOCK, EDESTADDRREQ,
        EMSGSIZE, EPROTOTYPE, ENOPROTOOPT, EPROTONOSUPPORT, ENOTSUP, EAFNOSUPPORT,
        EADDRINUSE, EADDRNOTAVAIL, ENETDOWN, ENETUNREACH, ENETRESET,
        ECONNABORTED, ECONNRESET, ENOBUFS, EISCONN, ENOTCONN, ETIMEDOUT,
        ECONNREFUSED, EHOSTUNREACH, EALREADY, EINPROGRESS, ECANCELED,
        EOWNERDEAD, ENOTRECOVERABLE,
    ],
    linux_only: [
        ENOTBLK, ECHRNG, EL2NSYNC, EL3HLT, EL3RST, ELNRNG, EUNATCH, ENOCSI,
        EL2HLT, EBADE, EBADR, EXFULL, ENOANO, EBADRQC, EBADSLT, EBFONT,
        ENONET, ENOPKG, EREMOTE, EADV, ESRMNT, ECOMM, EMULTIHOP, EDOTDOT,
        ENOTUNIQ, EBADFD, EREMCHG, ELIBACC, ELIBBAD, ELIBSCN, ELIBMAX,
        ELIBEXEC, ERESTART, ESTRPIPE, EUSERS, ESOCKTNOSUPPORT, EPFNOSUPPORT,
        ESHUTDOWN, ETOOMANYREFS, EHOSTDOWN, ESTALE, EUCLEAN, ENOTNAM, ENAVAIL,
        EISNAM, EREMOTEIO, EDQUOT, ENOMEDIUM, EMEDIUMTYPE, ENOKEY, EKEYEXPIRED,
        EKEYREVOKED, EKEYREJECTED, ERFKILL, EHWPOISON,
    ]
);
