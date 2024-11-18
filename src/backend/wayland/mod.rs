// TODO: Send & receive events without queue. Can just use futures,
// which have less overhead.

mod gamescope_wayland_client {
    pub mod input_method {
        use wayland_client::{self, protocol::*};

        pub mod __interfaces {
            use wayland_client::protocol::__interfaces::*;
            wayland_scanner::generate_interfaces!("wayland-protocols/gamescope-input-method.xml");
        }
        use self::__interfaces::*;

        wayland_scanner::generate_client_code!("wayland-protocols/gamescope-input-method.xml");
    }
}

use {
    crate::backend::{self, Event},
    async_io::Async,
    cec_rs::{CecKeypress, CecUserControlCode},
    futures_util::ready,
    gamescope_wayland_client::input_method::{
        __interfaces::GAMESCOPE_INPUT_METHOD_MANAGER_INTERFACE,
        gamescope_input_method::{self, Action, GamescopeInputMethod},
        gamescope_input_method_manager::{self, GamescopeInputMethodManager},
    },
    std::{
        future::poll_fn,
        io,
        task::{Context, Poll},
    },
    wayland_client::{
        backend::WaylandError,
        protocol::{
            __interfaces::WL_SEAT_INTERFACE,
            wl_registry::{self, WlRegistry},
            wl_seat::{self, WlSeat},
        },
        ConnectError, Connection, Dispatch, DispatchError, EventQueue, QueueHandle,
    },
};

pub struct Backend {
    connection: Connection,
}

impl backend::Backend for Backend {
    type Context = ();
    type Error = Error;
    type Proxy<'a> = Proxy;
    type Stream<'a> = ();

    async fn new(_: Self::Context) -> Result<Self, Error> {
        Ok(Self {
            connection: Connection::connect_to_env()?,
        })
    }

    async fn split<'a>(&'a self) -> Result<(Self::Proxy<'a>, Self::Stream<'a>), Error> {
        let display = self.connection.display();
        let mut event_queue = AsyncEventQueue::new(self.connection.new_event_queue())?;
        let qh = event_queue.handle();
        let _registry = display.get_registry(&qh, ());

        let mut state = State::new();
        event_queue.dispatch(&mut state).await?;

        match (state.seat.as_ref(), state.input_method_manager.as_ref()) {
            (Some(seat), Some(input_method_manager)) => {
                state.input_method = Some(input_method_manager.create_input_method(seat, &qh, ()));
                event_queue.dispatch(&mut state).await?;
            }
            _ => (),
        }

        Ok((Self::Proxy { state, event_queue }, Self::Stream::default()))
    }
}

pub struct Proxy {
    state: State,
    event_queue: AsyncEventQueue<State>,
}

impl backend::Proxy for Proxy {
    type Error = Error;

    async fn event(&mut self, event: &Event) -> Result<(), Self::Error> {
        let state = &self.state;

        if let Some(input_method) = &state.input_method {
            match event {
                Event::KeyPress(key_press) => match (key_press, key_press.duration.is_zero()) {
                    (CecKeypress { keycode, .. }, true) => match keycode {
                        CecUserControlCode::Up => {
                            input_method.set_action(Action::MoveUp);
                            input_method.commit(state.serial);
                            self.event_queue.flush().await?;
                        }
                        CecUserControlCode::Down => {
                            input_method.set_action(Action::MoveDown);
                            input_method.commit(state.serial);
                            self.event_queue.flush().await?;
                        }
                        CecUserControlCode::Left => {
                            input_method.set_action(Action::MoveLeft);
                            input_method.commit(state.serial);
                            self.event_queue.flush().await?;
                        }
                        CecUserControlCode::Right => {
                            input_method.set_action(Action::MoveRight);
                            input_method.commit(state.serial);
                            self.event_queue.flush().await?;
                        }
                        CecUserControlCode::Select => {
                            input_method.set_action(Action::Submit);
                            input_method.commit(state.serial);
                            self.event_queue.flush().await?;
                        }
                        CecUserControlCode::Exit => {
                            input_method.set_string(String::from("\x1B"));
                            input_method.commit(state.serial);
                            self.event_queue.flush().await?;
                        }
                        _ => (),
                    },
                    _ => (),
                },
                _ => (),
            }
        }

        Ok(())
    }
}

