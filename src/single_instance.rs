//! Reuse one running viewer per Windows session.
//!
//! A named mutex decides which process owns the viewer. Later launches connect
//! to a named pipe served by the owner and ask it to focus the window or open a
//! path. The pipe is session-scoped so separate users never see each other.

use std::ffi::{OsStr, OsString};
use std::io;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
#[cfg(test)]
use std::sync::atomic::AtomicUsize;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_ACCESS_DENIED, ERROR_ALREADY_EXISTS, ERROR_BROKEN_PIPE,
    ERROR_FILE_NOT_FOUND, ERROR_PIPE_BUSY, ERROR_PIPE_CONNECTED, GENERIC_WRITE, GetLastError,
    HANDLE, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FlushFileBuffers, OPEN_EXISTING, PIPE_ACCESS_INBOUND, ReadFile, WriteFile,
};
use windows_sys::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_READMODE_BYTE,
    PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_WAIT, WaitNamedPipeW,
};
use windows_sys::Win32::System::RemoteDesktop::ProcessIdToSessionId;
use windows_sys::Win32::System::Threading::{CreateMutexW, GetCurrentProcessId};
use winit::event_loop::EventLoopProxy;

use crate::app::UserEvent;

const MUTEX_NAME: &str = r"Local\picasa-rs-single-instance";
const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
const MAX_BODY: usize = 128 * 1024;
const KIND_FOCUS: u8 = 0;
const KIND_OPEN: u8 = 1;
const HEADER_LEN: usize = 5;

#[cfg(test)]
static PIPE_TEST_COUNTER: AtomicUsize = AtomicUsize::new(0);

/// What a later launch asks the running viewer to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Request {
    /// Bring the window to the foreground without changing the photo.
    Focus,
    /// Open a file or directory, replacing the current folder listing.
    Open(PathBuf),
}

/// Keeps the single-instance mutex alive for as long as the process runs.
pub struct SingleInstance {
    handle: HANDLE,
}

impl SingleInstance {
    /// Returns `Some` when this process owns the viewer and `None` when another
    /// process already does. The mutex is released by the OS on process exit,
    /// so a crashed viewer never blocks the next launch.
    pub fn acquire() -> Result<Option<Self>> {
        let name = wide(MUTEX_NAME);
        // SAFETY: `name` is a valid null-terminated UTF-16 string and the
        // security attributes are optional.
        let handle = unsafe { CreateMutexW(std::ptr::null(), 0, name.as_ptr()) };
        if handle.is_null() {
            let error = unsafe { GetLastError() };
            if error == ERROR_ACCESS_DENIED {
                // A viewer at a different integrity level owns the name; let
                // the forwarding attempt decide what happens next.
                return Ok(None);
            }
            bail!("could not create the single-instance mutex (error {error})");
        }

        if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
            // SAFETY: the handle came from CreateMutexW and is still open.
            unsafe { CloseHandle(handle) };
            return Ok(None);
        }

        Ok(Some(Self { handle }))
    }
}

impl Drop for SingleInstance {
    fn drop(&mut self) {
        // SAFETY: the handle is owned by this guard and is closed once.
        unsafe { CloseHandle(self.handle) };
    }
}

/// Start the pipe server that receives requests from later launches.
pub fn start_server(proxy: EventLoopProxy<UserEvent>) -> Result<()> {
    let name = pipe_name()?;
    thread::Builder::new()
        .name("picasa-rs-single-instance".to_string())
        .spawn(move || {
            serve(&name, |request| {
                let event = match request {
                    Request::Focus => UserEvent::Focus,
                    Request::Open(path) => UserEvent::OpenPath(path),
                };
                proxy.send_event(event).is_ok()
            });
        })
        .context("could not start the single-instance listener")?;
    Ok(())
}

/// Ask the running viewer to handle `request`.
pub fn forward(request: &Request) -> Result<()> {
    forward_to(&pipe_name()?, request)
}

