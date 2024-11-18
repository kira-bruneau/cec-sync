use {
    crate::{
        backend::{self, Event, Request},
        meta_command::{MetaCommand, Power},
    },
    cec_rs::{CecCommand, CecOpcode},
    futures_util::StreamExt,
    logind_zbus::manager::{ManagerProxy, PrepareForSleep, PrepareForSleepStream},
    zbus::proxy::CacheProperties,
};

pub struct Backend {
    manager: ManagerProxy<'static>,
}

impl backend::Backend for Backend {
    type Context = zbus::Connection;
    type Error = zbus::Error;
    type Proxy<'a> = Proxy<'a>;
    type Stream<'a> = Stream;

    async fn new(system: Self::Context) -> Result<Self, Self::Error> {
        let manager = ManagerProxy::builder(&system)
            .cache_properties(CacheProperties::No)
            .build()
            .await?;

        Ok(Self { manager })
    }

    async fn split<'a>(&'a self) -> Result<(Self::Proxy<'a>, Self::Stream<'a>), Self::Error> {
        let prepare_for_sleep = self.manager.receive_prepare_for_sleep().await?;
        Ok((
            Self::Proxy { backend: self },
            Self::Stream { prepare_for_sleep },
        ))
    }
}

pub struct Proxy<'a> {
    backend: &'a Backend,
}

impl backend::Proxy for Proxy<'_> {
    type Error = zbus::Error;

    async fn event(&mut self, event: &Event) -> Result<(), Self::Error> {
        match event {
            Event::Command(command) => match command {
                CecCommand {
                    opcode: CecOpcode::Standby,
                    ..
                } => self.backend.manager.suspend(false).await?,
                _ => (),
            },
            _ => (),
        }

        Ok(())
    }
}

pub struct Stream {
    prepare_for_sleep: PrepareForSleepStream<'static>,
}

impl backend::Stream for Stream {
    type Error = zbus::Error;

    fn into_stream(self) -> impl futures_util::Stream<Item = Result<Request, Self::Error>> {
        fn map_event(event: PrepareForSleep) -> Result<Request, zbus::Error> {
            Ok(match event.args()?.start {
                true => Request::MetaCommand(MetaCommand::Power(Power::Off { cooperative: true })),

                // After resuming from sleep, libcec gets stuck in an
                // infinite retry loop if we send MetaCommand::Active,
                // so just reset the connection instead
                false => Request::ResetDevice(None),
            })
        }

        self.prepare_for_sleep.map(map_event)
    }
}
