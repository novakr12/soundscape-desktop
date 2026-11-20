use crate::ipc::{self, FromWindow, ToWindow};
use soundscape_core::host::Reply;
use std::cell::{Cell, RefCell};
use std::io::Write;
use std::sync::{Arc, Mutex};
use url::Url;

pub enum Call {
    Eval(String),
    Evaluate(String, Reply),
    Show,
    Hide,
    Focus,
    Navigate(Url),
    Server(Option<Url>),
    Setup,
    Smooth(bool),
    Memory(u32),
    KeepWindow(i64),
    Exit,
}

pub type Out = Arc<Mutex<Box<dyn Write + Send>>>;

pub fn tell(out: &Out, message: &FromWindow) {
    if let Ok(mut out) = out.lock() {
        ipc::send(&mut *out, message);
    }
}

pub fn call_for(message: ToWindow, out: &Out) -> Option<Call> {
    Some(match message {
        ToWindow::Start(_) => return None,
        ToWindow::Eval { script } => Call::Eval(script),
        ToWindow::Evaluate { id, script } => {
            let out = out.clone();
            Call::Evaluate(
                script,
                Box::new(move |text| tell(&out, &FromWindow::Answer { id, text })),
            )
        }
        ToWindow::Show => Call::Show,
        ToWindow::Hide => Call::Hide,
        ToWindow::Focus => Call::Focus,
        ToWindow::Navigate { url } => Call::Navigate(Url::parse(&url).ok()?),
        ToWindow::Server { url } => Call::Server(url.and_then(|url| Url::parse(&url).ok())),
        ToWindow::Setup => Call::Setup,
        ToWindow::Smooth { on } => Call::Smooth(on),
        ToWindow::Memory { mb } => Call::Memory(mb),
        ToWindow::KeepWindow { seconds } => Call::KeepWindow(seconds),
        ToWindow::Exit => Call::Exit,
    })
}

pub struct Link {
    out: Out,
    pub server: RefCell<Option<Url>>,
    told_url: RefCell<Option<Url>>,
    told_hidden: Cell<Option<bool>>,
}

impl Link {
    pub fn new(out: Out, server: Option<Url>) -> Link {
        Link {
            out,
            server: RefCell::new(server),
            told_url: RefCell::new(None),
            told_hidden: Cell::new(None),
        }
    }

    pub fn send(&self, message: FromWindow) {
        tell(&self.out, &message);
    }

    pub fn url(&self, url: &Url) {
        if self.told_url.borrow().as_ref() == Some(url) {
            return;
        }
        *self.told_url.borrow_mut() = Some(url.clone());
        self.send(FromWindow::Url {
            url: url.to_string(),
        });
    }

    pub fn hidden(&self, hidden: bool) {
        if self.told_hidden.replace(Some(hidden)) == Some(hidden) {
            return;
        }
        self.send(FromWindow::Hidden { hidden });
    }
}

pub fn text(value: &servo::JSValue) -> Option<String> {
    match value {
        servo::JSValue::String(text) => Some(text.clone()),
        servo::JSValue::Boolean(on) => Some(on.to_string()),
        servo::JSValue::Number(number) => Some(number.to_string()),
        servo::JSValue::Null => Some("null".into()),
        servo::JSValue::Undefined => Some("undefined".into()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wire() -> (Out, Arc<Mutex<Vec<u8>>>) {
        struct Shared(Arc<Mutex<Vec<u8>>>);
        impl Write for Shared {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(bytes);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let bytes = Arc::new(Mutex::new(Vec::new()));
        (Arc::new(Mutex::new(Box::new(Shared(bytes.clone())))), bytes)
    }

    #[test]
    fn answers_scripts_with_their_number() {
        let (out, bytes) = wire();
        let Some(Call::Evaluate(script, reply)) = call_for(
            ToWindow::Evaluate {
                id: 4,
                script: "2 + 2".into(),
            },
            &out,
        ) else {
            panic!("evaluate");
        };
        assert_eq!(script, "2 + 2");
        reply(Some("4".into()));
        let sent = String::from_utf8(bytes.lock().unwrap().clone()).unwrap();
        assert_eq!(sent, "{\"do\":\"answer\",\"id\":4,\"text\":\"4\"}\n");
        assert!(
            call_for(
                ToWindow::Navigate {
                    url: "not a url".into()
                },
                &out
            )
            .is_none()
        );
    }

    #[test]
    fn tells_the_app_about_changes_only_once() {
        let (out, bytes) = wire();
        let link = Link::new(out, None);
        let page = Url::parse("https://music.example.com/stats").unwrap();
        link.url(&page);
        link.url(&page);
        link.hidden(true);
        link.hidden(true);
        link.hidden(false);
        let sent = String::from_utf8(bytes.lock().unwrap().clone()).unwrap();
        assert_eq!(sent.lines().count(), 3);
    }
}
