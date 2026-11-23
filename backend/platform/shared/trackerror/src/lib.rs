pub mod errno;

use std::{
    any::Any,
    borrow::Cow,
    collections::HashMap,
    ffi::CStr,
    fmt::{Debug, Display, Write},
    str::FromStr,
};

use num_enum::TryFromPrimitive;
use serde::{Deserialize, Serialize, de::DeserializeOwned};

pub type IoError = std::io::Error;
pub type IoErrorKind = std::io::ErrorKind;

pub fn encode_error_kind(kind: IoErrorKind) -> i32 {
    use std::io::ErrorKind::*;

    match kind {
        ArgumentListTooLong => libc::E2BIG,
        AddrInUse => libc::EADDRINUSE,
        AddrNotAvailable => libc::EADDRNOTAVAIL,
        ResourceBusy => libc::EBUSY,
        ConnectionAborted => libc::ECONNABORTED,
        ConnectionRefused => libc::ECONNREFUSED,
        ConnectionReset => libc::ECONNRESET,
        Deadlock => libc::EDEADLK,
        // EDQUOT 与 ESTALE 仅 Linux libc 提供；Windows 下降级为 EIO。
        QuotaExceeded => {
            #[cfg(not(target_os = "windows"))]
            {
                libc::EDQUOT
            }
            #[cfg(target_os = "windows")]
            {
                libc::EIO
            }
        }
        AlreadyExists => libc::EEXIST,
        FileTooLarge => libc::EFBIG,
        HostUnreachable => libc::EHOSTUNREACH,
        Interrupted => libc::EINTR,
        InvalidInput => libc::EINVAL,
        IsADirectory => libc::EISDIR,
        NotFound => libc::ENOENT,
        OutOfMemory => libc::ENOMEM,
        StorageFull => libc::ENOSPC,
        Unsupported => libc::ENOSYS,
        TooManyLinks => libc::EMLINK,
        InvalidFilename => libc::ENAMETOOLONG,
        NetworkDown => libc::ENETDOWN,
        NetworkUnreachable => libc::ENETUNREACH,
        NotConnected => libc::ENOTCONN,
        NotADirectory => libc::ENOTDIR,
        DirectoryNotEmpty => libc::ENOTEMPTY,
        BrokenPipe => libc::EPIPE,
        ReadOnlyFilesystem => libc::EROFS,
        NotSeekable => libc::ESPIPE,
        StaleNetworkFileHandle => {
            // ESTALE 仅 Linux libc 提供；Windows 下降级为 EIO。
            #[cfg(not(target_os = "windows"))]
            {
                libc::ESTALE
            }
            #[cfg(target_os = "windows")]
            {
                libc::EIO
            }
        }
        TimedOut => libc::ETIMEDOUT,
        ExecutableFileBusy => libc::ETXTBSY,
        CrossesDevices => libc::EXDEV,
        // Unsupported => EOPNOTSUPP,
        PermissionDenied => libc::EACCES,
        WouldBlock => libc::EAGAIN,
        // All other error is EIO
        _ => libc::EIO,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Frame {
    pub file: Cow<'static, str>,
    pub line: u32,
    pub module_path: Cow<'static, str>,
    pub msg: Option<Cow<'static, str>>,
}

impl Frame {
    pub fn new(
        file: Cow<'static, str>,
        line: u32,
        module_path: Cow<'static, str>,
        msg: Option<Cow<'static, str>>,
    ) -> Frame {
        Frame {
            file,
            line,
            module_path,
            msg,
        }
    }
}

impl Display for Frame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "at {}:{}, {}", self.file, self.line, self.module_path,)?;
        if let Some(msg) = &self.msg {
            write!(f, "\n  {}", msg)?;
        }
        Ok(())
    }
}

pub trait ErrorCategory: Debug + Send + Sync {
    fn name(&self) -> &'static str;
    // Map code to system errno
    fn as_errno(&self, error: &Error) -> i32;
    // Name of code
    fn code_name(&self, error: &Error) -> Cow<'static, str>;
    // String repr for this error
    fn error_desc(&self, error: &Error) -> Cow<'static, str>;
}

#[derive(Debug)]
pub struct Error {
    pub code: i32,
    pub category: &'static dyn ErrorCategory,
    pub backtrace: Vec<Frame>,
    pub source: Option<Box<dyn std::error::Error + Send + Sync + 'static>>,
    pub caused_by: Option<Box<Error>>,
    pub attachment: Option<HashMap<String, serde_json::Value>>,
}

