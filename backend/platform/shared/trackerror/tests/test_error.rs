use std::str::FromStr;

use trackerror::{
    AttachError, CatchError, IoError, IoErrorKind, Result, SerdeError, make_error, make_frame,
};

#[test]
fn test_make_error() {
    let _err: Result<()> = Err(make_error!(libc::EINVAL)).catch(|| make_frame!(""));
    let _err: Result<()> =
        Err(make_error!(libc::EINVAL, "msg")).catch(|| make_frame!("data {}", 1));
    let _err = make_error!(libc::EINVAL, "msg with data {}", 1);
    let _err = make_error!(libc::EINVAL => IoError::from(IoErrorKind::NotFound));
    let _err = make_error!(libc::EINVAL => IoError::from(IoErrorKind::NotFound), "msg");
    let _err =
        make_error!(libc::EINVAL => IoError::from(IoErrorKind::NotFound), "msg with data {}", 1);
}

#[test]
fn test_attach() {
    let res: Result<()> = Err(make_error!(libc::EINVAL))
        .attach("name1", 1)
        .attach_with("name2", || "xxx")
        .map_err(|err| make_error!(libc::EIO => err))
        .attach("name2", "zzz");
    let err = res.unwrap_err();
    println!("{}", err);
    println!("all attachment {:?}", err.get_attachment_all());
}

#[test]
fn test_serde_error() {
    let res: Result<()> =
        Err(make_error!(libc::EINVAL => IoError::from(IoErrorKind::ConnectionAborted)))
            .attach("name1", 1)
            .attach_with("name2", || "xxx")
            .map_err(|err| make_error!(libc::EIO => err))
            .attach("name2", "zzz");

    let err: SerdeError = res.unwrap_err().into();
    let s = err.to_string();
    println!("{}", s);
    let new_err = SerdeError::from_str(&s).unwrap();
    assert_eq!(err, new_err);
    println!("{}", new_err.get_error_msg());
}
