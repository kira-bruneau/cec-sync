mod mpris;
mod systemd_logind;

use {
    crate::backend::{self, Event, Request},
    futures_util::{TryFutureExt, stream_select, try_join},
    zbus::Connection,
};

pub struct Backend {
    systemd_logind: systemd_logind::Backend,
    mpris: mpris::Backend,
}

impl backend::Backend for Backend {
    type Context = ();
    type Error = zbus::Error;
    type Proxy<'a> = Proxy<'a>;
    type Stream<'a> = Stream<'a>;

    async fn new(_: Self::Context) -> Result<Self, Self::Error> {
        let (systemd_logind, mpris) = try_join!(
            Connection::system().and_then(systemd_logind::Backend::new),
            Connection::session().and_then(mpris::Backend::new),
        )?;

        Ok(Self {
            systemd_logind,
            mpris,
        })
    }

    async fn split<'a>(&'a self) -> Result<(Self::Proxy<'a>, Self::Stream<'a>), Self::Error> {
        let ((mpris_proxy, mpris_stream), (systemd_logind_proxy, systemd_logind_stream)) =
            try_join!(self.mpris.split(), self.systemd_logind.split())?;

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

pub struct Proxy<'a> {
    mpris: mpris::Proxy<'a>,
    systemd_logind: systemd_logind::Proxy<'a>,
}

impl backend::Proxy for Proxy<'_> {
    type Error = zbus::Error;

    async fn event(&mut self, event: &Event) -> Result<(), Self::Error> {
        try_join!(self.mpris.event(event), self.systemd_logind.event(event))?;
        Ok(())
    }
}

pub struct Stream<'a> {
    mpris: mpris::Stream<'a>,
    systemd_logind: systemd_logind::Stream<'a>,
}

impl backend::Stream for Stream<'_> {
    type Error = zbus::Error;

    fn into_stream(self) -> impl futures_util::Stream<Item = Result<Request, Self::Error>> {
        stream_select!(self.mpris.into_stream(), self.systemd_logind.into_stream())
    }
}
