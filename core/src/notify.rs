use crate::host::{AppHandle, Manager};
use crate::submit::Failure;
use crate::AppState;
use futures_util::StreamExt;
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;
use tokio::sync::OnceCell;
use url::Url;
use zbus::message::Type;
use zbus::{Connection, MatchRule, MessageStream};

const EVERY: Duration = Duration::from_secs(60);
const FIRST: Duration = Duration::from_secs(10);
const AT_MOST: usize = 3;
const REMEMBERED: usize = 50;
const NOTIFICATIONS: &str = "org.freedesktop.Notifications";
const NOTIFICATIONS_PATH: &str = "/org/freedesktop/Notifications";
const OPEN_ACTION: &str = "default";
pub const NOTIFICATIONS_PAGE: &str = "/notifications";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Click {
    Window,
    Page(String),
    Downloads,
}

static BUS: OnceCell<Connection> = OnceCell::const_new();
static WAITING: Mutex<Vec<(u32, Click)>> = Mutex::new(Vec::new());

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct Item {
    pub id: i64,
    pub read: bool,
    pub title: String,
    pub body: String,
    #[serde(default)]
    pub url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Answer {
    notifications: Vec<Item>,
}

pub fn fresh(items: &[Item], last_seen: Option<i64>) -> (Vec<Item>, Option<i64>) {
    let newest = items.iter().map(|item| item.id).max();
    let Some(last) = last_seen else {
        return (Vec::new(), newest.or(Some(0)));
    };
    let mut show: Vec<Item> = items
        .iter()
        .filter(|item| item.id > last && !item.read)
        .cloned()
        .collect();
    show.sort_by_key(|item| item.id);
    (show, Some(newest.map_or(last, |newest| newest.max(last))))
}

fn shown(item: &Item) -> (String, String, Click) {
    let click = item
        .url
        .clone()
        .filter(|url| url.starts_with('/') && !url.starts_with("//"))
        .unwrap_or_else(|| NOTIFICATIONS_PAGE.into());
    (item.title.clone(), item.body.clone(), Click::Page(click))
}

pub fn summary(items: &[Item]) -> Vec<(String, String, Click)> {
    if items.len() <= AT_MOST {
        return items.iter().map(shown).collect();
    }
    let mut all: Vec<(String, String, Click)> = items[..AT_MOST - 1].iter().map(shown).collect();
    all.push((
        "Soundscape".into(),
        format!("{} more notifications", items.len() - (AT_MOST - 1)),
        Click::Page(NOTIFICATIONS_PAGE.into()),
    ));
    all
}

pub fn remember(waiting: &mut Vec<(u32, Click)>, id: u32, click: Click) {
    waiting.retain(|(seen, _)| *seen != id);
    waiting.push((id, click));
    if waiting.len() > REMEMBERED {
        let extra = waiting.len() - REMEMBERED;
        waiting.drain(..extra);
    }
}

pub fn clicked(waiting: &mut Vec<(u32, Click)>, id: u32) -> Option<Click> {
    let at = waiting.iter().position(|(seen, _)| *seen == id)?;
    Some(waiting.remove(at).1)
}

fn open(app: &AppHandle, click: Click) {
    match click {
        Click::Window => crate::show(app),
        Click::Downloads => crate::updates::open_downloads(app),
        Click::Page(path) => match app
            .state::<AppState>()
            .server()
            .and_then(|server| server.join(&path).ok())
        {
            Some(page) => {
                let _ = crate::open_main(app, page);
            }
            None => crate::show(app),
        },
    }
}

async fn listen(app: AppHandle, connection: Connection) -> zbus::Result<()> {
    let rule = MatchRule::builder()
        .msg_type(Type::Signal)
        .interface(NOTIFICATIONS)?
        .member("ActionInvoked")?
        .build();
    let mut stream = MessageStream::for_match_rule(rule, &connection, None).await?;
    while let Some(Ok(message)) = stream.next().await {
        let Ok((id, action)) = message.body().deserialize::<(u32, String)>() else {
            continue;
        };
        if action != OPEN_ACTION {
            continue;
        }
        let click = WAITING
            .lock()
            .ok()
            .and_then(|mut waiting| clicked(&mut waiting, id));
        if let Some(click) = click {
            open(&app, click);
        }
    }
    Ok(())
}

async fn bus(app: &AppHandle) -> zbus::Result<&'static Connection> {
    BUS.get_or_try_init(|| async {
        let connection = Connection::session().await?;
        let listener = connection.clone();
        let app = app.clone();
        crate::host::async_runtime::spawn(async move {
            if let Err(error) = listen(app, listener).await {
                eprintln!("Couldn't follow clicks on notifications: {error}");
            }
        });
        Ok(connection)
    })
    .await
}