impl Error {
    pub fn new<E: std::error::Error + Any + Send + Sync + 'static>(
        category: &'static dyn ErrorCategory,
        code: i32,
        source: Option<E>,
        frame: Option<Frame>,
    ) -> Error {
        if let Some(src) = &source
            && (src as &dyn Any).is::<Error>()
        {
            // Wrap this error
            let caused_by = (Box::new(source.unwrap()) as Box<dyn Any>)
                .downcast::<Error>()
                .unwrap();
            Error {
                code,
                category,
                backtrace: {
                    let mut bt = Vec::with_capacity(8);
                    if let Some(frame) = frame {
                        bt.push(frame);
                    }
                    bt
                },
                source: None,
                caused_by: Some(caused_by),
                attachment: None,
            }
        } else {
            Error {
                code,
                category,
                backtrace: {
                    let mut bt = Vec::with_capacity(8);
                    if let Some(frame) = frame {
                        bt.push(frame);
                    }
                    bt
                },
                source: source.map(|v| Box::new(v) as _),
                caused_by: None,
                attachment: None,
            }
        }
    }

    pub fn new_system<E: std::error::Error + Send + Sync + 'static>(
        code: i32,
        source: Option<E>,
        frame: Option<Frame>,
    ) -> Error {
        Error::new(&SYSTEM_CATEGORY, code, source, frame)
    }

    pub fn push_frame(&mut self, frame: Frame) -> &mut Error {
        self.backtrace.push(frame);
        self
    }

    pub fn set_source<E: std::error::Error + Send + Sync + 'static>(
        &mut self,
        source: E,
    ) -> &mut Error {
        self.source = Some(Box::new(source) as _);
        self
    }

    pub fn as_errno(&self) -> i32 {
        self.category.as_errno(self)
    }

    pub fn errno() -> i32 {
        #[cfg(any(target_os = "linux", target_os = "android"))]
        {
            unsafe { *libc::__errno_location() }
        }
        #[cfg(all(unix, not(any(target_os = "linux", target_os = "android"))))]
        {
            unsafe { *libc::__error() }
        }
        #[cfg(target_os = "windows")]
        {
            // Windows 无 errno_location；用最近一次系统错误码降级。
            std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
        }
    }

    pub fn get_attachment_all(&self) -> HashMap<String, serde_json::Value> {
        let mut res = HashMap::new();

        fn __merge(err: &Error, res: &mut HashMap<String, serde_json::Value>) {
            if let Some(caused_by) = &err.caused_by {
                __merge(caused_by, res);
            }
            if let Some(attachment) = &err.attachment {
                res.extend(attachment.clone());
            }
        }

        __merge(self, &mut res);

        res
    }

    pub fn get_attachment<T: DeserializeOwned>(&self, name: &str) -> Option<T> {
        fn __get(err: &Error, name: &str) -> Option<serde_json::Value> {
            if let Some(attachment) = &err.attachment
                && attachment.contains_key(name)
            {
                attachment.get(name).cloned()
            } else if let Some(caused_by) = &err.caused_by {
                __get(&caused_by, name)
            } else {
                None
            }
        }

        __get(self, name).and_then(|v| serde_json::from_value(v).ok())
    }

    pub fn get_attachment_or_default<T: DeserializeOwned + Default>(&self, name: &str) -> T {
        self.get_attachment(name).unwrap_or_default()
    }

    pub fn get_code(&self) -> i32 {
        self.code
    }

    pub fn get_code_name(&self) -> Cow<'static, str> {
        self.category.code_name(self)
    }

    pub fn get_error_desc(&self) -> Cow<'static, str> {
        self.category.error_desc(self)
    }

    pub fn get_error_msg(&self) -> String {
        format!("{}", self)
    }

    pub fn get_category_name(&self) -> &'static str {
        self.category.name()
    }
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

