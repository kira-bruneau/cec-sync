mod player;

use {
    crate::{
        backend::{self, Request},
        meta_command::{DeckInfo, MetaCommand},
        Event,
    },
    cec_rs::{CecKeypress, CecUserControlCode},
    futures_util::{
        future::try_join_all, lock::Mutex as AsyncMutex, ready, FutureExt, StreamExt, TryFutureExt,
    },
    player::PlayerProxy,
    std::{cmp::min, collections::HashMap, future::Future, pin::Pin, task::Poll},
    zbus::{
        fdo::{DBusProxy, NameOwnerChanged},
        names::BusName,
        proxy::{CacheProperties, PropertyStream},
        MatchRule, MessageStream,
    },
};

pub struct Backend {
    players: AsyncMutex<Players>,
}

pub async fn new(session: zbus::Connection) -> Result<Backend, zbus::Error> {
    let players = Players::new(session).await?;
    Ok(Backend {
        players: AsyncMutex::new(players),
    })
}

pub async fn split<'a>(backend: &'a Backend) -> Result<(Proxy<'a>, Stream<'a>), zbus::Error> {
    Ok((Proxy { backend }, Stream { backend }))
}

pub struct Proxy<'a> {
    backend: &'a Backend,
}

impl backend::Proxy for Proxy<'_> {
    type Error = zbus::Error;

    async fn event(&mut self, event: &Event) -> Result<(), Self::Error> {
        match event {
            Event::KeyPress(key_press) => match (key_press, key_press.duration.is_zero()) {
                (CecKeypress { keycode, .. }, true) => match keycode {
                    CecUserControlCode::Play => {
                        try_join_all(
                            self.backend
                                .players
                                .try_lock()
                                .unwrap()
                                .iter()
                                .map(|player| player.proxy.play()),
                        )
                        .await?;
                    }
                    CecUserControlCode::Pause => {
                        try_join_all(
                            self.backend
                                .players
                                .try_lock()
                                .unwrap()
                                .iter()
                                .map(|player| player.proxy.play_pause()),
                        )
                        .await?;
                    }
                    CecUserControlCode::Stop => {
                        try_join_all(
                            self.backend
                                .players
                                .try_lock()
                                .unwrap()
                                .iter()
                                .map(|player| player.proxy.stop()),
                        )
                        .await?;
                    }
                    CecUserControlCode::FastForward => {
                        try_join_all(self.backend.players.try_lock().unwrap().iter().map(
                            |player| {
                                player
                                    .proxy
                                    .pause()
                                    .and_then(|_| player.proxy.seek(10000000))
                            },
                        ))
                        .await?;
                    }
                    CecUserControlCode::Rewind => {
                        try_join_all(self.backend.players.try_lock().unwrap().iter().map(
                            |player| {
                                player
                                    .proxy
                                    .pause()
                                    .and_then(|_| player.proxy.seek(-10000000))
                            },
                        ))
                        .await?;
                    }
                    CecUserControlCode::Forward => {
                        try_join_all(
                            self.backend
                                .players
                                .try_lock()
                                .unwrap()
                                .iter()
                                .map(|player| player.proxy.next()),
                        )
                        .await?;
                    }
                    CecUserControlCode::Backward => {
                        try_join_all(
                            self.backend
                                .players
                                .try_lock()
                                .unwrap()
                                .iter()
                                .map(|player| player.proxy.previous()),
                        )
                        .await?;
                    }
                    _ => (),
                },
                _ => (),
            },
            _ => (),
        }

        Ok(())
    }
}

pub struct Stream<'a> {
    backend: &'a Backend,
}

impl backend::Stream for Stream<'_> {
    type Error = zbus::Error;

    fn into_stream(self) -> impl futures_util::Stream<Item = Result<Request, Self::Error>> {
        self
    }
}

impl futures_util::Stream for Stream<'_> {
    type Item = Result<Request, zbus::Error>;

    fn poll_next(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Option<Self::Item>> {
        let mut players = ready!(self.backend.players.lock().poll_unpin(cx));
        players.poll_next_unpin_inner(cx)
    }
}

struct Players {
    session: zbus::Connection,
    media_player_owner_changed: MessageStream,
    inner: HashMap<String, PlayerFuture>,
    deck_info: DeckInfo,
}

