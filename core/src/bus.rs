use tokio::sync::Mutex;
use zbus::Connection;

static SESSION: Mutex<Option<Connection>> = Mutex::const_new(None);

pub async fn session() -> zbus::Result<Connection> {
    let mut guard = SESSION.lock().await;
    if let Some(connection) = guard.as_ref() {
        if !connection.is_closed() {
            return Ok(connection.clone());
        }
    }
    let connection = Connection::session().await?;
    *guard = Some(connection.clone());
    Ok(connection)
}

pub async fn reset(stale: &Connection) {
    let mut guard = SESSION.lock().await;
    if guard
        .as_ref()
        .is_some_and(|current| current.unique_name() == stale.unique_name())
    {
        *guard = None;
    }
}
