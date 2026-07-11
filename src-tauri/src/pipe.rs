//! Named-pipe client to the elevated helper + UAC-elevated spawn of that helper.
use std::io;
use std::path::PathBuf;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::windows::named_pipe::{ClientOptions, NamedPipeClient};
use vpn_ipc::{ClientCommand, PIPE_NAME};

/// Path to the bundled helper exe (next to the GUI exe).
fn helper_path() -> io::Result<PathBuf> {
    let exe = std::env::current_exe()?;
    let dir = exe.parent().ok_or_else(|| io::Error::other("no exe dir"))?;
    Ok(dir.join("yellow-vpn-helper.exe"))
}

/// Launch the helper elevated (UAC). Returns once ShellExecute has been issued;
/// the caller then polls the pipe until the helper has created it.
#[cfg(windows)]
pub fn spawn_helper_elevated() -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_HIDE;

    let path = helper_path()?;
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
    let verb: Vec<u16> = "runas".encode_utf16().chain(std::iter::once(0)).collect();

    let result = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            verb.as_ptr(),
            wide.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            SW_HIDE,
        )
    };
    // ShellExecuteW returns a value > 32 on success.
    if (result as isize) <= 32 {
        return Err(io::Error::other("elevation cancelled or failed (UAC)"));
    }
    Ok(())
}

#[cfg(not(windows))]
pub fn spawn_helper_elevated() -> io::Result<()> {
    Err(io::Error::other("helper spawn is Windows-only"))
}

/// Connect to the helper pipe, spawning the elevated helper first if it is absent.
/// Retries the pipe connection for a few seconds to cover UAC + helper startup.
pub async fn connect_with_spawn() -> io::Result<NamedPipeClient> {
    // Try an existing helper first.
    if let Ok(c) = ClientOptions::new().open(PIPE_NAME) {
        return Ok(c);
    }
    spawn_helper_elevated()?;
    // Poll for up to ~15s while the user clicks through UAC.
    for _ in 0..150 {
        match ClientOptions::new().open(PIPE_NAME) {
            Ok(c) => return Ok(c),
            Err(_) => tokio::time::sleep(Duration::from_millis(100)).await,
        }
    }
    Err(io::Error::other("helper did not come up (pipe never appeared)"))
}

/// Send one command as a JSON line.
pub async fn send_command(
    writer: &mut tokio::io::WriteHalf<NamedPipeClient>,
    cmd: &ClientCommand,
) -> io::Result<()> {
    let mut line = serde_json::to_string(cmd).map_err(io::Error::other)?;
    line.push('\n');
    writer.write_all(line.as_bytes()).await?;
    writer.flush().await
}

/// Split a connected pipe into a writer and a line-reader.
pub fn split(
    client: NamedPipeClient,
) -> (
    tokio::io::WriteHalf<NamedPipeClient>,
    tokio::io::Lines<BufReader<tokio::io::ReadHalf<NamedPipeClient>>>,
) {
    let (r, w) = tokio::io::split(client);
    (w, BufReader::new(r).lines())
}
