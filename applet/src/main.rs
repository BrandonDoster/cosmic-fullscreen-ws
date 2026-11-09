//! COSMIC panel applet for fullscreen-ws.
//!
//! Runs the same engine as the `fullscreen-ws` service, on the privileged
//! Wayland connection the panel hands to applets that set
//! `X-HostWaylandDisplay=true` (env `X_PRIVILEGED_WAYLAND_SOCKET`). Only that
//! connection is allowed to use COSMIC's window-management protocols.

use std::os::fd::{FromRawFd, RawFd};
use std::os::unix::net::UnixStream;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cosmic::app::{Core, Task};
use cosmic::applet::padded_control;
use cosmic::cosmic_config::{Config, ConfigGet, ConfigSet};
use cosmic::iced::platform_specific::shell::wayland::commands::popup::destroy_popup;
use cosmic::iced::widget::column;
use cosmic::iced::window::Id;
use cosmic::iced::{Length, Subscription, time};
use cosmic::widget::{divider, text, toggler};
use cosmic::Element;
use fullscreen_ws::{Engine, Settings, acquire_instance_lock_with};
use wayland_client::Connection;

const APP_ID: &str = "io.github.BrandonDoster.FullscreenWs";
const ICON: &str = "view-fullscreen-symbolic";

fn main() -> cosmic::iced::Result {
    cosmic::applet::run::<Applet>(())
}

struct Applet {
    core: Core,
    popup: Option<Id>,
    config: Option<Config>,
    settings: Arc<Settings>,
    status: Arc<Mutex<String>>,
    status_text: String,
}

#[derive(Debug, Clone)]
enum Message {
    TogglePopup,
    PopupClosed(Id),
    SetEnabled(bool),
    SetIncludeMaximized(bool),
    Tick,
}

/// Connect on the panel's privileged socket if we have one.
fn connect() -> Result<(Connection, &'static str), fullscreen_ws::Error> {
    if let Some(fd) = std::env::var("X_PRIVILEGED_WAYLAND_SOCKET")
        .ok()
        .and_then(|v| v.parse::<RawFd>().ok())
    {
        let stream = unsafe { UnixStream::from_raw_fd(fd) };
        return Ok((Connection::from_socket(stream)?, "privileged socket"));
    }
    Ok((Connection::connect_to_env()?, "normal socket (no privileged socket from panel)"))
}

fn spawn_engine(settings: Arc<Settings>, status: Arc<Mutex<String>>) {
    let set = move |s: String| {
        eprintln!("fullscreen-ws applet: {s}");
        *status.lock().unwrap() = s;
    };
    std::thread::spawn(move || {
        let (conn, how) = match connect() {
            Ok(c) => c,
            Err(e) => return set(format!("Error: can't connect: {e}")),
        };
        let _lock = match acquire_instance_lock_with(|| {
            set("Waiting: another copy (the fullscreen-ws service?) is running".into())
        }) {
            Ok(l) => l,
            Err(e) => return set(format!("Error: {e}")),
        };
        let engine = match Engine::new(conn, settings) {
            Ok(e) => e,
            Err(e) => return set(format!("Error via {how}: {e}")),
        };
        set(format!("Active via {how}, {} windows", engine.window_count()));
        if let Err(e) = engine.run() {
            set(format!("Stopped: {e}"));
        }
    });
}

impl cosmic::Application for Applet {
    type Executor = cosmic::SingleThreadExecutor;
    type Flags = ();
    type Message = Message;
    const APP_ID: &'static str = APP_ID;

    fn core(&self) -> &Core {
        &self.core
    }

    fn core_mut(&mut self) -> &mut Core {
        &mut self.core
    }

    fn init(core: Core, _flags: ()) -> (Self, Task<Message>) {
        let config = Config::new(APP_ID, 1).ok();
        let get = |key: &str, default: bool| {
            config.as_ref().and_then(|c| c.get::<bool>(key).ok()).unwrap_or(default)
        };
        let settings = Settings::new(
            get("enabled", true),
            !get("include_maximized", true),
            std::env::var_os("FSWS_DEBUG").is_some(),
        );
        let status = Arc::new(Mutex::new("Starting".to_string()));
        spawn_engine(settings.clone(), status.clone());
        let applet = Self {
            core,
            popup: None,
            config,
            settings,
            status,
            status_text: String::new(),
        };
        (applet, Task::none())
    }

    fn on_close_requested(&self, id: Id) -> Option<Message> {
        Some(Message::PopupClosed(id))
    }

    fn subscription(&self) -> Subscription<Message> {
        if self.popup.is_some() {
            time::every(Duration::from_millis(500)).map(|_| Message::Tick)
        } else {
            Subscription::none()
        }
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::TogglePopup => {
                if let Some(p) = self.popup.take() {
                    return destroy_popup(p);
                }
                self.status_text = self.status.lock().unwrap().clone();
                return cosmic::surface::surface_task(cosmic::surface::action::app_popup(
                    |_| Default::default(),
                    |app: &mut Self| {
                        let new_id = Id::unique();
                        app.popup = Some(new_id);
                        app.core.applet.get_popup_settings(
                            app.core.main_window_id().unwrap(),
                            new_id,
                            None,
                            None,
                            None,
                        )
                    },
                    None,
                ));
            }
            Message::PopupClosed(id) => {
                if self.popup == Some(id) {
                    self.popup = None;
                }
            }
            Message::SetEnabled(on) => {
                self.settings.enabled.store(on, Ordering::Relaxed);
                if let Some(c) = &self.config {
                    let _ = c.set("enabled", on);
                }
            }
            Message::SetIncludeMaximized(on) => {
                self.settings.fullscreen_only.store(!on, Ordering::Relaxed);
                if let Some(c) = &self.config {
                    let _ = c.set("include_maximized", on);
                }
            }
            Message::Tick => self.status_text = self.status.lock().unwrap().clone(),
        }
        Task::none()
    }

    fn view(&self) -> Element<'_, Message> {
        self.core
            .applet
            .icon_button(ICON)
            .on_press_down(Message::TogglePopup)
            .into()
    }

    fn view_window(&self, _id: Id) -> Element<'_, Message> {
        let content = column![
            padded_control(
                toggler(self.settings.enabled.load(Ordering::Relaxed))
                    .label("Fullscreen windows get their own workspace".to_string())
                    .on_toggle(Message::SetEnabled)
                    .width(Length::Fill)
            ),
            padded_control(
                toggler(!self.settings.fullscreen_only.load(Ordering::Relaxed))
                    .label("Maximized windows too".to_string())
                    .on_toggle(Message::SetIncludeMaximized)
                    .width(Length::Fill)
            ),
            padded_control(divider::horizontal::default()),
            padded_control(text::caption(self.status_text.clone())),
        ]
        .padding([8, 0]);
        self.core.applet.popup_container(content).into()
    }

    fn style(&self) -> Option<cosmic::iced::theme::Style> {
        Some(cosmic::applet::style())
    }
}
