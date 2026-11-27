//! Provides a [calloop] event source from [XDG Desktop Portal] events
//!
//! This module uses the [ashpd] crate

use ashpd::desktop::settings::{ColorScheme, Settings};
use calloop::channel::{Channel, Sender};
use calloop::{EventSource, Poll, PostAction, Readiness, Token, TokenFactory};
use futures::future::BoxFuture;
use smol::stream::StreamExt;

use gpui::{BackgroundExecutor, Task, WindowAppearance};

pub enum Event {
    WindowAppearance(WindowAppearance),
    #[cfg_attr(feature = "x11", allow(dead_code))]
    CursorTheme(String),
    #[cfg_attr(feature = "x11", allow(dead_code))]
    CursorSize(u32),
    ButtonLayout(String),
}

/// Forwards desktop settings from the portal to the event loop.
///
/// Owns the task that listens to the portal, so removing the source from the loop cancels it and
/// releases its D-Bus subscriptions.
pub struct XDPEventSource {
    channel: Channel<Event>,
    _listener: Task<()>,
    /// Nexus: the portal's first color scheme, for `initial_appearance`.
    first_appearance: std::sync::mpsc::Receiver<WindowAppearance>,
}

/// Nexus: how long the platform waits for the portal's first color scheme before it opens windows. Without the wait,
/// the first frames draw with the default (Light) while the portal answers, and a dark desktop sees a light flash.
pub const INITIAL_APPEARANCE_WAIT: std::time::Duration = std::time::Duration::from_millis(250);

impl XDPEventSource {
    pub fn new(executor: &BackgroundExecutor) -> Self {
        let (sender, channel) = calloop::channel::channel();
        let (first_sender, first_appearance) = std::sync::mpsc::sync_channel(1);
        let listener = executor.spawn(async move {
            if let Err(error) = listen(sender, first_sender).await {
                log::debug!("stopped listening to desktop portal settings: {error:#}");
            }
        });
        Self {
            channel,
            _listener: listener,
            first_appearance,
        }
    }

    /// Nexus: the portal's first color scheme, waiting at most `timeout` for it. None when the portal is missing,
    /// has no answer, or is slower than that; its answer then still comes later as an event.
    pub fn initial_appearance(&self, timeout: std::time::Duration) -> Option<WindowAppearance> {
        self.first_appearance.recv_timeout(timeout).ok()
    }
}

async fn listen(
    sender: Sender<Event>,
    first_sender: std::sync::mpsc::SyncSender<WindowAppearance>,
) -> anyhow::Result<()> {
    let settings = Settings::new().await?;

    if let Ok(initial_appearance) = settings.color_scheme().await {
        let appearance = window_appearance_from_color_scheme(initial_appearance);
        // Nexus: the platform may be waiting for it (`initial_appearance`). Nobody waiting is not an error.
        let _ = first_sender.try_send(appearance);
        sender.send(Event::WindowAppearance(appearance))?;
    }
    drop(first_sender);
    if let Ok(initial_theme) = settings
        .read::<String>("org.gnome.desktop.interface", "cursor-theme")
        .await
    {
        sender.send(Event::CursorTheme(initial_theme))?;
    }

    // If u32 is used here, it throws invalid type error
    if let Ok(initial_size) = settings
        .read::<i32>("org.gnome.desktop.interface", "cursor-size")
        .await
    {
        sender.send(Event::CursorSize(initial_size as u32))?;
    }

    if let Ok(initial_layout) = settings
        .read::<String>("org.gnome.desktop.wm.preferences", "button-layout")
        .await
    {
        sender.send(Event::ButtonLayout(initial_layout))?;
    }

    // Each listener runs until its stream or the channel closes. They're joined rather than
    // spawned, so that cancelling this future cancels them all.
    let mut listeners: Vec<BoxFuture<'_, anyhow::Result<()>>> = Vec::new();

    if let Ok(mut cursor_theme_changed) = settings
        .receive_setting_changed_with_args("org.gnome.desktop.interface", "cursor-theme")
        .await
    {
        let sender = sender.clone();
        listeners.push(Box::pin(async move {
            while let Some(theme) = cursor_theme_changed.next().await {
                sender.send(Event::CursorTheme(theme?))?;
            }
            anyhow::Ok(())
        }));
    }

    if let Ok(mut cursor_size_changed) = settings
        .receive_setting_changed_with_args::<i32>("org.gnome.desktop.interface", "cursor-size")
        .await
    {
        let sender = sender.clone();
        listeners.push(Box::pin(async move {
            while let Some(size) = cursor_size_changed.next().await {
                sender.send(Event::CursorSize(size? as u32))?;
            }
            anyhow::Ok(())
        }));
    }

    if let Ok(mut button_layout_changed) = settings
        .receive_setting_changed_with_args("org.gnome.desktop.wm.preferences", "button-layout")
        .await
    {
        let sender = sender.clone();
        listeners.push(Box::pin(async move {
            while let Some(layout) = button_layout_changed.next().await {
                sender.send(Event::ButtonLayout(layout?))?;
            }
            anyhow::Ok(())
        }));
    }

    let mut appearance_changed = settings.receive_color_scheme_changed().await?;
    listeners.push(Box::pin(async move {
        while let Some(scheme) = appearance_changed.next().await {
            sender.send(Event::WindowAppearance(
                window_appearance_from_color_scheme(scheme),
            ))?;
        }
        anyhow::Ok(())
    }));

    for result in futures::future::join_all(listeners).await {
        result?;
    }
    Ok(())
}

impl EventSource for XDPEventSource {
    type Event = Event;
    type Metadata = ();
    type Ret = ();
    type Error = anyhow::Error;

    fn process_events<F>(
        &mut self,
        readiness: Readiness,
        token: Token,
        mut callback: F,
    ) -> Result<PostAction, Self::Error>
    where
        F: FnMut(Self::Event, &mut Self::Metadata) -> Self::Ret,
    {
        self.channel.process_events(readiness, token, |evt, _| {
            if let calloop::channel::Event::Msg(msg) = evt {
                (callback)(msg, &mut ())
            }
        })?;

        Ok(PostAction::Continue)
    }

    fn register(
        &mut self,
        poll: &mut Poll,
        token_factory: &mut TokenFactory,
    ) -> calloop::Result<()> {
        self.channel.register(poll, token_factory)?;

        Ok(())
    }

    fn reregister(
        &mut self,
        poll: &mut Poll,
        token_factory: &mut TokenFactory,
    ) -> calloop::Result<()> {
        self.channel.reregister(poll, token_factory)?;

        Ok(())
    }

    fn unregister(&mut self, poll: &mut Poll) -> calloop::Result<()> {
        self.channel.unregister(poll)?;

        Ok(())
    }
}

fn window_appearance_from_color_scheme(cs: ColorScheme) -> WindowAppearance {
    match cs {
        ColorScheme::PreferDark => WindowAppearance::Dark,
        ColorScheme::PreferLight => WindowAppearance::Light,
        ColorScheme::NoPreference => WindowAppearance::Light,
    }
}
