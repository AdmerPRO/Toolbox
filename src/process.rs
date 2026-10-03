use std::{
    io,
    path::Path,
    process::{Output, Stdio},
    time::Duration,
};
use tokio::{io::AsyncReadExt, process::Command};

// A timeout or disconnected client must also stop FFmpeg spawned by yt-dlp.
#[cfg(unix)]
struct ProcessTree(u32);
#[cfg(unix)]
impl Drop for ProcessTree {
    fn drop(&mut self) {
        unsafe {
            libc::kill(-(self.0 as i32), libc::SIGKILL);
        }
    }
}

#[cfg(windows)]
struct ProcessTree(usize);
#[cfg(windows)]
impl ProcessTree {
    fn new(child: &tokio::process::Child) -> io::Result<Self> {
        use windows_sys::Win32::{Foundation::CloseHandle, System::JobObjects::*};
        unsafe {
            let handle = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if handle.is_null() {
                return Err(io::Error::last_os_error());
            }
            let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let process = child
                .raw_handle()
                .ok_or_else(|| io::Error::other("Missing process handle"))?;
            if SetInformationJobObject(
                handle,
                JobObjectExtendedLimitInformation,
                &limits as *const _ as *const _,
                std::mem::size_of_val(&limits) as u32,
            ) == 0
                || AssignProcessToJobObject(handle, process as _) == 0
            {
                let error = io::Error::last_os_error();
                CloseHandle(handle);
                return Err(error);
            }
            Ok(Self(handle as usize))
        }
    }
}
#[cfg(windows)]
impl Drop for ProcessTree {
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.0 as _);
        }
    }
}

async fn capture(reader: impl tokio::io::AsyncRead + Unpin, limit: u64) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader.take(limit + 1).read_to_end(&mut bytes).await?;
    if bytes.len() as u64 > limit {
        return Err(io::Error::other("Process output limit exceeded"));
    }
    Ok(bytes)
}

pub async fn run(
    command: &mut Command,
    seconds: u64,
    directory: Option<&Path>,
    max_bytes: u64,
) -> io::Result<Output> {
    command
        .kill_on_drop(true)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    command.process_group(0);
    let mut child = command.spawn()?;
    #[cfg(unix)]
    let _tree = ProcessTree(
        child
            .id()
            .ok_or_else(|| io::Error::other("Missing process ID"))?,
    );
    #[cfg(windows)]
    let _tree = ProcessTree::new(&child)?;
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    let operation = async {
        let output = async {
            let (status, stdout, stderr) = tokio::try_join!(
                child.wait(),
                capture(stdout, 8 * 1024 * 1024),
                capture(stderr, 256 * 1024)
            )?;
            Ok::<_, io::Error>(Output {
                status,
                stdout,
                stderr,
            })
        };
        tokio::pin!(output);
        let mut interval = tokio::time::interval(Duration::from_millis(250));
        loop {
            tokio::select! {
                result = &mut output => {
                    if let Some(path) = directory {
                        check(path, max_bytes).await?;
                    }
                    return result;
                },
                _ = interval.tick(), if directory.is_some() => {
                    check(directory.unwrap(), max_bytes).await?;
                }
            }
        }
    };
    tokio::time::timeout(Duration::from_secs(seconds), operation)
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "Media process timed out"))?
}

async fn check(path: &Path, max_bytes: u64) -> io::Result<()> {
    let path = path.to_owned();
    tokio::task::spawn_blocking(move || {
        let size = crate::storage::directory_size(&path).map_err(io::Error::other)?;
        if size > max_bytes {
            return Err(io::Error::other("Job disk limit exceeded"));
        }
        crate::storage::check_free_space(std::path::Path::new("storage")).map_err(io::Error::other)
    })
    .await
    .map_err(io::Error::other)?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn rejects_excessive_process_output() {
        assert_eq!(capture(&b"1234"[..], 4).await.unwrap(), b"1234");
        assert!(capture(&b"12345"[..], 4).await.is_err());
    }

    #[tokio::test]
    async fn stops_a_process_tree_at_its_deadline() {
        #[cfg(windows)]
        let mut command = {
            let mut c = Command::new("cmd");
            c.args(["/C", "ping -n 30 127.0.0.1 > nul"]);
            c
        };
        #[cfg(unix)]
        let mut command = {
            let mut c = Command::new("sh");
            c.args(["-c", "sleep 30 & wait"]);
            c
        };
        let start = std::time::Instant::now();
        let error = run(&mut command, 1, None, 0).await.unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert!(start.elapsed() < Duration::from_secs(5));
    }
}