fn pipe_name() -> Result<String> {
    let mut session = 0u32;
    // SAFETY: `session` is a valid out pointer.
    if unsafe { ProcessIdToSessionId(GetCurrentProcessId(), &mut session) } == 0 {
        bail!(
            "could not determine the Windows session id (error {})",
            unsafe { GetLastError() }
        );
    }
    Ok(format!(r"\\.\pipe\picasa-rs-{session}"))
}

fn serve(name: &str, mut deliver: impl FnMut(Request) -> bool) {
    let name = wide(name);
    loop {
        // SAFETY: the name is a valid null-terminated UTF-16 string; the
        // security attributes are optional.
        let handle = unsafe {
            CreateNamedPipeW(
                name.as_ptr(),
                PIPE_ACCESS_INBOUND,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                1,
                0,
                4096,
                0,
                std::ptr::null(),
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            log::warn!(
                "single-instance pipe could not be created (error {})",
                unsafe { GetLastError() }
            );
            return;
        }

        // SAFETY: `handle` is a pipe server created above.
        let connected = unsafe { ConnectNamedPipe(handle, std::ptr::null_mut()) };
        let connect_error = if connected == 0 {
            unsafe { GetLastError() }
        } else {
            0
        };
        if connected == 0 && connect_error != ERROR_PIPE_CONNECTED {
            // SAFETY: `handle` is still owned here.
            unsafe { CloseHandle(handle) };
            if connect_error == 232 {
                // ERROR_NO_DATA: the client went away between create and
                // connect. Recreate the pipe and keep waiting.
                continue;
            }
            log::warn!("single-instance pipe connect failed (error {connect_error})");
            thread::sleep(Duration::from_millis(100));
            continue;
        }

        let request = read_request(handle);
        // SAFETY: `handle` is a connected pipe server owned here.
        unsafe {
            DisconnectNamedPipe(handle);
            CloseHandle(handle);
        }

        match request {
            Ok(Some(request)) => {
                if !deliver(request) {
                    return;
                }
            }
            Ok(None) => {}
            Err(error) => log::debug!("single-instance request was ignored: {error}"),
        }
    }
}

fn forward_to(name: &str, request: &Request) -> Result<()> {
    let frame = encode(request)?;
    let name = wide(name);
    let deadline = Instant::now() + CONNECT_TIMEOUT;

    loop {
        // SAFETY: the name is a valid null-terminated UTF-16 string and the
        // optional arguments are null.
        let handle = unsafe {
            CreateFileW(
                name.as_ptr(),
                GENERIC_WRITE,
                0,
                std::ptr::null(),
                OPEN_EXISTING,
                0,
                std::ptr::null_mut(),
            )
        };
        if handle != INVALID_HANDLE_VALUE {
            let result = write_all(handle, &frame);
            // SAFETY: `handle` came from CreateFileW and is closed once.
            unsafe {
                FlushFileBuffers(handle);
                CloseHandle(handle);
            }
            return result.context("could not send the request to the running viewer");
        }

        let error = unsafe { GetLastError() };
        if !is_retryable(error) || Instant::now() >= deadline {
            bail!("could not connect to the running viewer (error {error})");
        }
        if error == ERROR_PIPE_BUSY {
            // SAFETY: the name is valid and the timeout is bounded.
            unsafe { WaitNamedPipeW(name.as_ptr(), 250) };
        } else {
            thread::sleep(Duration::from_millis(50));
        }
    }
}

fn is_retryable(error: u32) -> bool {
    error == ERROR_FILE_NOT_FOUND || error == ERROR_PIPE_BUSY
}

fn read_request(handle: HANDLE) -> io::Result<Option<Request>> {
    let mut header = [0u8; HEADER_LEN];
    let mut filled = 0;
    while filled < HEADER_LEN {
        let mut read = 0u32;
        // SAFETY: the buffer is valid for the requested length.
        let ok = unsafe {
            ReadFile(
                handle,
                header[filled..].as_mut_ptr(),
                (HEADER_LEN - filled) as u32,
                &mut read,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 {
            let error = io::Error::last_os_error();
            if filled == 0 && error.raw_os_error() == Some(ERROR_BROKEN_PIPE as i32) {
                return Ok(None);
            }
            return Err(error);
        }
        if read == 0 {
            if filled == 0 {
                return Ok(None);
            }
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "the request header was truncated",
            ));
        }
        filled += read as usize;
    }

    let (kind, length) = parse_header(&header).map_err(invalid_data)?;
    let mut body = vec![0u8; length];
    read_exact(handle, &mut body)?;
    decode_body(kind, &body).map(Some).map_err(invalid_data)
}

fn read_exact(handle: HANDLE, buffer: &mut [u8]) -> io::Result<()> {
    let mut filled = 0;
    while filled < buffer.len() {
        let mut read = 0u32;
        // SAFETY: the buffer is valid for the requested length.
        let ok = unsafe {
            ReadFile(
                handle,
                buffer[filled..].as_mut_ptr(),
                (buffer.len() - filled) as u32,
                &mut read,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        if read == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "the request body was truncated",
            ));
        }
        filled += read as usize;
    }
    Ok(())
}

