use crate::IDENTIFIER;
use zbus::fdo::{RequestNameFlags, RequestNameReply};
use zbus::{interface, Connection};

pub const PATH: &str = "/io/github/novakr12/SoundscapeDesktop";

pub type Wanted = Box<dyn Fn() + Send + Sync>;

struct Window(Wanted);

#[interface(name = "io.github.novakr12.SoundscapeDesktop")]
impl Window {
    fn show(&self) {
        (self.0)();
    }
}

pub enum Claim {
    First(Option<Connection>),
    Running(Connection),
}

pub async fn claim(show: Wanted) -> Claim {
    let Ok(connection) = crate::bus::session().await else {
        return Claim::First(None);
    };
    if connection
        .object_server()
        .at(PATH, Window(show))
        .await
        .is_err()
    {
        return Claim::First(Some(connection));
    }
    match connection
        .request_name_with_flags(IDENTIFIER, RequestNameFlags::DoNotQueue.into())
        .await
    {
        Ok(RequestNameReply::Exists | RequestNameReply::InQueue) | Err(zbus::Error::NameTaken) => {
            Claim::Running(connection)
        }
        _ => Claim::First(Some(connection)),
    }
}

pub async fn wake(connection: &Connection) -> bool {
    connection
        .call_method(Some(IDENTIFIER), PATH, Some(IDENTIFIER), "Show", &())
        .await
        .is_ok()
}
