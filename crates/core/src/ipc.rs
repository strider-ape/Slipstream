//! Local control channel between the daemon and its clients (CLI, browser host, GUI):
//! one JSON message per line over a Unix socket (Linux) or a named pipe (Windows).
//! Only the same user can connect, and there is no TCP port (SPEC-browser-capture §1).

use std::io;
use std::path::{Path, PathBuf};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use tokio::io::{
    AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader, ReadHalf, WriteHalf,
};

/// Bumped when messages change incompatibly.
pub const PROTOCOL: u32 = 1;

/// Client → daemon.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Request {
    Ping,
    Add(AddRequest),
    List,
    Pause {
        id: i64,
    },
    Resume {
        id: i64,
    },
    Remove {
        id: i64,
        /// Also delete the file (or the unfinished part file) from disk.
        #[serde(default)]
        delete_file: bool,
    },
    /// Stream a `Downloads` snapshot twice a second until the client hangs up.
    Watch,
    Shutdown,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AddRequest {
    pub url: String,
    /// Folder to save into (default: the user's Downloads folder).
    #[serde(default)]
    pub dir: Option<PathBuf>,
    #[serde(default)]
    pub name: Option<String>,
    /// Headers to replay (cookies, referer, user agent...).
    #[serde(default)]
    pub headers: Vec<(String, String)>,
    /// Most connections (0 = default).
    #[serde(default)]
    pub connections: usize,
    /// Bytes/s (0 = unlimited).
    #[serde(default)]
    pub speed_limit: u64,
}

/// Daemon → client.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Response {
    Pong { version: String, protocol: u32 },
    Added { id: i64 },
    Downloads { downloads: Vec<DownloadInfo> },
    Ok,
    Error { message: String },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Queued,
    Downloading,
    Paused,
    Complete,
    Error,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Queued => "queued",
            Status::Downloading => "downloading",
            Status::Paused => "paused",
            Status::Complete => "complete",
            Status::Error => "error",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        [
            Self::Queued,
            Self::Downloading,
            Self::Paused,
            Self::Complete,
            Self::Error,
        ]
        .into_iter()
        .find(|v| v.as_str() == s)
    }
}

/// One row of the download list.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DownloadInfo {
    pub id: i64,
    pub url: String,
    pub dir: PathBuf,
    /// File name, once the server told us (or the user chose one).
    pub name: Option<String>,
    pub status: Status,
    pub size: Option<u64>,
    pub downloaded: u64,
    /// Bytes/s right now.
    pub speed: u64,
    pub connections: usize,
    pub error: Option<String>,
}

/// Where the daemon keeps its data and where it listens.
#[derive(Clone, Debug)]
pub struct Paths {
    pub data_dir: PathBuf,
    /// Unix socket path, or Windows pipe name.
    pub endpoint: String,
}

impl Paths {
    /// The usual per-user locations, or everything under `home` when given
    /// (tests, portable installs).
    pub fn new(home: Option<&Path>) -> io::Result<Self> {
        if let Some(home) = home {
            #[cfg(unix)]
            let endpoint = home.join("ctl.sock").to_string_lossy().into_owned();
            #[cfg(windows)]
            let endpoint = format!(
                r"\\.\pipe\slipstream-{:016x}",
                fnv1a(home.to_string_lossy().as_bytes())
            );
            return Ok(Self {
                data_dir: home.to_owned(),
                endpoint,
            });
        }
        Self::per_user()
    }

    /// Like [`Paths::new`], with `home` taken from `$SLIPSTREAM_HOME` if set.
    pub fn from_env() -> io::Result<Self> {
        Self::new(env_path("SLIPSTREAM_HOME").as_deref())
    }

    #[cfg(unix)]
    fn per_user() -> io::Result<Self> {
        let data = match env_path("XDG_DATA_HOME") {
            Some(d) => d,
            None => env_path("HOME").ok_or_else(no_home)?.join(".local/share"),
        }
        .join("slipstream");
        // $XDG_RUNTIME_DIR is per-user, private and wiped at logout: ideal for a socket.
        let run =
            env_path("XDG_RUNTIME_DIR").map_or_else(|| data.clone(), |r| r.join("slipstream"));
        let endpoint = run.join("ctl.sock").to_string_lossy().into_owned();
        Ok(Self {
            data_dir: data,
            endpoint,
        })
    }

