use {
    crate::{
        backend::{self, Event, Request},
        meta_command::{MetaCommand, Power},
    },
    cec_rs::{CecCommand, CecOpcode},
    futures_util::StreamExt,
    logind_zbus::manager::{
        ManagerProxy as LogindManagerProxy, PrepareForSleep, PrepareForSleepStream,
    },
    zbus::{proxy::CacheProperties, Connection},
};

pub struct Backend {
    connection: Connection,
}

impl backend::Backend for Backend {
    type Error = zbus::Error;

    type Proxy = Proxy;

    type Stream = Stream;

    async fn new() -> Result<Self, Self::Error> {
        Ok(Self {
            connection: Connection::system().await?,
        })
    }

    async fn split(self) -> Result<(Self::Proxy, Self::Stream), Self::Error> {
        let logind_manager = LogindManagerProxy::builder(&self.connection)
            .cache_properties(CacheProperties::No)
            .build()
            .await?;

        let prepare_for_sleep = logind_manager.receive_prepare_for_sleep().await?;
        Ok((
            Self::Proxy { logind_manager },
            Self::Stream { prepare_for_sleep },
        ))
    }
}

pub struct Proxy {
    logind_manager: LogindManagerProxy<'static>,
}

impl backend::Proxy for Proxy {
    type Error = zbus::Error;

    async fn event(&mut self, event: &Event) -> Result<(), Self::Error> {
        match event {
            Event::Command(command) => match command {
                CecCommand {
                    opcode: CecOpcode::Standby,
                    ..
                } => self.logind_manager.suspend(false).await?,
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