async fn send(app: &AppHandle, title: &str, body: &str, click: Click) -> zbus::Result<()> {
    let connection = bus(app).await?;
    let hints: HashMap<&str, zbus::zvariant::Value<'_>> = HashMap::from([(
        "desktop-entry",
        zbus::zvariant::Value::from(crate::host::APP_ID),
    )]);
    let reply = connection
        .call_method(
            Some(NOTIFICATIONS),
            NOTIFICATIONS_PATH,
            Some(NOTIFICATIONS),
            "Notify",
            &(
                "Soundscape",
                0u32,
                crate::host::APP_ID,
                title,
                body,
                vec![OPEN_ACTION, "Open"],
                hints,
                -1i32,
            ),
        )
        .await?;
    let id: u32 = reply.body().deserialize()?;
    if let Ok(mut waiting) = WAITING.lock() {
        remember(&mut waiting, id, click);
    }
    Ok(())
}

pub async fn fetch(server: &Url, token: &str, after: Option<i64>) -> Result<Vec<Item>, Failure> {
    let after = after.map(|after| after.to_string());
    let query: Vec<(&str, &str)> = after
        .iter()
        .map(|after| ("after", after.as_str()))
        .collect();
    crate::server::get_json::<Answer>(server, token, "/api/notifications", &query)
        .await
        .map(|answer| answer.notifications)
}

pub async fn show(app: &AppHandle, title: &str, body: &str, click: Click) -> Result<(), String> {
    send(app, title, body, click).await.map_err(|error| {
        eprintln!("Couldn't show the notification \"{title}\": {error}");
        error.to_string()
    })
}

pub async fn run(app: AppHandle) {
    crate::server::poll(app, FIRST, EVERY, |app| async move { check(&app).await }).await;
}

pub async fn check(app: &AppHandle) {
    let state = app.state::<AppState>();
    let config = state.config();
    let Some((server, token)) = crate::server::credentials(app) else {
        return;
    };
    if !config.notifications {
        return;
    }
    match fetch(&server, &token, config.last_notification).await {
        Ok(items) => {
            let (show, last) = fresh(&items, config.last_notification);
            if last != config.last_notification {
                state.update(|config| config.last_notification = last);
            }
            for (title, body, click) in summary(&show) {
                let _ = self::show(app, &title, &body, click).await;
            }
        }
        Err(Failure::SignedOut) => {
            state.set_signed_out(true);
            crate::tray::refresh(app);
        }
        Err(_) => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: i64, read: bool) -> Item {
        Item {
            id,
            read,
            title: format!("Title {id}"),
            body: format!("Body {id}"),
            url: Some(format!("/u/friend{id}")),
        }
    }

    #[test]
    fn the_first_check_only_remembers_where_things_are() {
        assert_eq!(
            fresh(&[item(5, false), item(3, false)], None),
            (vec![], Some(5))
        );
        assert_eq!(fresh(&[], None), (vec![], Some(0)));
    }

    #[test]
    fn shows_new_unread_ones_oldest_first() {
        let items = vec![
            item(9, false),
            item(8, true),
            item(7, false),
            item(4, false),
        ];
        let (show, last) = fresh(&items, Some(5));
        assert_eq!(
            show.iter().map(|item| item.id).collect::<Vec<_>>(),
            vec![7, 9]
        );
        assert_eq!(last, Some(9));
        assert_eq!(fresh(&[], Some(9)), (vec![], Some(9)));
    }

    #[test]
    fn folds_a_burst_into_a_few_notifications() {
        let three: Vec<_> = (1..=3).map(|id| item(id, false)).collect();
        assert_eq!(summary(&three).len(), 3);
        let ten: Vec<_> = (1..=10).map(|id| item(id, false)).collect();
        let shown = summary(&ten);
        assert_eq!(shown.len(), 3);
        assert_eq!(
            shown[0],
            (
                "Title 1".to_string(),
                "Body 1".to_string(),
                Click::Page("/u/friend1".into())
            )
        );
        assert_eq!(shown[2].1, "8 more notifications");
        assert_eq!(shown[2].2, Click::Page(NOTIFICATIONS_PAGE.into()));
    }

    #[test]
    fn opens_only_pages_on_your_own_server() {
        let mut far = item(1, false);
        far.url = Some("https://evil.example.org/".into());
        assert_eq!(summary(&[far])[0].2, Click::Page(NOTIFICATIONS_PAGE.into()));
        let mut sneaky = item(2, false);
        sneaky.url = Some("//evil.example.org/".into());
        assert_eq!(
            summary(&[sneaky])[0].2,
            Click::Page(NOTIFICATIONS_PAGE.into())
        );
        let mut plain = item(3, false);
        plain.url = None;
        assert_eq!(
            summary(&[plain])[0].2,
            Click::Page(NOTIFICATIONS_PAGE.into())
        );
    }

    #[test]
    fn remembers_where_each_notification_leads() {
        let mut waiting = Vec::new();
        remember(&mut waiting, 4, Click::Window);
        remember(&mut waiting, 5, Click::Downloads);
        assert_eq!(clicked(&mut waiting, 5), Some(Click::Downloads));
        assert_eq!(clicked(&mut waiting, 5), None);
        for id in 10..10 + REMEMBERED as u32 {
            remember(&mut waiting, id, Click::Window);
        }
        assert_eq!(waiting.len(), REMEMBERED);
        assert_eq!(clicked(&mut waiting, 4), None);
    }
}