fn write_all(handle: HANDLE, buffer: &[u8]) -> io::Result<()> {
    let mut written = 0;
    while written < buffer.len() {
        let mut count = 0u32;
        // SAFETY: the buffer is valid for the requested length.
        let ok = unsafe {
            WriteFile(
                handle,
                buffer[written..].as_ptr(),
                (buffer.len() - written) as u32,
                &mut count,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        if count == 0 {
            return Err(io::Error::new(
                io::ErrorKind::WriteZero,
                "the pipe accepted no bytes",
            ));
        }
        written += count as usize;
    }
    Ok(())
}

fn encode(request: &Request) -> Result<Vec<u8>> {
    let (kind, path) = match request {
        Request::Focus => (KIND_FOCUS, None),
        Request::Open(path) => (KIND_OPEN, Some(path)),
    };
    let body = match path {
        Some(path) => path_bytes(path)?,
        None => Vec::new(),
    };
    let length = u32::try_from(body.len()).context("the request path is too long")?;
    let mut frame = Vec::with_capacity(HEADER_LEN + body.len());
    frame.push(kind);
    frame.extend_from_slice(&length.to_le_bytes());
    frame.extend_from_slice(&body);
    Ok(frame)
}

fn path_bytes(path: &Path) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    for unit in path.as_os_str().encode_wide() {
        bytes.extend_from_slice(&unit.to_le_bytes());
    }
    if bytes.len() > MAX_BODY {
        bail!("the path is too long to forward");
    }
    Ok(bytes)
}

fn parse_header(header: &[u8; HEADER_LEN]) -> Result<(u8, usize)> {
    let length = u32::from_le_bytes([header[1], header[2], header[3], header[4]]) as usize;
    if length > MAX_BODY {
        bail!("the request is too large");
    }
    if header[0] != KIND_FOCUS && header[0] != KIND_OPEN {
        bail!("unknown request kind {}", header[0]);
    }
    if header[0] == KIND_FOCUS && length != 0 {
        bail!("a focus request cannot carry a path");
    }
    Ok((header[0], length))
}

fn decode_body(kind: u8, body: &[u8]) -> Result<Request> {
    match kind {
        KIND_FOCUS => Ok(Request::Focus),
        KIND_OPEN => {
            if body.is_empty() || !body.len().is_multiple_of(2) {
                bail!("the request path is malformed");
            }
            let units: Vec<u16> = body
                .as_chunks::<2>()
                .0
                .iter()
                .map(|pair| u16::from_le_bytes(*pair))
                .collect();
            Ok(Request::Open(PathBuf::from(OsString::from_wide(&units))))
        }
        other => bail!("unknown request kind {other}"),
    }
}

fn invalid_data(error: anyhow::Error) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error.to_string())
}