struct State {
    pub seat: Option<WlSeat>,
    pub input_method_manager: Option<GamescopeInputMethodManager>,
    pub input_method: Option<GamescopeInputMethod>,
    pub serial: u32,
}

impl State {
    fn new() -> Self {
        Self {
            seat: None,
            input_method_manager: None,
            input_method: None,
            serial: 0,
        }
    }
}

impl Dispatch<WlRegistry, ()> for State {
    fn event(
        state: &mut Self,
        registry: &WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<State>,
    ) {
        if let wl_registry::Event::Global {
            name, interface, ..
        } = event
        {
            match &interface[..] {
                "wl_seat" => {
                    state.seat = Some(registry.bind::<WlSeat, _, _>(
                        name,
                        WL_SEAT_INTERFACE.version,
                        qh,
                        (),
                    ));
                }
                "gamescope_input_method_manager" => {
                    state.input_method_manager =
                        Some(registry.bind::<GamescopeInputMethodManager, _, _>(
                            name,
                            GAMESCOPE_INPUT_METHOD_MANAGER_INTERFACE.version,
                            qh,
                            (),
                        ));
                }
                _ => (),
            }
        }
    }
}

impl Dispatch<WlSeat, ()> for State {
    fn event(
        _state: &mut Self,
        _seat: &WlSeat,
        _event: wl_seat::Event,
        _: &(),
        _: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<GamescopeInputMethodManager, ()> for State {
    fn event(
        _state: &mut Self,
        _control: &GamescopeInputMethodManager,
        _event: gamescope_input_method_manager::Event,
        _: &(),
        _: &Connection,
        _qh: &QueueHandle<State>,
    ) {
    }
}

impl Dispatch<GamescopeInputMethod, ()> for State {
    fn event(
        state: &mut Self,
        _control: &GamescopeInputMethod,
        event: gamescope_input_method::Event,
        _: &(),
        _: &Connection,
        _qh: &QueueHandle<State>,
    ) {
        match event {
            gamescope_input_method::Event::Done { serial } => state.serial = serial,
            _ => (),
        }
    }
}

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("failed to connect to server: {0}")]
    Connect(#[from] ConnectError),
    #[error("failed to dispatch event: {0}")]
    Dispatch(#[from] DispatchError),
    #[error("wayland: {0}")]
    WaylandError(#[from] WaylandError),
    #[error(transparent)]
    Io(#[from] io::Error),
}

struct AsyncEventQueue<State> {
    inner: Async<EventQueue<State>>,
}

impl<State> AsyncEventQueue<State> {
    pub fn new(inner: EventQueue<State>) -> io::Result<Self> {
        Ok(Self {
            inner: Async::new_nonblocking(inner)?,
        })
    }

    pub fn handle(&self) -> QueueHandle<State> {
        self.inner.get_ref().handle()
    }

    pub async fn dispatch(&mut self, state: &mut State) -> Result<usize, DispatchError> {
        poll_fn(|cx| self.poll_dispatch(cx, state)).await
    }

    fn poll_dispatch(
        &mut self,
        cx: &mut Context,
        state: &mut State,
    ) -> Poll<Result<usize, DispatchError>> {
        loop {
            // dispatch_pending won't move & drop the inner resource, so the get_mut call is safe
            let dispatched = unsafe { self.inner.get_mut().dispatch_pending(state)? };
            if dispatched > 0 {
                return Poll::Ready(Ok(dispatched));
            }

            ready!(self.poll_flush(cx))?;

            if let Some(guard) = self.inner.get_ref().prepare_read() {
                ready!(self.inner.poll_readable(cx)).map_err(|err| WaylandError::Io(err))?;
                guard.read()?;
            }
        }
    }

    async fn flush(&self) -> Result<(), WaylandError> {
        poll_fn(|cx| self.poll_flush(cx)).await
    }

    fn poll_flush(&self, cx: &mut Context) -> Poll<Result<(), WaylandError>> {
        loop {
            match self.inner.get_ref().flush() {
                Ok(()) => return Poll::Ready(Ok(())),
                Err(WaylandError::Io(err)) if err.kind() == io::ErrorKind::WouldBlock => {
                    ready!(self.inner.poll_writable(cx))?
                }
                Err(err) => return Poll::Ready(Err(err)),
            };
        }
    }
}
