use std::{
    fs::File,
    io,
    os::windows::{ffi::OsStrExt, io::IntoRawHandle},
    path::Path,
};

pub(crate) fn close(file: File) -> io::Result<()> {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn CloseHandle(handle: *mut std::ffi::c_void) -> i32;
    }
    if unsafe { CloseHandle(file.into_raw_handle()) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

pub(crate) fn publish(source: &Path, destination: &Path, replace_file: bool) -> io::Result<()> {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn MoveFileExW(source: *const u16, destination: *const u16, flags: u32) -> i32;
    }
    let source: Vec<u16> = source.as_os_str().encode_wide().chain(Some(0)).collect();
    let destination: Vec<u16> = destination
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    // WRITE_THROUGH waits for namespace publication; COPY_ALLOWED and delayed moves are forbidden.
    let flags = 8 | u32::from(replace_file);
    if unsafe { MoveFileExW(source.as_ptr(), destination.as_ptr(), flags) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}
