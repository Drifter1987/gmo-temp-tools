//! Native notification delivery for the background pollers.
//!
//! Toasts are sent from a worker thread so a slow shell cannot stall an SMS,
//! mail or webhook observer. The app is not installed with a Start-menu
//! shortcut, so the documented PowerShell AUMID is used; the toast still comes
//! from the local GMO TempTools process and carries our title/body.

pub fn notify(title: &str, body: &str) {
    if std::env::var("TEMPTOOLS_DISABLE_NOTIFICATIONS")
        .map(|value| value == "1" || value.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
    {
        return;
    }

    let title = clip(title, 64);
    let body = clip(body, 220);
    std::thread::spawn(move || {
        #[cfg(windows)]
        {
            use tauri_winrt_notification::Toast;
            match Toast::new(Toast::POWERSHELL_APP_ID)
                .title(&title)
                .text1(&body)
                .show()
            {
                Ok(()) => eprintln!("desktop notification delivered: {title}"),
                Err(error) => eprintln!("desktop notification failed: {error}"),
            }
        }
        #[cfg(not(windows))]
        {
            let _ = (&title, &body);
        }
    });
}

fn clip(value: &str, limit: usize) -> String {
    let mut out: String = value.chars().take(limit).collect();
    if value.chars().count() > limit {
        out.push('…');
    }
    out
}
