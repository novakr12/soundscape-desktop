use crate::submit::Failure;
use crate::AppState;
use serde::Deserialize;
use std::time::Duration;
use tauri::{AppHandle, Manager};
use tauri_plugin_notification::NotificationExt;
use url::Url;

const EVERY: Duration = Duration::from_secs(60);
const FIRST: Duration = Duration::from_secs(10);
const AT_MOST: usize = 3;

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct Item {
    pub id: i64,
    pub read: bool,
    pub title: String,
    pub body: String,
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

pub fn summary(items: &[Item]) -> Vec<(String, String)> {
    if items.len() <= AT_MOST {
        return items
            .iter()
            .map(|item| (item.title.clone(), item.body.clone()))
            .collect();
    }
    let mut shown: Vec<(String, String)> = items[..AT_MOST - 1]
        .iter()
        .map(|item| (item.title.clone(), item.body.clone()))
        .collect();
    shown.push((
        "Soundscape".into(),
        format!("{} more notifications", items.len() - (AT_MOST - 1)),
    ));
    shown
}

pub async fn fetch(server: &Url, token: &str, after: Option<i64>) -> Result<Vec<Item>, Failure> {
    let mut address = server
        .join("/api/notifications")
        .map_err(|_| Failure::Refused(0))?;
    if let Some(after) = after {
        address
            .query_pairs_mut()
            .append_pair("after", &after.to_string());
    }
    let response = crate::server::client()
        .get(address)
        .header("authorization", format!("Token {token}"))
        .send()
        .await
        .map_err(|_| Failure::Offline)?;
    match response.status().as_u16() {
        200..=299 => response
            .json::<Answer>()
            .await
            .map(|answer| answer.notifications)
            .map_err(|_| Failure::Refused(200)),
        401 | 403 => Err(Failure::SignedOut),
        404 => Err(Failure::Refused(404)),
        _ => Err(Failure::Offline),
    }
}

pub async fn run(app: AppHandle) {
    tokio::time::sleep(FIRST).await;
    loop {
        check(&app).await;
        tokio::time::sleep(EVERY).await;
    }
}

pub async fn check(app: &AppHandle) {
    let state = app.state::<AppState>();
    let config = state.config();
    let (Some(server), Some(token)) = (
        config.server.as_deref().and_then(|s| Url::parse(s).ok()),
        config.token.clone(),
    ) else {
        return;
    };
    if !config.notifications || state.signed_out() {
        return;
    }
    match fetch(&server, &token, config.last_notification).await {
        Ok(items) => {
            let (show, last) = fresh(&items, config.last_notification);
            if last != config.last_notification {
                state.update(|config| config.last_notification = last);
            }
            for (title, body) in summary(&show) {
                let _ = app.notification().builder().title(title).body(body).show();
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
        assert_eq!(shown[0], ("Title 1".to_string(), "Body 1".to_string()));
        assert_eq!(shown[2].1, "8 more notifications");
    }
}
