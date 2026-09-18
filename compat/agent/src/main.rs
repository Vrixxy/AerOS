#[cfg(target_os = "linux")]
use std::fs::{File, OpenOptions};
#[cfg(target_os = "linux")]
use std::io::{self, Read, Write};
#[cfg(target_os = "linux")]
use std::os::unix::process::CommandExt;
#[cfg(target_os = "linux")]
use std::process::{Command, Stdio};
#[cfg(target_os = "linux")]
use std::thread;
#[cfg(target_os = "linux")]
use std::time::Duration;

#[cfg(target_os = "linux")]
const PORT: &str = "/dev/virtio-ports/org.aeros.compat";
#[cfg(target_os = "linux")]
const MAGIC: [u8; 4] = *b"AERL";
#[cfg(target_os = "linux")]
const VERSION: u16 = 1;
#[cfg(target_os = "linux")]
const HEADER_BYTES: usize = 16;
#[cfg(target_os = "linux")]
const MAX_PAYLOAD: usize = 65_536;
const MAX_PATH: usize = 1_024;
const ALLOWED_PERMISSIONS: u32 = 0x3f;
#[cfg(target_os = "linux")]
const HELLO: u16 = 1;
#[cfg(target_os = "linux")]
const LAUNCH: u16 = 2;
#[cfg(target_os = "linux")]
const EXIT: u16 = 3;

#[cfg(target_os = "linux")]
fn main() {
    loop {
        if let Ok(port) = OpenOptions::new().read(true).write(true).open(PORT) {
            let _ = serve(port);
        }
        thread::sleep(Duration::from_millis(250));
    }
}

#[cfg(not(target_os = "linux"))]
fn main() {}

#[cfg(target_os = "linux")]
fn serve(mut port: File) -> io::Result<()> {
    write_frame(&mut port, HELLO, 1, b"debian13-amd64")?;
    loop {
        let frame = read_frame(&mut port)?;
        match frame.opcode {
            HELLO => write_frame(&mut port, HELLO, frame.request, b"aer-guest-agent/1")?,
            LAUNCH => {
                let Some(launch) = parse_launch(&frame.payload) else {
                    write_status(&mut port, frame.request, 22, 0)?;
                    continue;
                };
                let permission_text = launch.permissions.to_string();
                let child = Command::new(launch.path)
                    .uid(65_534)
                    .gid(65_534)
                    .env_clear()
                    .env("PATH", "/usr/local/bin:/usr/bin:/bin")
                    .env("HOME", "/tmp/aerapp")
                    .env("AEROS_PERMISSIONS", permission_text)
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn();
                let Ok(mut child) = child else {
                    write_status(&mut port, frame.request, 2, 0)?;
                    continue;
                };
                write_status(&mut port, frame.request, 0, child.id())?;
                let status = child.wait()?;
                let code = status
                    .code()
                    .map_or(128, |value| u32::try_from(value).unwrap_or(255));
                write_status(&mut port, frame.request, code, child.id())?;
                return write_frame(&mut port, EXIT, frame.request, &[]);
            }
            _ => write_status(&mut port, frame.request, 95, 0)?,
        }
    }
}

#[cfg(target_os = "linux")]
struct Frame {
    opcode: u16,
    request: u32,
    payload: Vec<u8>,
}

struct Launch<'a> {
    permissions: u32,
    path: &'a str,
}

#[cfg(target_os = "linux")]
fn read_frame(port: &mut File) -> io::Result<Frame> {
    let mut header = [0u8; HEADER_BYTES];
    port.read_exact(&mut header)?;
    let version = u16::from_le_bytes([header[4], header[5]]);
    let opcode = u16::from_le_bytes([header[6], header[7]]);
    let request = u32::from_le_bytes(header[8..12].try_into().unwrap_or_default());
    let payload_bytes = usize::try_from(u32::from_le_bytes(
        header[12..16].try_into().unwrap_or_default(),
    ))
    .unwrap_or(usize::MAX);
    if header[..4] != MAGIC
        || version != VERSION
        || request == 0
        || payload_bytes > MAX_PAYLOAD
        || !(HELLO..=EXIT).contains(&opcode)
    {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "invalid frame"));
    }
    let mut payload = vec![0u8; payload_bytes];
    port.read_exact(&mut payload)?;
    Ok(Frame {
        opcode,
        request,
        payload,
    })
}

#[cfg(target_os = "linux")]
fn write_frame(port: &mut File, opcode: u16, request: u32, payload: &[u8]) -> io::Result<()> {
    if payload.len() > MAX_PAYLOAD || request == 0 {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "invalid frame"));
    }
    let mut header = [0u8; HEADER_BYTES];
    header[..4].copy_from_slice(&MAGIC);
    header[4..6].copy_from_slice(&VERSION.to_le_bytes());
    header[6..8].copy_from_slice(&opcode.to_le_bytes());
    header[8..12].copy_from_slice(&request.to_le_bytes());
    header[12..16].copy_from_slice(&(payload.len() as u32).to_le_bytes());
    port.write_all(&header)?;
    port.write_all(payload)?;
    port.flush()
}

#[cfg(target_os = "linux")]
fn write_status(port: &mut File, request: u32, status: u32, pid: u32) -> io::Result<()> {
    let mut payload = [0u8; 8];
    payload[..4].copy_from_slice(&status.to_le_bytes());
    payload[4..].copy_from_slice(&pid.to_le_bytes());
    write_frame(port, LAUNCH, request, &payload)
}

fn parse_launch(payload: &[u8]) -> Option<Launch<'_>> {
    if payload.len() < 6 {
        return None;
    }
    let permissions = u32::from_le_bytes(payload[..4].try_into().ok()?);
    let path_bytes = usize::from(u16::from_le_bytes(payload[4..6].try_into().ok()?));
    if permissions & !ALLOWED_PERMISSIONS != 0
        || path_bytes == 0
        || path_bytes > MAX_PATH
        || payload.len() != 6usize.checked_add(path_bytes)?
    {
        return None;
    }
    let path = std::str::from_utf8(&payload[6..]).ok()?;
    if !path.starts_with('/')
        || !path.bytes().all(|byte| byte.is_ascii_graphic())
        || !path
            .split('/')
            .all(|component| component != "." && component != "..")
    {
        return None;
    }
    Some(Launch { permissions, path })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_bounded_launch() {
        let mut payload = vec![0u8; 6];
        let path = b"/usr/bin/true";
        payload[..4].copy_from_slice(&1u32.to_le_bytes());
        payload[4..6].copy_from_slice(&(path.len() as u16).to_le_bytes());
        payload.extend_from_slice(path);
        let launch = parse_launch(&payload).expect("valid launch");
        assert_eq!(launch.permissions, 1);
        assert_eq!(launch.path, "/usr/bin/true");
    }

    #[test]
    fn rejects_escape_and_unknown_permissions() {
        let traversal = launch_payload(0, "/usr/../bin/true");
        let excessive = launch_payload(0x40, "/usr/bin/true");
        assert!(parse_launch(&traversal).is_none());
        assert!(parse_launch(&excessive).is_none());
    }

    fn launch_payload(permissions: u32, path: &str) -> Vec<u8> {
        let mut payload = Vec::with_capacity(6 + path.len());
        payload.extend_from_slice(&permissions.to_le_bytes());
        payload.extend_from_slice(&(path.len() as u16).to_le_bytes());
        payload.extend_from_slice(path.as_bytes());
        payload
    }
}
