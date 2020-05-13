use libc::chmod;
use std::ffi::CString;
use std::io::{self, Error};
use futures::Stream;
use tokio::prelude::*;
use tokio::net::{UnixListener, UnixStream};
use std::path::Path;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::mem::MaybeUninit;
use std::os::unix::io::FromRawFd;
use std::os::unix::io::RawFd;

/// Socket permissions and ownership on UNIX
pub struct SecurityAttributes {
    // read/write permissions for owner, group and others in unix octal.
    mode: Option<u16>
}

impl SecurityAttributes {
    /// New default security attributes.
    pub fn empty() -> Self {
        SecurityAttributes {
            mode: None
        }
    }

    /// New security attributes that allow everyone to connect.
    pub fn allow_everyone_connect(mut self) -> io::Result<Self> {
        self.mode = Some(0o777);
        Ok(self)
    }

    /// Set a custom permission on the socket
    pub fn set_mode(mut self, mode: u16) -> io::Result<Self> {
        self.mode = Some(mode);
        Ok(self)
    }

    /// New security attributes that allow everyone to create.
    pub fn allow_everyone_create() -> io::Result<Self> {
        Ok(SecurityAttributes {
            mode: None
        })
    }

    /// called in unix, after server socket has been created
    /// will apply security attributes to the socket.
     pub(crate) unsafe fn apply_permissions(&self, path: &str) -> io::Result<()> {
        let path = CString::new(path.to_owned())?;
         if let Some(mode) = self.mode {
            if chmod(path.as_ptr(), mode as _) == -1 {
                return Err(Error::last_os_error())
            }
        }

        Ok(())
    }
}

/// Endpoint implementation for unix systems
pub struct Endpoint {
    path: String,
    security_attributes: SecurityAttributes,
    unix_listener: Option<UnixListener>,
}

impl Endpoint {
    /// Stream of incoming connections
    pub fn incoming(&mut self) -> io::Result<impl Stream<Item = tokio::io::Result<impl AsyncRead + AsyncWrite>> + '_> {
        if self.unix_listener.is_none() {
            self.unix_listener = Some(self.inner()?);
        }
        unsafe {
            // the call to bind in `inner()` creates the file
            // `apply_permission()` will set the file permissions.
            self.security_attributes.apply_permissions(&self.path)?;
        };
        // for some unknown reason, the Incoming struct borrows the listener
        // so we have to hold on to the listener in order to return the Incoming struct.
        Ok(self.unix_listener.as_mut().unwrap().incoming())
    }

    /// Inner platform-dependant state of the endpoint
    fn inner(&self) -> io::Result<UnixListener> {
        UnixListener::bind(&self.path)
    }

    /// Constructs a new instance of Self from the given raw file descriptor.
    pub fn from_raw_fd(fd: RawFd) -> Self {
        let sys_unix_listener: std::os::unix::net::UnixListener;
        unsafe {
            sys_unix_listener = std::os::unix::net::UnixListener::from_raw_fd(fd);
        }
        let unix_listener = UnixListener::from_std(sys_unix_listener).unwrap();
        Endpoint {
            path: "".to_string(),
            security_attributes: SecurityAttributes::empty(),
            unix_listener: Some(unix_listener),
        }
    }

    /// Set security attributes for the connection
    pub fn set_security_attributes(&mut self, security_attributes: SecurityAttributes) {
        self.security_attributes = security_attributes;
    }

    /// Make new connection using the provided path and running event pool
    pub async fn connect<P: AsRef<Path>>(path: P) -> io::Result<Connection> {
        Ok(Connection::wrap(UnixStream::connect(path.as_ref()).await?))
    }

    /// Returns the path of the endpoint.
    pub fn path(&self) -> &str {
        &self.path
    }

    /// New IPC endpoint at the given path
    pub fn new(path: String) -> Self {
        Endpoint {
            path,
            security_attributes: SecurityAttributes::empty(),
            unix_listener: None,
        }
    }
}

impl Drop for Endpoint {
    fn drop(&mut self) {
        use std::fs;
        if let Ok(()) = fs::remove_file(Path::new(&self.path)) {
            log::trace!("Removed socket file at: {}", self.path)
        }
    }
}

/// IPC connection.
pub struct Connection {
    inner: UnixStream,
}

impl Connection {
    fn wrap(stream: UnixStream) -> Self {
        Self { inner: stream }
    }

    /// Constructs a new instance of Self from the given raw file descriptor.
    pub fn from_raw_fd(fd: RawFd) -> Self {
        let std_stream: std::os::unix::net::UnixStream;
        unsafe {
            std_stream = std::os::unix::net::UnixStream::from_raw_fd(fd);
        }
        let stream = UnixStream::from_std(std_stream).unwrap();

        Self { inner: stream }
    }
}

impl AsyncRead for Connection {
    unsafe fn prepare_uninitialized_buffer(&self, buf: &mut [MaybeUninit<u8>]) -> bool {
        self.inner.prepare_uninitialized_buffer(buf)
    }

    fn poll_read(
        self: Pin<&mut Self>,
        ctx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        let this = Pin::into_inner(self);
        Pin::new(&mut this.inner).poll_read(ctx, buf)
    }
}

impl AsyncWrite for Connection {
    fn poll_write(
        self: Pin<&mut Self>,
        ctx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, io::Error>> {
        let this = Pin::into_inner(self);
        Pin::new(&mut this.inner).poll_write(ctx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, ctx: &mut Context<'_>) -> Poll<Result<(), io::Error>> {
        let this = Pin::into_inner(self);
        Pin::new(&mut this.inner).poll_flush(ctx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, ctx: &mut Context<'_>) -> Poll<Result<(), io::Error>> {
        let this = Pin::into_inner(self);
        Pin::new(&mut this.inner).poll_shutdown(ctx)
    }
}
