//! The desktop's light or dark setting, read once at startup where winit reports none.

use std::time::Duration;

const LIMIT: Duration = Duration::from_millis(500);

/// The desktop's choice, or `None` when it doesn't say (or on systems where egui's own
/// `system_theme` is the answer). Never takes more than 500 ms.
pub fn desktop_mode() -> Option<egui::Theme> {
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    {
        within(LIMIT, read_portal)
    }
    #[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
    {
        let _ = LIMIT;
        None
    }
}

#[cfg(any(target_os = "linux", target_os = "freebsd", test))]
fn mode_of(color_scheme: u32) -> Option<egui::Theme> {
    match color_scheme {
        1 => Some(egui::Theme::Dark),
        2 => Some(egui::Theme::Light),
        _ => None,
    }
}

#[cfg(any(target_os = "linux", target_os = "freebsd", test))]
fn within(limit: Duration, read: impl FnOnce() -> Option<egui::Theme> + Send + 'static) -> Option<egui::Theme> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .name("desktop-mode".into())
        .spawn(move || {
            let _ = tx.send(read());
        })
        .ok()?;
    rx.recv_timeout(limit).ok().flatten()
}

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
fn read_portal() -> Option<egui::Theme> {
    use zbus::blocking::Connection;
    let bus = Connection::session().ok()?;
    let ask = |method: &str| {
        bus.call_method(
            Some("org.freedesktop.portal.Desktop"),
            "/org/freedesktop/portal/desktop",
            Some("org.freedesktop.portal.Settings"),
            method,
            &("org.freedesktop.appearance", "color-scheme"),
        )
        .ok()
        .and_then(|reply| reply.body().deserialize::<zbus::zvariant::OwnedValue>().ok())
        .and_then(|value| number_in(&value))
    };
    ask("ReadOne").or_else(|| ask("Read")).and_then(mode_of)
}

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
fn number_in(value: &zbus::zvariant::Value<'_>) -> Option<u32> {
    use zbus::zvariant::Value;
    match value {
        Value::U32(n) => Some(*n),
        Value::Value(inner) => number_in(inner),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_portals_values_mean_dark_light_or_nothing() {
        assert_eq!(mode_of(1), Some(egui::Theme::Dark));
        assert_eq!(mode_of(2), Some(egui::Theme::Light));
        assert_eq!(mode_of(0), None);
        assert_eq!(mode_of(7), None);
    }

    #[test]
    fn a_reader_that_never_answers_is_left_behind() {
        let (_keep, never) = std::sync::mpsc::channel::<()>();
        let started = std::time::Instant::now();
        let got = within(Duration::from_millis(50), move || {
            let _ = never.recv();
            Some(egui::Theme::Dark)
        });
        assert_eq!(got, None);
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn a_reader_that_panics_gives_nothing() {
        assert_eq!(within(Duration::from_secs(5), || panic!("no bus")), None);
    }
}
