mod backend;
mod meta_command;

use {
    async_executor::LocalExecutor,
    async_io::block_on,
    async_net::unix::UnixDatagram,
    backend::{unix_socket, Backend, Event, Request, Stream},
    cec_rs::{
        CecAudioStatusError, CecConnectionCfgBuilder, CecConnectionError, CecDeviceType,
        CecDeviceTypeVec, CecLogLevel,
    },
    clap::{command, Parser, Subcommand},
    futures_util::{StreamExt, TryFutureExt, TryStreamExt},
    meta_command::MetaCommand,
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
    MetaCommand(MetaCommand),
}

impl Command {
    pub async fn run(self) -> Result<(), Error> {
        match self {
            Command::Serve => serve().await,
            Command::MetaCommand(command) => send_or_run(command).await,
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

    let (_, unix_stream) = unix_socket::Backend::new()
        .and_then(|backend| backend.split())
        .await
        .map_err(BackendError::UnixSocket)?;

    let local_ex = LocalExecutor::new();

    local_ex
        .spawn(async move {
            while let Ok(event) = rx.recv().await {
                match &event {
                    Event::LogMessage(log_message) => eprintln!(
                        "{}: cec: {}",
                        match log_message.level {
                            CecLogLevel::Error => "error",
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
        })
        .detach();

    local_ex
        .run(async move {
            let key_press_tx = tx.clone();
            let command_tx = tx.clone();
            let log_message_tx = tx.clone();

            let cec = match cec_config()
                .key_press_callback(Box::new(move |key_press| {
                    let _ = key_press_tx.try_send(Event::KeyPress(key_press));
                }))
                .command_received_callback(Box::new(move |command| {
                    let _ = command_tx.try_send(Event::Command(command));
                }))
                .log_message_callback(Box::new(move |log_message| {
                    let _ = log_message_tx.try_send(Event::LogMessage(log_message));
                }))
                .build()
                .unwrap()
                .open()
            {
                Ok(cec) => Some(Arc::new(cec)),
                Err(
                    err @ CecConnectionError::LibInitFailed
                    | err @ CecConnectionError::CallbackRegistrationFailed,
                ) => {
                    return Err(Error::from(err));
                }
                Err(err) => {
                    log_notice(err, "waiting for adapter...");
                    None
                }
            };

            let mut unix_stream = unix_stream.into_stream().map_err(BackendError::UnixSocket);
            while let Some(action) = unix_stream.next().await {
                if let Some(action) = log_result(action) {
                    match action {
                        Request::MetaCommand(command) => {
                            if let Some(cec) = &cec {
                                log_result(command.run(cec.clone()).await);
                            }
                        }
                    }
                }
            }

            Ok(())
        })
        .await
}

async fn send_or_run(command: MetaCommand) -> Result<(), Error> {
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

async fn send(command: MetaCommand) -> Result<(), io::Error> {
    let socket = UnixDatagram::unbound()?;

    // Serialization should never fail
    let mut buf = [0u8; MetaCommand::POSTCARD_MAX_SIZE];
    let command = postcard::to_slice(&command, &mut buf).unwrap();

    let path = unix_socket::Backend::path();
    socket.send_to(&command, &path).await?;
    Ok(())
}

fn cec_config() -> CecConnectionCfgBuilder {
    CecConnectionCfgBuilder::default()
        .device_name("cec-sync".to_owned())
        .device_types(CecDeviceTypeVec::new(CecDeviceType::PlaybackDevice))
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
    Backend(#[from] BackendError),
    #[error("failed to send to cec-sync service: {0}")]
    Send(io::Error),
}

impl From<CecConnectionError> for Error {
    fn from(value: CecConnectionError) -> Self {
        Self::Cec(CecError::Connection(value))
    }
}

#[derive(thiserror::Error, Debug)]
pub enum CecError {
    #[error("{}", match .0 {
        CecConnectionError::LibInitFailed => "init failed",
        CecConnectionError::CallbackRegistrationFailed => "callback registration failed",
        CecConnectionError::NoAdapterFound => "no adapter found",
        CecConnectionError::AdapterOpenFailed => "failed to open adapter",
        CecConnectionError::TransmitFailed => "transmit failed",
    })]
    Connection(CecConnectionError),
    #[error("{}", match .0 {
        CecAudioStatusError::Unknown => "unknown audio status",
        CecAudioStatusError::Reserved(_) => "reserved audio status",
    })]
    AudioStatus(CecAudioStatusError),
}

impl From<CecConnectionError> for CecError {
    fn from(value: CecConnectionError) -> Self {
        Self::Connection(value)
    }
}

impl From<CecAudioStatusError> for CecError {
    fn from(value: CecAudioStatusError) -> Self {
        Self::AudioStatus(value)
    }
}

#[derive(thiserror::Error, Debug)]
enum BackendError {
    #[error("unix socket: {0}")]
    UnixSocket(unix_socket::Error),
}
