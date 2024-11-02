use {
    crate::backend::{self, dbus, udev, unix_socket, wayland, Event, Request},
    futures_util::{stream_select, try_join, TryFutureExt, TryStreamExt},
};

pub struct Backend {
    unix_socket: unix_socket::Backend,
    dbus: dbus::Backend,
    udev: udev::Backend,
    wayland: wayland::Backend,
}

impl backend::Backend for Backend {
    type Error = Error;

    type Proxy = Proxy;

    type Stream = Stream;

    async fn new() -> Result<Self, Self::Error> {
        let (unix_socket, dbus, udev, wayland) = try_join!(
            unix_socket::Backend::new().map_err(Error::UnixSocket),
            dbus::Backend::new().map_err(Error::Dbus),
            udev::Backend::new().map_err(Error::Udev),
            wayland::Backend::new().map_err(Error::Wayland),
        )?;

        Ok(Self {
            unix_socket,
            dbus,
            udev,
            wayland,
        })
    }

    async fn split(self) -> Result<(Self::Proxy, Self::Stream), Self::Error> {
        let (
            (_, unix_socket_stream),
            (dbus_proxy, dbus_stream),
            (_, udev_stream),
            (wayland_proxy, _),
        ) = try_join!(
            self.unix_socket.split().map_err(Error::UnixSocket),
            self.dbus.split().map_err(Error::Dbus),
            self.udev.split().map_err(Error::Udev),
            self.wayland.split().map_err(Error::Wayland)
        )?;

        Ok((
            Self::Proxy {
                dbus: dbus_proxy,
                wayland: wayland_proxy,
            },
            Self::Stream {
                unix_socket: unix_socket_stream,
                dbus: dbus_stream,
                udev: udev_stream,
            },
        ))
    }
}

pub struct Proxy {
    dbus: <dbus::Backend as backend::Backend>::Proxy,
    wayland: <wayland::Backend as backend::Backend>::Proxy,
}

impl backend::Proxy for Proxy {
    type Error = Error;

    async fn event(&mut self, event: &Event) -> Result<(), Self::Error> {
        try_join!(
            self.dbus.event(&event).map_err(Error::Dbus),
            self.wayland.event(&event).map_err(Error::Wayland)
        )?;

        Ok(())
    }
}

pub struct Stream {
    unix_socket: <unix_socket::Backend as backend::Backend>::Stream,
    dbus: <dbus::Backend as backend::Backend>::Stream,
    udev: <udev::Backend as backend::Backend>::Stream,
}

impl backend::Stream for Stream {
    type Error = Error;

    fn into_stream(self) -> impl futures_util::Stream<Item = Result<Request, Self::Error>> {
        stream_select!(
            self.unix_socket.into_stream().map_err(Error::UnixSocket),
            self.udev.into_stream().map_err(Error::Udev),
            self.dbus.into_stream().map_err(Error::Dbus),
        )
    }
}

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("unix socket: {0}")]
    UnixSocket(<unix_socket::Backend as backend::Backend>::Error),
    #[error("dbus: {0}")]
    Dbus(<dbus::Backend as backend::Backend>::Error),
    #[error("udev: {0}")]
    Udev(<udev::Backend as backend::Backend>::Error),
    #[error("wayland: {0}")]
    Wayland(<wayland::Backend as backend::Backend>::Error),
}