impl Players {
    async fn new(session: zbus::Connection) -> Result<Self, zbus::Error> {
        let media_player_owner_changed = MessageStream::for_match_rule(
            MatchRule::builder()
                .msg_type(zbus::message::Type::Signal)
                .sender("org.freedesktop.DBus")?
                .path("/org/freedesktop/DBus")?
                .interface("org.freedesktop.DBus")?
                .member("NameOwnerChanged")?
                .arg0ns("org.mpris.MediaPlayer2")?
                .build(),
            &session,
            Some(1),
        )
        .await?;

        let dbus = DBusProxy::builder(&session)
            .cache_properties(CacheProperties::No)
            .build()
            .await?;

        let inner = dbus
            .list_names()
            .await?
            .into_iter()
            .flat_map(|name| {
                if name.as_str().starts_with("org.mpris.MediaPlayer2.") {
                    Some((
                        name.as_str().to_owned(),
                        PlayerFuture::Pending(Box::pin(Player::new(&session, name.into())) as _),
                    ))
                } else {
                    None
                }
            })
            .collect();

        Ok(Self {
            session,
            media_player_owner_changed,
            inner,
            deck_info: DeckInfo::default(),
        })
    }

    fn poll_next_unpin_inner(
        &mut self,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Option<Result<Request, zbus::Error>>> {
        let mut has_updates = false;
        loop {
            match self.media_player_owner_changed.poll_next_unpin(cx)? {
                Poll::Pending => break,
                Poll::Ready(None) => return Poll::Ready(None),
                Poll::Ready(Some(message)) => {
                    let signal =
                        NameOwnerChanged::from_message(message).ok_or(zbus::Error::MissingField)?;

                    let args = signal.args()?;
                    if args.new_owner().is_some() {
                        let future = PlayerFuture::Pending(Box::pin(Player::new(
                            &self.session,
                            args.name().to_owned(),
                        )));

                        self.inner.insert(args.name().as_str().to_owned(), future);
                    } else {
                        self.inner.remove(args.name().as_str());
                        has_updates = true;
                    }
                }
            }
        }

        for future in self.inner.values_mut() {
            loop {
                if let Poll::Ready(player) = future.poll_as_mut_unpin(cx)? {
                    match player.playback_status_changed.poll_next_unpin(cx) {
                        Poll::Pending => break,

                        // Property change streams hang when player is removed
                        // Have to watch NameOwnerChanged events & manually remove
                        Poll::Ready(None) => unreachable!(),

                        // We could break the outer loop early at this point, but
                        // we still want poll all the other futures, so set a flag
                        Poll::Ready(Some(_)) => has_updates = true,
                    }
                }
            }
        }

        if has_updates {
            let deck_info = self
                .iter()
                .map(|player| match player.proxy.cached_playback_status() {
                    Ok(Some(status)) => match status.as_str() {
                        "Playing" => DeckInfo::Play,
                        "Paused" => DeckInfo::Still,
                        "Stopped" => DeckInfo::Stop,
                        _ => DeckInfo::Stop,
                    },
                    _ => DeckInfo::Stop,
                })
                .fold(DeckInfo::Stop, |acc, status| min(acc, status));

            if self.deck_info != deck_info {
                self.deck_info = deck_info;
                return Poll::Ready(Some(Ok(Request::MetaCommand(MetaCommand::DeckInfo(
                    deck_info,
                )))));
            }
        }

        Poll::Pending
    }

    fn iter(&self) -> impl Iterator<Item = &Player> {
        self.inner.values().flat_map(PlayerFuture::as_ref)
    }
}

enum PlayerFuture {
    Pending(Pin<Box<dyn Future<Output = Result<Player, zbus::Error>>>>),
    Ready(Player),
}

impl PlayerFuture {
    fn poll_as_mut_unpin(
        &mut self,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Result<&mut Player, zbus::Error>> {
        match self {
            PlayerFuture::Pending(future) => match future.poll_unpin(cx) {
                Poll::Pending => Poll::Pending,
                Poll::Ready(player) => {
                    *self = PlayerFuture::Ready(player?);
                    match self {
                        PlayerFuture::Ready(player) => Poll::Ready(Ok(player)),
                        PlayerFuture::Pending(_) => unreachable!(),
                    }
                }
            },
            PlayerFuture::Ready(player) => Poll::Ready(Ok(player)),
        }
    }

    fn as_ref(&self) -> Option<&Player> {
        match self {
            PlayerFuture::Pending(_) => None,
            PlayerFuture::Ready(player) => Some(player),
        }
    }
}

struct Player {
    proxy: PlayerProxy<'static>,
    playback_status_changed: PropertyStream<'static, String>,
}

impl Player {
    fn new(
        session: &zbus::Connection,
        destination: BusName<'static>,
    ) -> impl Future<Output = Result<Self, zbus::Error>> {
        PlayerProxy::builder(session)
            .destination(destination)
            .unwrap()
            .build()
            .and_then(|proxy| Player::from_proxy(proxy).map(Ok))
    }

    async fn from_proxy(proxy: PlayerProxy<'static>) -> Self {
        let playback_status_changed = proxy.receive_playback_status_changed().await;
        Self {
            proxy,
            playback_status_changed,
        }
    }
}
