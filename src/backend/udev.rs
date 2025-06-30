use {
    crate::backend::{self, Request},
    async_io::Async,
    futures_util::{TryStreamExt, future, ready},
    std::{
        ffi::{CString, OsStr},
        io,
        os::unix::ffi::OsStrExt,
        pin::Pin,
        task::Poll,
    },
    udev::{EventType, MonitorBuilder, MonitorSocket},
};

pub struct Backend {}

impl Backend {
    pub const CEC_VID: u16 = 0x2548;
    pub const CEC_PID: u16 = 0x1001;
    pub const CEC_PID2: u16 = 0x1002;

    fn parse_id(id: Option<&OsStr>) -> Option<u16> {
        id.and_then(OsStr::to_str)
            .and_then(|id| u16::from_str_radix(id, 16).ok())
    }
}

impl backend::Backend for Backend {
    type Context = ();
    type Error = io::Error;
    type Proxy<'a> = ();
    type Stream<'a> = Stream;

    async fn new(_: Self::Context) -> Result<Self, Self::Error> {
        Ok(Self {})
    }

    async fn split<'a>(&'a self) -> Result<(Self::Proxy<'a>, Self::Stream<'a>), Self::Error> {
        Ok((
            Self::Proxy::default(),
            Self::Stream {
                socket: AsyncMonitorSocket::new(
                    MonitorBuilder::new()?.match_subsystem("tty")?.listen()?,
                )?,
            },
        ))
    }
}

pub struct Stream {
    socket: AsyncMonitorSocket,
}

impl backend::Stream for Stream {
    type Error = io::Error;

    fn into_stream(self) -> impl futures_util::Stream<Item = Result<Request, Self::Error>> {
        fn map_event(event: udev::Event) -> Result<Option<Request>, io::Error> {
            Ok(
                match event
                    .parent_with_subsystem_devtype("usb", "usb_device")?
                    .map(|parent| {
                        (
                            Backend::parse_id(parent.attribute_value("idVendor")),
                            Backend::parse_id(parent.attribute_value("idProduct")),
                        )
                    }) {
                    Some((Some(Backend::CEC_VID), Some(Backend::CEC_PID | Backend::CEC_PID2))) => {
                        match event.event_type() {
                            EventType::Add => Some(Request::ResetDevice(Some(
                                // usb_device should always have a valid devnode
                                CString::new(event.devnode().unwrap().as_os_str().as_bytes())
                                    .unwrap(),
                            ))),
                            EventType::Remove => Some(Request::RemoveDevice(
                                // usb_device should always have a valid devnode
                                CString::new(event.devnode().unwrap().as_os_str().as_bytes())
                                    .unwrap(),
                            )),
                            _ => None,
                        }
                    }
                    _ => None,
                },
            )
        }

        self.socket
            .try_filter_map(|event| future::ready(map_event(event)))
    }
}

struct AsyncMonitorSocket {
    inner: Async<MonitorSocket>,
}

impl AsyncMonitorSocket {
    pub fn new(inner: MonitorSocket) -> io::Result<Self> {
        Ok(Self {
            inner: Async::new_nonblocking(inner)?,
        })
    }
}

impl futures_util::Stream for AsyncMonitorSocket {
    type Item = Result<udev::Event, io::Error>;

    fn poll_next(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Option<Self::Item>> {
        loop {
            if let Some(event) = self.inner.get_ref().iter().next() {
                return Poll::Ready(Some(Ok(event)));
            }

            ready!(self.inner.poll_readable(cx))?;
        }
    }
}
