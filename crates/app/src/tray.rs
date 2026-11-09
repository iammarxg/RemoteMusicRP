// SPDX-License-Identifier: AGPL-3.0-only
//! System tray (Windows, macOS; Linux is in `tray_linux`). The tray's event loop owns the main thread
//! (required on macOS); the async service and web UI run on a worker thread.
//!
//! Menu: Open RemoteMusicRP · Pause/Resume presence · Quit.

use std::sync::Arc;

use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};
use winit::application::ApplicationHandler;
use winit::event::{StartCause, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::WindowId;

use crate::app::App;
use crate::settings;

enum UserEvent {
    Menu(MenuEvent),
    /// The service thread finished (Ctrl+C, update restart, fatal error).
    Stopped,
}

/// The monochrome record icon (see `tray_art`).
fn icon() -> Option<Icon> {
    use crate::tray_art::{SIZE, rgba};
    Icon::from_rgba(rgba(), SIZE, SIZE).ok()
}

use crate::tray_common::open_url;

struct TrayApp {
    app: Arc<App>,
    url: String,
    tray: Option<TrayIcon>,
    open: MenuItem,
    pause: MenuItem,
    quit: MenuItem,
}

impl TrayApp {
    fn pause_label(&self) -> &'static str {
        if settings::get_bool(&self.app.db, settings::PAUSED) {
            "Resume presence"
        } else {
            "Pause presence"
        }
    }
}

impl ApplicationHandler<UserEvent> for TrayApp {
    fn new_events(&mut self, _el: &ActiveEventLoop, cause: StartCause) {
        // macOS: icons may only be created once the loop is running.
        if cause == StartCause::Init && self.tray.is_none() {
            let menu = Menu::new();
            self.pause.set_text(self.pause_label());
            let _ = menu.append_items(&[
                &self.open,
                &self.pause,
                &PredefinedMenuItem::separator(),
                &self.quit,
            ]);
            let mut b = TrayIconBuilder::new()
                .with_menu(Box::new(menu))
                .with_tooltip("RemoteMusicRP");
            if let Some(i) = icon() {
                b = b.with_icon(i);
            }
            match b.build() {
                Ok(t) => self.tray = Some(t),
                Err(e) => tracing::warn!(error = %e, "could not create the tray icon"),
            }
        }
    }

    fn resumed(&mut self, _el: &ActiveEventLoop) {}

    fn window_event(&mut self, _el: &ActiveEventLoop, _id: WindowId, _ev: WindowEvent) {}

    fn user_event(&mut self, el: &ActiveEventLoop, ev: UserEvent) {
        match ev {
            UserEvent::Menu(e) if e.id == *self.open.id() => open_url(&self.url),
            UserEvent::Menu(e) if e.id == *self.pause.id() => {
                let paused = !settings::get_bool(&self.app.db, settings::PAUSED);
                if settings::set_bool(&self.app.db, settings::PAUSED, paused).is_ok() {
                    self.app.reload.notify_one();
                }
                self.pause.set_text(self.pause_label());
            }
            UserEvent::Menu(e) if e.id == *self.quit.id() => {
                let _ = self.app.stop.send(true);
            }
            UserEvent::Menu(_) => {}
            UserEvent::Stopped => {
                self.tray = None;
                el.exit();
            }
        }
    }
}

/// Run the service on a worker thread and the tray on this (main) thread.
pub fn run(
    app: Arc<App>,
    serve: impl FnOnce(Arc<App>) -> Result<(), String> + Send + 'static,
) -> Result<(), String> {
    let event_loop = EventLoop::<UserEvent>::with_user_event()
        .build()
        .map_err(|e| format!("tray event loop: {e}"))?;
    event_loop.set_control_flow(ControlFlow::Wait);
    let proxy = event_loop.create_proxy();
    MenuEvent::set_event_handler(Some(move |e| {
        let _ = proxy.send_event(UserEvent::Menu(e));
    }));

    let proxy = event_loop.create_proxy();
    let worker_app = app.clone();
    let worker = std::thread::Builder::new()
        .name("service".into())
        .spawn(move || {
            let r = serve(worker_app);
            let _ = proxy.send_event(UserEvent::Stopped);
            r
        })
        .map_err(|e| format!("worker thread: {}", e.kind()))?;

    let url = crate::tray_common::ui_url(&app);
    let mut tray = TrayApp {
        app: app.clone(),
        url,
        tray: None,
        open: MenuItem::new("Open RemoteMusicRP", true, None),
        pause: MenuItem::new("Pause presence", true, None),
        quit: MenuItem::new("Quit", true, None),
    };
    let loop_result = event_loop
        .run_app(&mut tray)
        .map_err(|e| format!("tray: {e}"));
    let _ = app.stop.send(true);
    let worker_result = worker
        .join()
        .map_err(|_| "service thread panicked".to_owned())?;
    loop_result.and(worker_result)
}
