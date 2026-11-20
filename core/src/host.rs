use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::fmt;
use std::future::Future;
use std::ops::Deref;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex, OnceLock};
use url::Url;

#[derive(Debug, Clone)]
pub struct Error(pub String);

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;

pub type Reply = Box<dyn FnOnce(Option<String>) + Send>;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Running {
    pub cpu_rendering: bool,
    pub faster_scripts: bool,
}

pub trait Host: Send + Sync {
    fn exists(&self, label: &str) -> bool;
    fn eval(&self, label: &str, script: String);
    fn evaluate(&self, label: &str, script: String, reply: Reply);
    fn show(&self, label: &str);
    fn hide(&self, label: &str);
    fn focus(&self, label: &str);
    fn close(&self, label: &str);
    fn is_visible(&self, label: &str) -> bool;
    fn url(&self, label: &str) -> Option<Url>;
    fn navigate(&self, label: &str, url: Url);
    fn open_window(&self, label: &str, page: &str, options: &WindowOptions) -> Result<()>;
    fn run_on_main(&self, work: Box<dyn FnOnce() + Send>);
    fn smooth_scrolling(&self, on: bool);
    fn memory_limit(&self, mb: u32);
    fn running(&self) -> Running;
    fn restart(&self);
    fn exit(&self, code: i32);
}

pub mod async_runtime {
    use super::*;

    pub use tokio::task::JoinHandle;

    pub fn runtime() -> &'static tokio::runtime::Runtime {
        static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
        RUNTIME.get_or_init(|| {
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .thread_name("soundscape-tasks")
                .enable_all()
                .build()
                .expect("task runtime")
        })
    }

    pub fn spawn<F>(work: F) -> JoinHandle<F::Output>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        runtime().spawn(work)
    }

    pub fn block_on<F: Future>(work: F) -> F::Output {
        match tokio::runtime::Handle::try_current() {
            Ok(_) => tokio::task::block_in_place(|| runtime().block_on(work)),
            Err(_) => runtime().block_on(work),
        }
    }
}

struct Inner {
    host: Box<dyn Host>,
    states: Mutex<HashMap<TypeId, &'static (dyn Any + Send + Sync)>>,
}

#[derive(Clone)]
pub struct AppHandle {
    inner: Arc<Inner>,
}

impl AppHandle {
    pub fn new(host: Box<dyn Host>) -> AppHandle {
        AppHandle {
            inner: Arc::new(Inner {
                host,
                states: Mutex::new(HashMap::new()),
            }),
        }
    }

    pub fn host(&self) -> &dyn Host {
        self.inner.host.as_ref()
    }

    pub fn restart(&self) {
        self.inner.host.restart();
    }

    pub fn exit(&self, code: i32) {
        self.inner.host.exit(code);
    }

    pub fn run_on_main_thread<F: FnOnce() + Send + 'static>(&self, work: F) -> Result<()> {
        self.inner.host.run_on_main(Box::new(work));
        Ok(())
    }
}

pub struct State<'a, T: 'static>(&'a T);

impl<T: 'static> Deref for State<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        self.0
    }
}

pub trait Manager {
    fn app_handle(&self) -> &AppHandle;

    fn state<T: Send + Sync + 'static>(&self) -> State<'static, T> {
        self.try_state::<T>().expect("state was not managed")
    }

    fn try_state<T: Send + Sync + 'static>(&self) -> Option<State<'static, T>> {
        let states = self.app_handle().inner.states.lock().expect("states lock");
        let found: &'static (dyn Any + Send + Sync) = *states.get(&TypeId::of::<T>())?;
        found.downcast_ref::<T>().map(State)
    }

    fn manage<T: Send + Sync + 'static>(&self, value: T) -> bool {
        let mut states = self.app_handle().inner.states.lock().expect("states lock");
        if states.contains_key(&TypeId::of::<T>()) {
            return false;
        }
        let kept: &'static T = Box::leak(Box::new(value));
        states.insert(TypeId::of::<T>(), kept);
        true
    }

    fn get_webview_window(&self, label: &str) -> Option<WebviewWindow> {
        let app = self.app_handle();
        app.inner.host.exists(label).then(|| WebviewWindow {
            app: app.clone(),
            label: label.to_string(),
        })
    }
}

