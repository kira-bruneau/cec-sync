use {
    crate::{
        backend::{self, Event, Request},
        meta_command::{MetaCommand, Power},
    },
    async_stream::try_stream,
    cec_rs::{CecCommand, CecOpcode},
    futures_util::StreamExt,
    logind_zbus::manager::{InhibitType, ManagerProxy, PrepareForSleepStream},
    std::cell::RefCell,
    zbus::{proxy::CacheProperties, zvariant::OwnedFd},
};

pub struct Backend {
    manager: ManagerProxy<'static>,
    sleep_lock: RefCell<Option<OwnedFd>>,
}

impl Backend {
    async fn sleep_lock(manager: &ManagerProxy<'static>) -> Result<OwnedFd, zbus::Error> {
        manager
            .inhibit(
                InhibitType::Sleep,
                "cec-sync",
                "Signal sleep event to CEC devices before sleeping",
                "delay",
            )
            .await
    }
}

impl backend::Backend for Backend {
    type Context = zbus::Connection;
    type Error = zbus::Error;
    type Proxy<'a> = Proxy<'a>;
    type Stream<'a> = Stream<'a>;

    async fn new(system: Self::Context) -> Result<Self, Self::Error> {
        let manager = ManagerProxy::builder(&system)
            .cache_properties(CacheProperties::No)
            .build()
            .await?;

        let sleep_lock = RefCell::new(Some(Self::sleep_lock(&manager).await?));
        Ok(Self {
            manager,
            sleep_lock,
        })
    }

    async fn split<'a>(&'a self) -> Result<(Self::Proxy<'a>, Self::Stream<'a>), Self::Error> {
        let prepare_for_sleep = self.manager.receive_prepare_for_sleep().await?;
        Ok((
            Self::Proxy { backend: self },
            Self::Stream {
                backend: self,
                prepare_for_sleep,
            },
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
                } => {
                    self.backend.sleep_lock.replace(None);
                    self.backend.manager.suspend(false).await?;
                }
                _ => (),
            },
            _ => (),
        }

        Ok(())
    }
}

pub struct Stream<'a> {
    backend: &'a Backend,
    prepare_for_sleep: PrepareForSleepStream<'static>,
}

impl backend::Stream for Stream<'_> {
    type Error = zbus::Error;

    fn into_stream(mut self) -> impl futures_util::Stream<Item = Result<Request, Self::Error>> {
        Box::pin(try_stream! {
            while let Some(event) = self.prepare_for_sleep.next().await {
                match event.args()?.start {
                    true => {
                        if self.backend.sleep_lock.borrow().is_some() {
                            yield Request::MetaCommand(MetaCommand::Power(Power::Off {
                                cooperative: true,
                            }));
                        }
                    }
                    false => {
                        // After resuming from sleep, libcec gets stuck in an
                        // infinite retry loop if we send MetaCommand::Active,
                        // so just reset the connection instead
                        yield Request::ResetDevice(None);

                        self.backend
                            .sleep_lock
                            .replace(Some(Backend::sleep_lock(&self.backend.manager).await?));
                    }
                }
            }
        })
    }
}
