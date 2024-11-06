mod mpris;
mod systemd_logind;

use {
    crate::backend::{self, Event, Request},
    futures_util::{stream_select, try_join, TryFutureExt},
    zbus::Connection,
};

pub struct Backend {
    systemd_logind: systemd_logind::Backend,
    mpris: mpris::Backend,
}

impl backend::Backend for Backend {
    type Error = zbus::Error;

    type Proxy = Proxy;

    type Stream = Stream;

    async fn new() -> Result<Self, Self::Error> {
        let (systemd_logind, mpris) = try_join!(
            Connection::system().and_then(systemd_logind::new),
            Connection::session().and_then(mpris::new),
        )?;

        Ok(Self {
            systemd_logind,
            mpris,
        })
    }

    async fn split(self) -> Result<(Self::Proxy, Self::Stream), Self::Error> {
        let ((mpris_proxy, mpris_stream), (systemd_logind_proxy, systemd_logind_stream)) = try_join!(
            mpris::split(self.mpris),
            systemd_logind::split(self.systemd_logind),
        )?;

        Ok((
            Self::Proxy {
                mpris: mpris_proxy,
                systemd_logind: systemd_logind_proxy,
            },
            Self::Stream {
                mpris: mpris_stream,
                systemd_logind: systemd_logind_stream,
            },
        ))
    }
}

pub struct Proxy {
    mpris: mpris::Proxy,
    systemd_logind: systemd_logind::Proxy,
}

impl backend::Proxy for Proxy {
    type Error = zbus::Error;

    async fn event(&mut self, event: &Event) -> Result<(), Self::Error> {
        try_join!(self.mpris.event(event), self.systemd_logind.event(event))?;
        Ok(())
    }
}

pub struct Stream {
    mpris: mpris::Stream,
    systemd_logind: systemd_logind::Stream,
}

impl backend::Stream for Stream {
    type Error = zbus::Error;

    fn into_stream(self) -> impl futures_util::Stream<Item = Result<Request, Self::Error>> {
        stream_select!(self.mpris.into_stream(), self.systemd_logind.into_stream())
    }
}
