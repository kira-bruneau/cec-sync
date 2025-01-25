mod backend;
mod macro_command;

use {
    async_channel::Sender,
    async_executor::LocalExecutor,
    async_io::block_on,
    async_net::unix::UnixDatagram,
    backend::{all, unix_socket, Backend, Event, Proxy, Request, Stream},
    cec_rs::{
        CecConnection, CecConnectionCfgBuilder, CecConnectionResultError, CecDeviceType,
        CecDeviceTypeVec, CecLogLevel, TryFromCecAudioStatusError,
    },
    clap::{command, Parser, Subcommand},
    futures_util::{try_join, StreamExt},
    macro_command::MacroCommand,
    postcard::experimental::max_size::MaxSize,
    std::{
        fmt::Debug,
        io::{self, ErrorKind},
        process::ExitCode,
        sync::Arc,
    },
};

fn main() -> ExitCode {
    match block_on(Args::parse().command.unwrap_or_default().run()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            log_error(err);
            ExitCode::FAILURE
        }
    }
}

#[derive(Parser)]
struct Args {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    #[command(about = "Run the cec-sync service [default]")]
    Serve,

    #[command(flatten)]
    Macro(MacroCommand),
}

impl Command {
    pub async fn run(self) -> Result<(), Error> {
        match self {
            Command::Serve => serve().await,
            Command::Macro(command) => send_or_run(command).await,
        }
    }
}

impl Default for Command {
    fn default() -> Self {
        Command::Serve
    }
}

async fn serve() -> Result<(), Error> {
    let (tx, rx) = async_channel::unbounded();

    let backend = all::Backend::new(()).await?;
    let (mut proxy, stream) = backend.split().await?;
    let local_ex = LocalExecutor::new();

    let input_task = local_ex.spawn(async move {
        while let Ok(event) = rx.recv().await {
            log_result(proxy.event(&event).await);

            match event {
                Event::LogMessage(log_message) => eprintln!(
                    "{}: cec: {}",
                    match log_message.level {
                        CecLogLevel::Error => return Err(CecError::Log(log_message.message)),
                        CecLogLevel::Warning => "warning",
                        CecLogLevel::Notice => "notice",
                        CecLogLevel::Traffic => "traffic",
                        CecLogLevel::Debug => "debug",
                        CecLogLevel::All => unreachable!(),
                    },
                    log_message.message
                ),
                _ => (),
            };
        }

        Ok(())
    });

    let output_task = local_ex.spawn(async move {
        // NOTE: For now, this assumes that there's only ever one
        // HDMI port that supports CEC.
        //
        // If we want to support multiple, we need to decide how
        // to tell the backends what port an event came from.
        // (would iHDMIPort from the CEC configuration work?)
        //
        // We also need to carefully consider what should be
        // handled globally vs. per-display.
        //
        // eg. Two different connected TVs could have different
        // volumes. It should be possible to adjust each
        // individually.
        let mut cec = cec_build(cec_config_evented(tx.clone()))?;
        let mut stream = stream.into_stream();
        while let Some(action) = stream.next().await {
            if let Some(action) = log_result(action) {
                match action {
                    Request::ResetDevice(port) => {
                        // Explicitly drop old cec connection to
                        // make sure it doesn't keep a lock on the
                        // device when we create a new connection
                        cec = None;
                        let _ = cec;

                        let config = cec_config_evented(tx.clone());
                        let config = match port {
                            Some(port) => config.port(port),
                            None => config,
                        };

                        cec = cec_build(config)?;
                    }
                    Request::RemoveDevice(_) => cec = None,
                    Request::Macro(command) => {
                        if let Some(cec) = &cec {
                            log_result(command.run(cec.clone()).await);
                        }
                    }
                }
            }
        }

        Ok(())
    });

    local_ex
        .run(async { try_join!(input_task, output_task) })
        .await?;

    Ok(())
}

async fn send_or_run(command: MacroCommand) -> Result<(), Error> {
    match send(command).await {
        Ok(()) => return Ok(()),
        Err(err)
            if match err.kind() {
                ErrorKind::NotFound | ErrorKind::ConnectionRefused => true,
                _ => false,
            } =>
        {
            log_notice(
                Error::Send(err),
                "falling back to a direct CEC connection...",
            );
        }
        Err(err) => log_error(Error::Send(err)),
    };

    let cec = Arc::new(cec_config().build().unwrap().open()?);
    command.run(cec).await?;
    Ok(())
}