    #[cfg(windows)]
    fn per_user() -> io::Result<Self> {
        let data = env_path("LOCALAPPDATA")
            .ok_or_else(no_home)?
            .join("Slipstream");
        let user = std::env::var("USERNAME").unwrap_or_default();
        let endpoint = format!(r"\\.\pipe\slipstream-{:016x}", fnv1a(user.as_bytes()));
        Ok(Self {
            data_dir: data,
            endpoint,
        })
    }

    /// The user's Downloads folder, the default place to save.
    pub fn downloads_dir() -> PathBuf {
        #[cfg(unix)]
        let home = std::env::var_os("HOME");
        #[cfg(windows)]
        let home = std::env::var_os("USERPROFILE");
        home.map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir)
            .join("Downloads")
    }
}

fn env_path(key: &str) -> Option<PathBuf> {
    std::env::var_os(key)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

fn no_home() -> io::Error {
    io::Error::new(io::ErrorKind::NotFound, "no home folder")
}

#[cfg(windows)]
fn fnv1a(data: &[u8]) -> u64 {
    data.iter().fold(0xcbf2_9ce4_8422_2325, |h, &b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// A connected socket or pipe, either side.
pub trait Duplex: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Duplex for T {}
pub type Stream = Box<dyn Duplex>;

/// Write one message as a JSON line.
pub async fn send<T: Serialize>(w: &mut (impl AsyncWrite + Unpin), msg: &T) -> io::Result<()> {
    let mut line = serde_json::to_vec(msg)?;
    line.push(b'\n');
    w.write_all(&line).await?;
    w.flush().await
}

/// Read one JSON-line message; `None` when the other side hung up.
pub async fn recv<T: DeserializeOwned>(
    r: &mut (impl AsyncBufReadExt + Unpin),
) -> io::Result<Option<T>> {
    let mut line = String::new();
    if r.read_line(&mut line).await? == 0 {
        return Ok(None);
    }
    Ok(Some(serde_json::from_str(&line)?))
}

/// The client end: send requests, read responses.
pub struct Client {
    reader: BufReader<ReadHalf<Stream>>,
    writer: WriteHalf<Stream>,
}

impl Client {
    pub async fn connect(paths: &Paths) -> io::Result<Self> {
        let (r, w) = tokio::io::split(connect(&paths.endpoint).await?);
        Ok(Self {
            reader: BufReader::new(r),
            writer: w,
        })
    }

    /// Send a request and wait for its answer.
    pub async fn call(&mut self, req: &Request) -> io::Result<Response> {
        send(&mut self.writer, req).await?;
        self.next()
            .await?
            .ok_or_else(|| io::ErrorKind::UnexpectedEof.into())
    }

    /// The next message (for `Watch`, which keeps sending).
    pub async fn next(&mut self) -> io::Result<Option<Response>> {
        recv(&mut self.reader).await
    }
}

#[cfg(unix)]
async fn connect(endpoint: &str) -> io::Result<Stream> {
    Ok(Box::new(tokio::net::UnixStream::connect(endpoint).await?))
}

#[cfg(windows)]
async fn connect(endpoint: &str) -> io::Result<Stream> {
    use tokio::net::windows::named_pipe::ClientOptions;
    const ERROR_PIPE_BUSY: i32 = 231;
    // All pipe instances can be busy for a moment while the server makes a new one.
    for _ in 0..50 {
        match ClientOptions::new().open(endpoint) {
            Ok(client) => return Ok(Box::new(client)),
            Err(e) if e.raw_os_error() == Some(ERROR_PIPE_BUSY) => {
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
            Err(e) => return Err(e),
        }
    }
    Err(io::ErrorKind::TimedOut.into())
}

/// The daemon end: accepts connections from the same user only.
pub struct Listener {
    #[cfg(unix)]
    inner: tokio::net::UnixListener,
    /// Owner of the socket's folder: the only user allowed in.
    #[cfg(unix)]
    owner: u32,
    #[cfg(windows)]
    inner: windows::PipeListener,
}

impl Listener {
    /// Start listening. Fails with `AddrInUse` if a daemon is already running.
    pub async fn bind(paths: &Paths) -> io::Result<Self> {
        Self::bind_endpoint(&paths.endpoint).await
    }

    #[cfg(unix)]
    async fn bind_endpoint(endpoint: &str) -> io::Result<Self> {
        use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
        let path = Path::new(endpoint);
        let dir = path.parent().unwrap_or(Path::new("."));
        // A private folder (rwx for us only): nobody else can even reach the socket.
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)?;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
        if path.exists() {
            if tokio::net::UnixStream::connect(path).await.is_ok() {
                return Err(io::Error::new(
                    io::ErrorKind::AddrInUse,
                    "daemon already running",
                ));
            }
            std::fs::remove_file(path)?; // left over from a crash
        }
        let inner = tokio::net::UnixListener::bind(path)?;
        Ok(Self {
            inner,
            owner: std::fs::metadata(dir)?.uid(),
        })
    }

    #[cfg(windows)]
    async fn bind_endpoint(endpoint: &str) -> io::Result<Self> {
        Ok(Self {
            inner: windows::PipeListener::bind(endpoint)?,
        })
    }

    #[cfg(unix)]
    pub async fn accept(&mut self) -> io::Result<Stream> {
        loop {
            let (stream, _) = self.inner.accept().await?;
            // Belt and braces next to the private folder: check who is calling.
            match stream.peer_cred() {
                Ok(cred) if cred.uid() == self.owner => return Ok(Box::new(stream)),
                _ => continue,
            }
        }
    }

    #[cfg(windows)]
    pub async fn accept(&mut self) -> io::Result<Stream> {
        self.inner.accept().await
    }
}

#[cfg(windows)]
mod windows {
    //! Named pipe whose access list allows only the current user.

    use std::ffi::c_void;
    use std::io;
    use std::ptr::null_mut;

    use tokio::net::windows::named_pipe::{NamedPipeServer, ServerOptions};
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, LocalFree};
    use windows_sys::Win32::Security::Authorization::{
        ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
        SDDL_REVISION_1,
    };
    use windows_sys::Win32::Security::{
        GetTokenInformation, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER, TokenUser,
    };
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    use super::Stream;

    pub struct PipeListener {
        name: String,
        next: NamedPipeServer,
        attrs: Box<SECURITY_ATTRIBUTES>,
    }

    // SAFETY: the security attributes are only read by the OS while creating pipes.
    unsafe impl Send for PipeListener {}
    unsafe impl Sync for PipeListener {}

    impl PipeListener {
        pub fn bind(name: &str) -> io::Result<Self> {
            let mut attrs = Box::new(SECURITY_ATTRIBUTES {
                nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
                lpSecurityDescriptor: user_only_descriptor()?,
                bInheritHandle: 0,
            });
            let ptr = (&mut *attrs as *mut SECURITY_ATTRIBUTES).cast::<c_void>();
            // first_pipe_instance: fails if another daemon already owns the name.
            let first = unsafe {
                ServerOptions::new()
                    .first_pipe_instance(true)
                    .reject_remote_clients(true)
                    .create_with_security_attributes_raw(name, ptr)
            }
            .map_err(|e| match e.kind() {
                io::ErrorKind::PermissionDenied => {
                    io::Error::new(io::ErrorKind::AddrInUse, "daemon already running")
                }
                _ => e,
            })?;
            Ok(Self {
                name: name.to_owned(),
                next: first,
                attrs,
            })
        }

        pub async fn accept(&mut self) -> io::Result<Stream> {
            self.next.connect().await?;
            // Make the next instance before handing this one out, so clients never
            // find the name missing.
            let ptr = (&mut *self.attrs as *mut SECURITY_ATTRIBUTES).cast::<c_void>();
            let fresh = unsafe {
                ServerOptions::new()
                    .reject_remote_clients(true)
                    .create_with_security_attributes_raw(&self.name, ptr)?
            };
            Ok(Box::new(std::mem::replace(&mut self.next, fresh)))
        }
    }

    impl Drop for PipeListener {
        fn drop(&mut self) {
            // SAFETY: allocated by ConvertStringSecurityDescriptorToSecurityDescriptorW.
            unsafe { LocalFree(self.attrs.lpSecurityDescriptor) };
        }
    }

    /// "D:P(A;;GA;;;<my SID>)": a protected access list granting everything to the
    /// current user and nothing to anyone else.
    fn user_only_descriptor() -> io::Result<*mut c_void> {
        let sid = current_user_sid()?;
        let sddl: Vec<u16> = format!("D:P(A;;GA;;;{sid})")
            .encode_utf16()
            .chain([0])
            .collect();
        let mut sd = null_mut();
        // SAFETY: valid NUL-terminated string and out pointer.
        let ok = unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                SDDL_REVISION_1,
                &mut sd,
                null_mut(),
            )
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(sd)
    }

    fn current_user_sid() -> io::Result<String> {
        // SAFETY: standard token query; every handle/buffer is checked and freed.
        unsafe {
            let mut token: HANDLE = null_mut();
            if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
                return Err(io::Error::last_os_error());
            }
            let mut len = 0u32;
            GetTokenInformation(token, TokenUser, null_mut(), 0, &mut len);
            let mut buf = vec![0u8; len as usize];
            let ok = GetTokenInformation(token, TokenUser, buf.as_mut_ptr().cast(), len, &mut len);
            CloseHandle(token);
            if ok == 0 {
                return Err(io::Error::last_os_error());
            }
            let user = &*(buf.as_ptr() as *const TOKEN_USER);
            let mut wide = null_mut();
            if ConvertSidToStringSidW(user.User.Sid, &mut wide) == 0 {
                return Err(io::Error::last_os_error());
            }
            let len = (0..).take_while(|&i| *wide.add(i) != 0).count();
            let sid = String::from_utf16_lossy(std::slice::from_raw_parts(wide, len));
            LocalFree(wide.cast());
            Ok(sid)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_round_trip_as_json_lines() {
        let req = Request::Add(AddRequest {
            url: "https://x/a.zip".into(),
            ..Default::default()
        });
        let json = serde_json::to_string(&req).unwrap();
        assert!(
            json.starts_with(r#"{"cmd":"add","url":"https://x/a.zip""#),
            "{json}"
        );
        assert_eq!(serde_json::from_str::<Request>(&json).unwrap(), req);
        let pause: Request = serde_json::from_str(r#"{"cmd":"pause","id":3}"#).unwrap();
        assert_eq!(pause, Request::Pause { id: 3 });
        assert_eq!(
            serde_json::to_string(&Response::Ok).unwrap(),
            r#"{"type":"ok"}"#
        );
    }

    #[test]
    fn status_names() {
        for s in [
            Status::Queued,
            Status::Downloading,
            Status::Paused,
            Status::Complete,
            Status::Error,
        ] {
            assert_eq!(Status::parse(s.as_str()), Some(s));
        }
    }

    #[tokio::test]
    async fn client_talks_to_listener() {
        let home = std::env::temp_dir().join(format!("slipstream-ipc-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(&home).unwrap();
        let paths = Paths::new(Some(&home)).unwrap();
        let mut listener = Listener::bind(&paths).await.unwrap();
        let again = Listener::bind(&paths).await;
        assert_eq!(
            again.err().map(|e| e.kind()),
            Some(io::ErrorKind::AddrInUse)
        );

        let server = tokio::spawn(async move {
            let (r, mut w) = tokio::io::split(listener.accept().await.unwrap());
            let mut r = BufReader::new(r);
            let req: Request = recv(&mut r).await.unwrap().unwrap();
            assert_eq!(req, Request::Ping);
            send(
                &mut w,
                &Response::Pong {
                    version: "t".into(),
                    protocol: PROTOCOL,
                },
            )
            .await
            .unwrap();
        });
        let mut client = Client::connect(&paths).await.unwrap();
        let pong = client.call(&Request::Ping).await.unwrap();
        assert_eq!(
            pong,
            Response::Pong {
                version: "t".into(),
                protocol: PROTOCOL
            }
        );
        server.await.unwrap();
    }
}
