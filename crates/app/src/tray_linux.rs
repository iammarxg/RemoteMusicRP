// SPDX-License-Identifier: AGPL-3.0-only
//! Linux tray icon: a StatusNotifierItem over the session D-Bus (KDE, and
//! GNOME with the AppIndicator extension, Cinnamon, XFCE, …). Pure Rust via
//! `ksni`; no GTK. It runs as a task next to the service.
//!
//! Menu: Open RemoteMusicRP · Pause/Resume presence · Quit. A left click
//! opens the web UI.
//!
//! If the desktop has no tray (no StatusNotifierWatcher), the web UI is
//! opened in the browser once instead, so a desktop start never leaves the
//! user without a way in.

use std::sync::Arc;

use ksni::menu::StandardItem;
use ksni::{Icon, MenuItem, ToolTip, Tray, TrayMethods};

use crate::app::App;
use crate::settings;
use crate::tray_common::{open_url, ui_url};

struct LinuxTray {
    app: Arc<App>,
    url: String,
}

impl LinuxTray {
    fn paused(&self) -> bool {
        settings::get_bool(&self.app.db, settings::PAUSED)
    }
}

impl Tray for LinuxTray {
    fn id(&self) -> String {
        "remotemusicrp".into()
    }

    fn title(&self) -> String {
        "RemoteMusicRP".into()
    }

    fn icon_pixmap(&self) -> Vec<Icon> {
        let size = i32::try_from(crate::tray_art::SIZE).unwrap_or(32);
        vec![Icon {
            width: size,
            height: size,
            data: crate::tray_art::argb(),
        }]
    }

    fn tool_tip(&self) -> ToolTip {
        ToolTip {
            title: "RemoteMusicRP".into(),
            description: if self.paused() {
                "Presence paused".into()
            } else {
                String::new()
            },
            ..Default::default()
        }
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        open_url(&self.url);
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        let pause_label = if self.paused() {
            "Resume presence"
        } else {
            "Pause presence"
        };
        vec![
            StandardItem {
                label: "Open RemoteMusicRP".into(),
                activate: Box::new(|t: &mut Self| open_url(&t.url)),
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: pause_label.into(),
                activate: Box::new(|t: &mut Self| {
                    let paused = !t.paused();
                    if settings::set_bool(&t.app.db, settings::PAUSED, paused).is_ok() {
                        t.app.reload.notify_one();
                    }
                }),
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            StandardItem {
                label: "Quit".into(),
                activate: Box::new(|t: &mut Self| {
                    let _ = t.app.stop.send(true);
                }),
                ..Default::default()
            }
            .into(),
        ]
    }
}

/// Show the tray until the app stops. Call from inside the tokio runtime.
pub async fn run(app: Arc<App>) {
    if std::env::var_os("DBUS_SESSION_BUS_ADDRESS").is_none_or(|v| v.is_empty()) {
        // No desktop session (SSH, service, container): stay headless.
        tracing::info!("no desktop session: running without a tray icon");
        return;
    }
    let url = ui_url(&app);
    let mut stop = app.stop.subscribe();
    let tray = LinuxTray {
        app: app.clone(),
        url: url.clone(),
    };
    match tray.spawn().await {
        Ok(handle) => {
            tracing::info!("tray icon shown");
            let _ = stop.wait_for(|s| *s).await;
            handle.shutdown().await;
        }
        Err(e) => {
            tracing::info!(reason = %e, "no system tray on this desktop; opening the web UI instead");
            open_url(&url);
        }
    }
}