async fn send(command: MacroCommand) -> Result<(), io::Error> {
    let socket = UnixDatagram::unbound()?;

    // Serialization should never fail
    let mut buf = [0u8; MacroCommand::POSTCARD_MAX_SIZE];
    let command = postcard::to_slice(&command, &mut buf).unwrap();

    let path = unix_socket::Backend::path();
    socket.send_to(&command, &path).await?;
    Ok(())
}

fn cec_config_evented(tx: Sender<Event>) -> CecConnectionCfgBuilder {
    let key_press_tx = tx.clone();
    let command_tx = tx.clone();
    let log_message_tx = tx;
    cec_config()
        .key_press_callback(Box::new(move |key_press| {
            let _ = key_press_tx.try_send(Event::KeyPress(key_press));
        }))
        .command_received_callback(Box::new(move |command| {
            let _ = command_tx.try_send(Event::Command(command));
        }))
        .log_message_callback(Box::new(move |log_message| {
            let _ = log_message_tx.try_send(Event::LogMessage(log_message));
        }))
}

fn cec_config() -> CecConnectionCfgBuilder {
    CecConnectionCfgBuilder::default()
        .device_name("cec-sync".to_owned())
        .device_types(CecDeviceTypeVec::new(CecDeviceType::PlaybackDevice))
}

fn cec_build(
    config: CecConnectionCfgBuilder,
) -> Result<Option<Arc<CecConnection>>, CecConnectionResultError> {
    Ok(match config.build().unwrap().open() {
        Ok(cec) => Some(Arc::new(cec)),
        Err(
            err @ CecConnectionResultError::LibInitFailed
            | err @ CecConnectionResultError::CallbackRegistrationFailed,
        ) => {
            return Err(err);
        }
        Err(err) => {
            log_notice(err, "waiting for adapter...");
            None
        }
    })
}

fn log_result<T, E: Into<Error>>(result: Result<T, E>) -> Option<T> {
    match result {
        Err(err) => {
            log_error(err);
            None
        }
        Ok(ok) => Some(ok),
    }
}

fn log_error<E: Into<Error>>(err: E) {
    eprintln!("error: {}", err.into());
}

fn log_notice<E: Into<Error>>(err: E, recovery_message: &str) {
    eprintln!("notice: {}, {}", err.into(), recovery_message);
}

#[derive(thiserror::Error, Debug)]
enum Error {
    #[error("cec: {0}")]
    Cec(#[from] CecError),
    #[error(transparent)]
    Backend(#[from] all::Error),
    #[error("failed to send to cec-sync service: {0}")]
    Send(io::Error),
}

impl From<CecConnectionResultError> for Error {
    fn from(value: CecConnectionResultError) -> Self {
        Self::Cec(CecError::Connection(value))
    }
}

#[derive(thiserror::Error, Debug)]
pub enum CecError {
    #[error("{}", match .0 {
        CecConnectionResultError::LibInitFailed => "init failed",
        CecConnectionResultError::CallbackRegistrationFailed => "callback registration failed",
        CecConnectionResultError::NoAdapterFound => "no adapter found",
        CecConnectionResultError::AdapterOpenFailed => "failed to open adapter",
        CecConnectionResultError::TransmitFailed => "transmit failed",
    })]
    Connection(CecConnectionResultError),
    #[error("{}", match .0 {
        TryFromCecAudioStatusError::Unknown => "unknown audio status",
        TryFromCecAudioStatusError::Reserved(_) => "reserved audio status",
    })]
    AudioStatus(TryFromCecAudioStatusError),
    #[error("{0}")]
    Log(String),
}

impl From<CecConnectionResultError> for CecError {
    fn from(value: CecConnectionResultError) -> Self {
        Self::Connection(value)
    }
}

impl From<TryFromCecAudioStatusError> for CecError {
    fn from(value: TryFromCecAudioStatusError) -> Self {
        Self::AudioStatus(value)
    }
}