impl Manager for AppHandle {
    fn app_handle(&self) -> &AppHandle {
        self
    }
}

#[derive(Clone)]
pub struct WebviewWindow {
    app: AppHandle,
    label: String,
}

impl WebviewWindow {
    pub fn label(&self) -> &str {
        &self.label
    }

    fn host(&self) -> &dyn Host {
        self.app.host()
    }

    pub fn eval(&self, script: impl Into<String>) -> Result<()> {
        self.host().eval(&self.label, script.into());
        Ok(())
    }

    pub fn show(&self) -> Result<()> {
        self.host().show(&self.label);
        Ok(())
    }

    pub fn hide(&self) -> Result<()> {
        self.host().hide(&self.label);
        Ok(())
    }

    pub fn set_focus(&self) -> Result<()> {
        self.host().focus(&self.label);
        Ok(())
    }

    pub fn unminimize(&self) -> Result<()> {
        self.host().show(&self.label);
        Ok(())
    }

    pub fn is_visible(&self) -> Result<bool> {
        Ok(self.host().is_visible(&self.label))
    }

    pub fn close(&self) -> Result<()> {
        self.host().close(&self.label);
        Ok(())
    }

    pub fn destroy(&self) -> Result<()> {
        self.host().close(&self.label);
        Ok(())
    }

    pub fn url(&self) -> Result<Url> {
        self.host()
            .url(&self.label)
            .ok_or_else(|| Error("no page".into()))
    }

    pub fn navigate(&self, url: Url) -> Result<()> {
        self.host().navigate(&self.label, url);
        Ok(())
    }

    pub fn evaluate(&self, script: impl Into<String>, reply: Reply) {
        self.host().evaluate(&self.label, script.into(), reply);
    }
}

impl Manager for WebviewWindow {
    fn app_handle(&self) -> &AppHandle {
        &self.app
    }
}

pub enum WebviewUrl {
    App(std::path::PathBuf),
    External(Url),
}

#[derive(Clone, Debug, Default)]
pub struct WindowOptions {
    pub title: String,
    pub width: f64,
    pub height: f64,
    pub resizable: bool,
    pub always_on_top: bool,
}

pub struct WebviewWindowBuilder {
    app: AppHandle,
    label: String,
    page: String,
    options: WindowOptions,
}

impl WebviewWindowBuilder {
    pub fn new<M: Manager>(
        manager: &M,
        label: impl Into<String>,
        url: WebviewUrl,
    ) -> WebviewWindowBuilder {
        let page = match url {
            WebviewUrl::App(path) => path.to_string_lossy().into_owned(),
            WebviewUrl::External(url) => url.to_string(),
        };
        WebviewWindowBuilder {
            app: manager.app_handle().clone(),
            label: label.into(),
            page,
            options: WindowOptions {
                resizable: true,
                width: 800.0,
                height: 600.0,
                ..WindowOptions::default()
            },
        }
    }

    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.options.title = title.into();
        self
    }

    pub fn inner_size(mut self, width: f64, height: f64) -> Self {
        self.options.width = width;
        self.options.height = height;
        self
    }

    pub fn min_inner_size(self, _: f64, _: f64) -> Self {
        self
    }

    pub fn resizable(mut self, resizable: bool) -> Self {
        self.options.resizable = resizable;
        self
    }

    pub fn always_on_top(mut self, on_top: bool) -> Self {
        self.options.always_on_top = on_top;
        self
    }

    pub fn build(self) -> Result<WebviewWindow> {
        self.app
            .host()
            .open_window(&self.label, &self.page, &self.options)?;
        Ok(WebviewWindow {
            app: self.app,
            label: self.label,
        })
    }
}

pub const APP_ID: &str = "io.github.novakr12.SoundscapeDesktop";

pub fn open_url(url: &str) -> std::result::Result<(), String> {
    Command::new("xdg-open")
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|error| error.to_string())
}