impl Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(
            f,
            "{} - {}:{}({})",
            self.category.error_desc(self),
            self.category.name(),
            self.category.code_name(self),
            self.code,
        )?;
        if let Some(source) = &self.source {
            writeln!(f, "Source: {}", source)?;
            writeln!(f, "Source detail: {:?}", source)?;
        }
        if let Some(attachment) = &self.attachment {
            writeln!(f, "Attachment:")?;
            for (name, value) in attachment {
                writeln!(
                    f,
                    "  {}: {}",
                    name,
                    serde_json::to_string(value).unwrap_or_else(|err| format!("{err}"))
                )?;
            }
        }

        if !self.backtrace.is_empty() {
            writeln!(f, "\nBacktrace:")?;
        }
        for (idx, frame) in self.backtrace.iter().enumerate().rev() {
            writeln!(f, "{}: {}", idx, frame)?;
        }

        if let Some(caused_by) = &self.caused_by {
            write!(f, "\nCaused by: {}", caused_by)?;
        }

        Ok(())
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.source.as_ref().map(|item| item.as_ref() as _)
    }
}

impl From<IoError> for Error {
    fn from(value: IoError) -> Self {
        let code = if let Some(code) = value.raw_os_error() {
            code
        } else {
            encode_error_kind(value.kind())
        };
        let mut error = Error::new_system(code, Option::<IoError>::None, None);
        if value.get_ref().is_some() {
            error.source = value.into_inner();
        } else {
            error.set_source(value);
        }
        error
    }
}

impl From<std::string::FromUtf8Error> for Error {
    fn from(error: std::string::FromUtf8Error) -> Self {
        Error::new_system(libc::EINVAL, Some(error), None)
    }
}

impl From<std::str::Utf8Error> for Error {
    fn from(error: std::str::Utf8Error) -> Self {
        Error::new_system(libc::EINVAL, Some(error), None)
    }
}

impl From<serde_json::Error> for Error {
    fn from(error: serde_json::Error) -> Self {
        Error::new_system(libc::EINVAL, Some(error), None)
    }
}

impl From<std::fmt::Error> for Error {
    fn from(error: std::fmt::Error) -> Self {
        Error::new_system(libc::EINVAL, Some(error), None)
    }
}

pub trait CatchError<T> {
    type Error;
    fn catch<F>(self, f: F) -> Result<T, Self::Error>
    where
        F: FnOnce() -> Frame;
}

impl<T, E> CatchError<T> for std::result::Result<T, E>
where
    E: Into<Error>,
{
    type Error = Error;

    fn catch<F>(self, f: F) -> Result<T>
    where
        F: FnOnce() -> Frame,
    {
        self.map_err(move |error| {
            let mut error = error.into();
            error.push_frame(f());
            error
        })
    }
}

impl<T> CatchError<T> for Option<T> {
    type Error = Error;

    fn catch<F>(self, f: F) -> Result<T>
    where
        F: FnOnce() -> Frame,
    {
        self.ok_or_else(|| Error::new_system(libc::ENOENT, Option::<Error>::None, Some(f())))
    }
}

impl<T> CatchError<T> for Result<T, SerdeError> {
    type Error = SerdeError;

    fn catch<F>(self, f: F) -> Result<T, Self::Error>
    where
        F: FnOnce() -> Frame,
    {
        self.map_err(|mut error| {
            error.backtrace.push(f());
            error
        })
    }
}

pub trait AttachError<T> {
    fn attach<V>(self, name: impl Into<String>, value: V) -> Result<T>
    where
        V: Serialize;
    fn attach_with<F, V>(self, name: impl Into<String>, f: F) -> Result<T>
    where
        F: FnOnce() -> V,
        V: Serialize;
}

impl<T> AttachError<T> for Result<T> {
    fn attach<V>(self, name: impl Into<String>, value: V) -> Result<T>
    where
        V: Serialize,
    {
        self.map_err(|mut err| {
            err.attachment.get_or_insert_default().insert(
                name.into(),
                serde_json::to_value(&value).unwrap_or_default(),
            );
            err
        })
    }

    fn attach_with<F, V>(self, name: impl Into<String>, f: F) -> Result<T>
    where
        F: FnOnce() -> V,
        V: Serialize,
    {
        self.map_err(|mut err| {
            err.attachment
                .get_or_insert_default()
                .insert(name.into(), serde_json::to_value(&f()).unwrap_or_default());
            err
        })
    }
}

#[derive(Debug)]
pub struct SystemCategory;

impl ErrorCategory for SystemCategory {
    fn name(&self) -> &'static str {
        "system"
    }

    fn as_errno(&self, error: &Error) -> i32 {
        error.code
    }

    fn code_name(&self, error: &Error) -> Cow<'static, str> {
        let code = crate::errno::Errno::try_from_primitive(error.code)
            .unwrap_or(crate::errno::Errno::EINVAL);
        code.to_string().into()
    }

    fn error_desc(&self, error: &Error) -> Cow<'static, str> {
        generic_strerror(error.code).into()
    }
}

