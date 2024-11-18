use {
    crate::{
        backend::{self, Request},
        meta_command::MetaCommand,
    },
    async_io::Async,
    async_net::unix::UnixDatagram,
    futures_util::{ready, StreamExt},
    postcard::experimental::max_size::MaxSize,
    std::{
        env, fs,
        io::{self},
        path::PathBuf,
        pin::Pin,
        sync::Arc,
        task::Poll,
    },
};

pub struct Backend {
    socket: UnixDatagram,
}

impl Backend {
    pub fn path() -> PathBuf {
        let mut socket_path = env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| env::temp_dir());

        socket_path.push("cec-sync");
        socket_path
    }
}

impl backend::Backend for Backend {
    type Context = ();
    type Error = Error;
    type Proxy<'a> = ();
    type Stream<'a> = Stream;

    async fn new(_: Self::Context) -> Result<Self, Self::Error> {
        let path = Self::path();
        let _ = fs::remove_file(&path);
        Ok(Self {
            socket: UnixDatagram::bind(&path)?,
        })
    }

    async fn split<'a>(&'a self) -> Result<(Self::Proxy<'a>, Self::Stream<'a>), Self::Error> {
        Ok((
            Self::Proxy::default(),
            Self::Stream {
                socket: self.socket.clone(),
            },
        ))
    }
}

pub struct Stream {
    socket: UnixDatagram,
}

impl backend::Stream for Stream {
    type Error = Error;

    fn into_stream(self) -> impl futures_util::Stream<Item = Result<Request, Self::Error>> {
        MetaCommandStream {
            inner: self.socket.into(),
        }
        .map(|result| result.map(Request::MetaCommand))
    }
}

struct MetaCommandStream {
    inner: Arc<Async<std::os::unix::net::UnixDatagram>>,
}

impl futures_util::Stream for MetaCommandStream {
    type Item = Result<MetaCommand, Error>;

    fn poll_next(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Option<Self::Item>> {
        loop {
            let mut buf = [0u8; MetaCommand::POSTCARD_MAX_SIZE];
            match self.inner.get_ref().recv(&mut buf) {
                Ok(0) => return Poll::Ready(None),
                Ok(_) => {
                    return Poll::Ready(Some(
                        postcard::from_bytes(&buf).map_err(Error::InvalidCommand),
                    ))
                }
                Err(err) if err.kind() == io::ErrorKind::WouldBlock => (),
                Err(err) => return Poll::Ready(Some(Err(Error::Io(err)))),
            };

            ready!(self.inner.poll_readable(cx))?;
        }
    }
}

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error("invalid command: {0}")]
    InvalidCommand(postcard::Error),
}
