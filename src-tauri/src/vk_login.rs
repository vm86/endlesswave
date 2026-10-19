//! Signing in to VK in a window of the app. The user signs in on the VK site as in any browser (a password, a code, a QR);
//! the cookies of the session are then read from the window and given to the page, which hands them to the radio. The
//! window keeps nothing (its data is private to it and gone when it closes).

use serde::Serialize;
use std::time::Duration;
use tauri::{AppHandle, Manager, Url, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

const LABEL: &str = "vk-login";
const START: &str = "https://vk.ru/audio";
/// The hosts whose cookies the session is made of.
const HOSTS: [&str; 2] = ["https://vk.ru/", "https://login.vk.ru/"];
const TIMEOUT: Duration = Duration::from_secs(600);
const POLL: Duration = Duration::from_millis(1000);

#[derive(Serialize)]
pub struct VkSession {
    /// The `Cookie` header of the session.
    cookie: String,
    /// The cookie `remixuserid`, when the site sets it; the radio finds out whose the session is when it asks for a token.
    user_id: Option<String>,
}

/// The cookies of the hosts as a `Cookie` header, and the cookie `remixuserid` if the session is there.
async fn session(window: &WebviewWindow) -> Option<VkSession> {
    let mut cookies: Vec<(String, String)> = Vec::new();
    for host in HOSTS {
        let url = Url::parse(host).ok()?;
        let found = match window.cookies_for_url(url) {
            Ok(found) => found,
            Err(error) => {
                eprintln!("[vk-login] cookies of {host}: error {error}");
                return None;
            }
        };
        eprintln!(
            "[vk-login] cookies of {host}: {}",
            found
                .iter()
                .map(|c| format!(
                    "{}({}{})",
                    c.name(),
                    c.value().len(),
                    if c.http_only() == Some(true) {
                        ",httponly"
                    } else {
                        ""
                    }
                ))
                .collect::<Vec<_>>()
                .join(" ")
        );
        for cookie in found {
            let (name, value) = (cookie.name().to_owned(), cookie.value().to_owned());
            if !value.is_empty() && !cookies.iter().any(|(n, _)| *n == name) {
                cookies.push((name, value));
            }
        }
    }
    let value = |name: &str| {
        cookies
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.clone())
    };
    value("remixsid").or_else(|| value("remixsid6"))?;
    let user_id = value("remixuserid").filter(|u| u.parse::<u64>().is_ok());
    let cookie = cookies
        .iter()
        .map(|(n, v)| format!("{n}={v}"))
        .collect::<Vec<_>>()
        .join("; ");
    Some(VkSession { cookie, user_id })
}

/// Open the VK site in a window and wait until the user has signed in (or closed the window).
#[tauri::command]
pub async fn vk_login(app: AppHandle) -> Result<VkSession, String> {
    if let Some(open) = app.get_webview_window(LABEL) {
        let _ = open.close();
    }
    let url = Url::parse(START).map_err(|e| e.to_string())?;
    let window = WebviewWindowBuilder::new(&app, LABEL, WebviewUrl::External(url))
        .title("Sign in to VK")
        .inner_size(520.0, 760.0)
        .incognito(true)
        .build()
        .map_err(|e| format!("could not open the window: {e}"))?;
    let started = tokio::time::Instant::now();
    let result = loop {
        tokio::time::sleep(POLL).await;
        if app.get_webview_window(LABEL).is_none() {
            break Err("the window was closed".to_owned());
        }
        if let Some(found) = session(&window).await {
            break Ok(found);
        }
        if started.elapsed() > TIMEOUT {
            break Err("timed out".to_owned());
        }
    };
    let _ = window.close();
    result
}