pub static SYSTEM_CATEGORY: SystemCategory = SystemCategory;

pub fn generic_strerror(errno: i32) -> String {
    #[cfg(not(target_os = "windows"))]
    {
        unsafe {
            let mut buf: Vec<u8> = Vec::with_capacity(128);
            buf.set_len(128);
            buf[0] = b'\0';
            libc::strerror_r(errno, buf.as_ptr() as _, buf.len());
            str::from_utf8(CStr::from_ptr(buf.as_ptr() as _).to_bytes())
                .unwrap()
                .to_owned()
        }
    }
    #[cfg(target_os = "windows")]
    {
        // Windows 无 strerror_r；优先用标准库的 OS 错误描述，无法识别时退化为 "errno <n>"。
        match std::io::Error::from_raw_os_error(errno) {
            err if err.raw_os_error().is_some() => {
                let msg = format!("{}", err);
                if msg.is_empty() {
                    format!("errno {}", errno)
                } else {
                    msg
                }
            }
            _ => format!("errno {}", errno),
        }
    }
}

#[doc(hidden)]
#[macro_export]
macro_rules! __trackerror_source {
    () => { Option::<$crate::Error>::None };
    ($source:expr) => { Some($source) };
}

#[doc(hidden)]
#[macro_export]
macro_rules! __trackerror_msg {
    () => { None };
    ($msg:literal) => { Some(::std::borrow::Cow::Borrowed($msg)) };
    ($($fmt_arg:expr),+) => { Some(::std::borrow::Cow::Owned(format!($($fmt_arg),+))) };
}

#[macro_export]
macro_rules! make_error {
    ($code:expr $(=> $source:expr)? $(, $($msg:tt)+)?) => {
        $crate::make_error_impl!(
            &$crate::SYSTEM_CATEGORY,
            $code,
            $crate::__trackerror_source!($($source)?),
            $crate::__trackerror_msg!($($($msg)+)?)
        )
    };
}

#[macro_export]
macro_rules! make_error_category {
    ($category:expr, $code:expr $(=> $source:expr)? $(, $($msg:tt)+)?) => {
        $crate::make_error_impl!(
            $category,
            $code,
            $crate::__trackerror_source!($($source)?),
            $crate::__trackerror_msg!($($($msg)+)?)
        )
    };
}

#[macro_export]
macro_rules! make_error_impl {
    ($category:expr, $code:expr, $source:expr, $msg:expr) => {{
        let frame = $crate::Frame::new(
            std::borrow::Cow::Borrowed(file!()),
            line!(),
            std::borrow::Cow::Borrowed(module_path!()),
            $msg,
        );
        $crate::Error::new($category, $code, $source, Some(frame))
    }};
}

#[macro_export]
macro_rules! make_frame {
    () => {
        $crate::Frame::new(
            std::borrow::Cow::Borrowed(file!()),
            line!(),
            std::borrow::Cow::Borrowed(module_path!()),
            None,
        )
    };
    ($msg:literal) => {
        $crate::Frame::new(
            std::borrow::Cow::Borrowed(file!()),
            line!(),
            std::borrow::Cow::Borrowed(module_path!()),
            Some(std::borrow::Cow::Borrowed($msg)),
        )
    };
    ($($fmt_arg:expr),*) => {
        $crate::Frame::new(
            std::borrow::Cow::Borrowed(file!()),
            line!(),
            std::borrow::Cow::Borrowed(module_path!()),
            Some(std::borrow::Cow::Owned(format!($($fmt_arg),*))),
        )
    };
}

// Used for Termination and display backtrace on failed.
pub struct TerminationError(pub Error);

impl Debug for TerminationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl Display for TerminationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<Error> for TerminationError {
    fn from(value: Error) -> Self {
        TerminationError(value)
    }
}

impl std::error::Error for TerminationError {}

pub type TerminationResult<T, E = TerminationError> = std::result::Result<T, E>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SerdeError {
    pub code: i32,
    pub code_name: String,
    pub category_name: String,
    pub error_desc: String,
    pub backtrace: Vec<Frame>,
    pub source: Option<String>,
    pub source_detail: Option<String>,
    pub caused_by: Option<Box<SerdeError>>,
    pub attachment: Option<HashMap<String, serde_json::Value>>,
}