fn wide(text: impl AsRef<OsStr>) -> Vec<u16> {
    text.as_ref().encode_wide().chain(Some(0)).collect()
}

#[cfg(test)]
mod tests {
    use super::{
        HEADER_LEN, KIND_FOCUS, KIND_OPEN, MAX_BODY, PIPE_TEST_COUNTER, Request, decode_body,
        encode, forward_to, is_retryable, parse_header, serve,
    };
    use std::path::PathBuf;
    use std::sync::atomic::Ordering;
    use std::sync::mpsc;
    use std::thread;
    use std::time::Duration;
    use windows_sys::Win32::Foundation::{
        ERROR_ACCESS_DENIED, ERROR_FILE_NOT_FOUND, ERROR_PIPE_BUSY,
    };

    #[test]
    fn focus_requests_round_trip() {
        let frame = encode(&Request::Focus).unwrap();
        assert_eq!(frame.len(), HEADER_LEN);
        let header: [u8; HEADER_LEN] = frame[..HEADER_LEN].try_into().unwrap();
        let (kind, length) = parse_header(&header).unwrap();
        assert_eq!(kind, KIND_FOCUS);
        assert_eq!(length, 0);
        assert_eq!(
            decode_body(kind, &frame[HEADER_LEN..]).unwrap(),
            Request::Focus
        );
    }

    #[test]
    fn unicode_paths_round_trip() {
        let path = PathBuf::from(r"C:\Фотографии\naïve 写真.jpg");
        let frame = encode(&Request::Open(path.clone())).unwrap();
        let header: [u8; HEADER_LEN] = frame[..HEADER_LEN].try_into().unwrap();
        let (kind, length) = parse_header(&header).unwrap();
        assert_eq!(kind, KIND_OPEN);
        assert_eq!(length, frame.len() - HEADER_LEN);
        assert_eq!(
            decode_body(kind, &frame[HEADER_LEN..]).unwrap(),
            Request::Open(path)
        );
    }

    #[test]
    fn malformed_headers_are_rejected() {
        assert!(parse_header(&[9, 0, 0, 0, 0]).is_err());
        assert!(parse_header(&[KIND_FOCUS, 1, 0, 0, 0]).is_err());
        let oversized = (MAX_BODY as u32 + 1).to_le_bytes();
        let mut header = [0u8; HEADER_LEN];
        header[0] = KIND_OPEN;
        header[1..].copy_from_slice(&oversized);
        assert!(parse_header(&header).is_err());
        assert!(decode_body(KIND_OPEN, &[0x41]).is_err());
        assert!(decode_body(KIND_OPEN, &[]).is_err());
    }

    #[test]
    fn only_missing_or_busy_pipes_are_retried() {
        assert!(is_retryable(ERROR_FILE_NOT_FOUND));
        assert!(is_retryable(ERROR_PIPE_BUSY));
        assert!(!is_retryable(ERROR_ACCESS_DENIED));
    }

    #[test]
    fn a_running_server_receives_forwarded_requests() {
        let counter = PIPE_TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
        let name = format!(r"\\.\pipe\picasa-rs-test-{}-{counter}", std::process::id());
        let server_name = name.clone();
        let (sender, receiver) = mpsc::channel();
        let server = thread::spawn(move || {
            serve(&server_name, |request| {
                sender.send(request).unwrap();
                false
            });
        });

        let path = PathBuf::from(r"C:\Photos\naïve 写真.jpg");
        forward_to(&name, &Request::Open(path.clone())).unwrap();
        assert_eq!(
            receiver.recv_timeout(Duration::from_secs(5)).unwrap(),
            Request::Open(path)
        );
        server.join().unwrap();
    }
}
