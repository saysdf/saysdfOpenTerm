mod dbus;
mod macos;
mod windows;

#[derive(Debug, Clone)]
pub struct ToastNotification {
    pub title: String,
    pub message: String,
    pub url: Option<String>,
    pub timeout: Option<std::time::Duration>,
}

impl ToastNotification {
    pub fn show(self) {
        show(self)
    }
}

#[cfg(windows)]
use crate::windows as backend;
#[cfg(all(not(target_os = "macos"), not(windows)))]
use dbus as backend;
#[cfg(target_os = "macos")]
use macos as backend;

mod nop {
    use super::*;

    #[allow(dead_code)]
    pub fn show_notif(_: ToastNotification) -> Result<(), Box<dyn std::error::Error>> {
        Ok(())
    }
}

pub fn show(notif: ToastNotification) {
    if let Err(err) = backend::show_notif(notif) {
        log::error!("Failed to show notification: {}", err);
    }
}

// saysdfTerminal: activation callback (click on toast -> tab), change command-notifications D4.
// Supported on Windows; on macOS/dbus the toast is shown without a callback.
pub type ActivationCallback = Box<dyn Fn() + Send + Sync + 'static>;

// saysdfTerminal: show a toast with an activation callback (change command-notifications D4).
pub fn show_with_activation(notif: ToastNotification, on_activate: ActivationCallback) {
    #[cfg(windows)]
    let result = crate::windows::show_notif_with_activation(notif, Some(on_activate));
    #[cfg(not(windows))]
    let result = {
        drop(on_activate);
        backend::show_notif(notif)
    };
    if let Err(err) = result {
        log::error!("Failed to show notification: {}", err);
    }
}

pub fn persistent_toast_notification_with_click_to_open_url(title: &str, message: &str, url: &str) {
    show(ToastNotification {
        title: title.to_string(),
        message: message.to_string(),
        url: Some(url.to_string()),
        timeout: None,
    });
}

pub fn persistent_toast_notification(title: &str, message: &str) {
    show(ToastNotification {
        title: title.to_string(),
        message: message.to_string(),
        url: None,
        timeout: None,
    });
}

#[cfg(target_os = "macos")]
pub use macos::initialize as macos_initialize;
