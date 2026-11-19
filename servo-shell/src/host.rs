use soundscape_desktop::host::{Host, Reply, Result, Running, WindowOptions};
use soundscape_desktop::mini::MINI;
use soundscape_desktop::{MAIN, SETUP};
use std::sync::{Arc, Mutex};
use url::Url;
use winit::event_loop::EventLoopProxy;

pub enum Call {
    Eval(String),
    Evaluate(String, Reply),
    Show,
    Hide,
    Focus,
    Navigate(Url),
    Setup,
    Rest,
    Prepare(String),
    OpenMini,
    MiniEval(String),
    CloseMini,
    Main(Box<dyn FnOnce() + Send>),
    Smooth(bool),
    Memory(u32),
    Restart,
    Exit,
}

#[derive(Default)]
pub struct Seen {
    pub hidden: bool,
    pub mini: bool,
    pub url: Option<Url>,
}

pub struct ServoHost {
    pub proxy: EventLoopProxy<crate::Wake>,
    pub seen: Arc<Mutex<Seen>>,
    pub running: Running,
}

impl ServoHost {
    fn call(&self, call: Call) {
        let _ = self.proxy.send_event(crate::Wake::Call(call));
    }

    fn mini_open(&self) -> bool {
        self.seen.lock().map(|seen| seen.mini).unwrap_or(false)
    }
}

impl Host for ServoHost {
    fn exists(&self, label: &str) -> bool {
        label == MAIN || (label == MINI && self.mini_open())
    }

    fn eval(&self, label: &str, script: String) {
        match label {
            MAIN => self.call(Call::Eval(script)),
            MINI => self.call(Call::MiniEval(script)),
            _ => {}
        }
    }

    fn evaluate(&self, label: &str, script: String, reply: Reply) {
        if label == MAIN {
            self.call(Call::Evaluate(script, reply));
        } else {
            reply(None);
        }
    }

    fn show(&self, label: &str) {
        match label {
            MAIN => self.call(Call::Show),
            MINI => self.call(Call::OpenMini),
            _ => {}
        }
    }

    fn hide(&self, label: &str) {
        match label {
            MAIN => self.call(Call::Hide),
            MINI => self.call(Call::CloseMini),
            _ => {}
        }
    }

    fn focus(&self, label: &str) {
        match label {
            MAIN => self.call(Call::Focus),
            MINI => self.call(Call::OpenMini),
            _ => {}
        }
    }

    fn close(&self, label: &str) {
        self.hide(label);
    }

    fn is_visible(&self, label: &str) -> bool {
        match label {
            MAIN => !self.seen.lock().map(|seen| seen.hidden).unwrap_or(false),
            MINI => self.mini_open(),
            _ => false,
        }
    }

    fn url(&self, label: &str) -> Option<Url> {
        if label != MAIN {
            return None;
        }
        self.seen.lock().ok()?.url.clone()
    }

    fn navigate(&self, label: &str, url: Url) {
        if label == MAIN {
            self.call(Call::Navigate(url));
        }
    }

    fn rest(&self, label: &str) {
        if label == MAIN {
            self.call(Call::Rest);
        }
    }

    fn prepare(&self, label: &str, script: String) {
        if label == MAIN {
            self.call(Call::Prepare(script));
        }
    }

    fn open_window(&self, label: &str, _page: &str, _options: &WindowOptions) -> Result<()> {
        match label {
            SETUP => self.call(Call::Setup),
            MINI => self.call(Call::OpenMini),
            _ => {
                return Err(soundscape_desktop::host::Error(format!(
                    "There is no {label} window"
                )));
            }
        }
        Ok(())
    }

    fn run_on_main(&self, work: Box<dyn FnOnce() + Send>) {
        self.call(Call::Main(work));
    }

    fn smooth_scrolling(&self, on: bool) {
        self.call(Call::Smooth(on));
    }

    fn memory_limit(&self, mb: u32) {
        self.call(Call::Memory(mb));
    }

    fn running(&self) -> Running {
        self.running
    }

    fn restart(&self) {
        self.call(Call::Restart);
    }

    fn exit(&self, _code: i32) {
        self.call(Call::Exit);
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