impl SerdeError {
    pub fn get_error_msg(&self) -> String {
        let mut msg = String::new();
        let f = &mut msg;

        writeln!(
            f,
            "{} - {}:{}({})",
            self.error_desc, self.category_name, self.code_name, self.code,
        )
        .unwrap();
        if let Some(source) = &self.source {
            writeln!(f, "Source: {}", source).unwrap();
            writeln!(
                f,
                "Source detail: {}",
                self.source_detail.as_ref().unwrap_or(&String::new())
            )
            .unwrap();
        }
        if let Some(attachment) = &self.attachment {
            writeln!(f, "Attachment:").unwrap();
            for (name, value) in attachment {
                writeln!(
                    f,
                    "  {}: {}",
                    name,
                    serde_json::to_string(value).unwrap_or_else(|err| format!("{err}"))
                )
                .unwrap();
            }
        }

        if !self.backtrace.is_empty() {
            writeln!(f, "\nBacktrace:").unwrap();
        }
        for (idx, frame) in self.backtrace.iter().enumerate().rev() {
            writeln!(f, "{}: {}", idx, frame).unwrap();
        }

        if let Some(caused_by) = &self.caused_by {
            write!(f, "\nCaused by: {}", caused_by.get_error_msg()).unwrap();
        }

        msg
    }

    pub fn get_attachment_all(&self) -> HashMap<String, serde_json::Value> {
        let mut res = HashMap::new();

        fn __merge(err: &SerdeError, res: &mut HashMap<String, serde_json::Value>) {
            if let Some(caused_by) = &err.caused_by {
                __merge(caused_by, res);
            }
            if let Some(attachment) = &err.attachment {
                res.extend(attachment.clone());
            }
        }

        __merge(self, &mut res);

        res
    }

    pub fn get_attachment<T: DeserializeOwned>(&self, name: &str) -> Option<T> {
        fn __get(err: &SerdeError, name: &str) -> Option<serde_json::Value> {
            if let Some(attachment) = &err.attachment
                && attachment.contains_key(name)
            {
                attachment.get(name).cloned()
            } else if let Some(caused_by) = &err.caused_by {
                __get(&caused_by, name)
            } else {
                None
            }
        }

        __get(self, name).and_then(|v| serde_json::from_value(v).ok())
    }
}

pub type SerdeResult<T, E = SerdeError> = std::result::Result<T, E>;

impl From<Error> for SerdeError {
    fn from(value: Error) -> Self {
        let code_name = value.get_code_name();
        let error_desc = value.get_error_desc();
        SerdeError {
            code: value.code,
            code_name: code_name.into_owned(),
            category_name: value.category.name().to_owned(),
            error_desc: error_desc.into_owned(),
            backtrace: value.backtrace.into_iter().map(|v| v.into()).collect(),
            source: value.source.as_ref().map(|v| format!("{}", v)),
            source_detail: value.source.as_ref().map(|v| format!("{:?}", v)),
            caused_by: value.caused_by.map(|v| Box::new((*v).into())),
            attachment: value.attachment,
        }
    }
}

impl Display for SerdeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            serde_json::to_string(self).map_err(|_err| std::fmt::Error)?
        )
    }
}

impl std::error::Error for SerdeError {}

impl FromStr for SerdeError {
    type Err = Error;
    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        serde_json::from_str(s).catch(|| make_frame!())
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_error() {
        let io_error: Result<(), IoError> = Err(IoError::from(IoErrorKind::NotFound));

        let error = io_error
            .catch(|| make_frame!())
            .catch(|| make_frame!("xxx"));
        println!("{}", error.unwrap_err());
    }

    #[test]
    #[ignore]
    fn test_termination_error() -> TerminationResult<()> {
        let io_error: Result<(), IoError> = Err(IoError::from(IoErrorKind::NotFound));

        let error = io_error
            .catch(|| make_frame!())
            .catch(|| make_frame!("xxx"));
        Ok(error?)
    }

    #[test]
    fn test_make_error() {
        let _err = make_error!(libc::EINVAL);
        let _err = make_error!(libc::EINVAL, "msg");
        let _err = make_error!(libc::EINVAL, "msg with data {}", 1);
        let _err = make_error!(libc::EINVAL => IoError::from(IoErrorKind::NotFound));
        let _err = make_error!(libc::EINVAL => IoError::from(IoErrorKind::NotFound), "msg");
        let _err = make_error!(libc::EINVAL => IoError::from(IoErrorKind::NotFound), "msg with data {}", 1);
    }
}
