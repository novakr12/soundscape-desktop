use crate::host::{AppHandle, Manager};
use crate::MAIN;
use std::time::Duration;
use tokio::sync::oneshot;

pub async fn evaluate(app: &AppHandle, script: String) -> Option<String> {
    let window = app.get_webview_window(MAIN)?;
    let (send, receive) = oneshot::channel::<Option<String>>();
    window.evaluate(
        script,
        Box::new(move |result| {
            let _ = send.send(result);
        }),
    );
    tokio::time::timeout(Duration::from_secs(2), receive)
        .await
        .ok()?
        .ok()?
}
