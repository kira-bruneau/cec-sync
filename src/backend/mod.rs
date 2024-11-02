pub mod all;
pub mod dbus;
pub mod udev;
pub mod unix_socket;
pub mod wayland;

use {
    crate::meta_command::MetaCommand,
    cec_rs::{CecCommand, CecKeypress, CecLogMessage},
    futures_util::stream,
    std::ffi::CString,
};

pub trait Backend: Sized {
    type Error;

    type Proxy: Proxy;

    type Stream: Stream;

    async fn new() -> Result<Self, Self::Error>;

    async fn split(self) -> Result<(Self::Proxy, Self::Stream), Self::Error>;
}

pub trait Proxy {
    type Error;

    async fn event(&mut self, event: &Event) -> Result<(), Self::Error>;
}

#[derive(Clone)]
pub enum Event {
    KeyPress(CecKeypress),
    Command(CecCommand),
    LogMessage(CecLogMessage),
}

impl Proxy for () {
    type Error = ();

    async fn event(&mut self, _event: &Event) -> Result<(), Self::Error> {
        Ok(())
    }
}

pub trait Stream {
    type Error;

    // Ideally this would be a async generator function, but that's
    // still experimental in Rust, so for now implementers will have
    // to explicitly compose streams.
    fn into_stream(self) -> impl futures_util::Stream<Item = Result<Request, Self::Error>>;
}

impl Stream for () {
    type Error = ();

    fn into_stream(self) -> impl futures_util::Stream<Item = Result<Request, Self::Error>> {
        stream::empty()
    }
}

#[derive(Debug, Clone)]
pub enum Request {
    ResetDevice(Option<CString>),
    RemoveDevice(#[expect(dead_code)] CString),
    MetaCommand(MetaCommand),
}
