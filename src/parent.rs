use crate::ipc::{self, FromWindow, Start, ToWindow};
use soundscape_core::host::{AppHandle, Host, Manager, Reply, Result, Running, WindowOptions};
use soundscape_core::mini::MINI;
use soundscape_core::{AppState, MAIN, SETUP};
use std::collections::HashMap;
use std::io::BufReader;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use url::Url;

const STOP_WAIT: Duration = Duration::from_secs(1);

struct Window {
    input: ChildStdin,
    process: Child,
    generation: u64,
}

#[derive(Default)]
struct Inner {
    window: Option<Window>,
    generation: u64,
    waiting: HashMap<u64, Reply>,
    asked: u64,
    url: Option<Url>,
    next_url: Option<Url>,
    hidden: bool,
    respawn: bool,
    done: bool,
}

#[derive(Clone)]
pub struct WindowHost {
    inner: Arc<Mutex<Inner>>,
    open_lock: Arc<Mutex<()>>,
    app: Arc<OnceLock<AppHandle>>,
    stop: Sender<()>,
    running: Arc<Mutex<Running>>,
}

impl WindowHost {
    pub fn new(app: Arc<OnceLock<AppHandle>>, stop: Sender<()>, running: Running) -> WindowHost {
        WindowHost {
            inner: Arc::new(Mutex::new(Inner {
                hidden: true,
                ..Inner::default()
            })),
            open_lock: Arc::new(Mutex::new(())),
            app,
            stop,
            running: Arc::new(Mutex::new(running)),
        }
    }

    fn app(&self) -> Option<&AppHandle> {
        self.app.get()
    }

    fn tell(&self, message: &ToWindow) -> bool {
        let mut inner = self.inner.lock().expect("window lock");
        match inner.window.as_mut() {
            Some(window) => ipc::send(&mut window.input, message),
            None => false,
        }
    }

    fn running_window(&self) -> bool {
        self.inner.lock().expect("window lock").window.is_some()
    }

    fn server_text(&self) -> Option<String> {
        self.app()
            .and_then(|app| app.state::<AppState>().server())
            .or_else(crate::profile::given)
            .map(String::from)
    }

    fn native_script(&self) -> Option<String> {
        self.app()
            .and_then(soundscape_core::native_link::script_for)
    }

    fn open(&self, url: Option<Url>) {
        let _opening = self.open_lock.lock().expect("open lock");
        if self.running_window() {
            let next = url
                .clone()
                .or_else(|| self.inner.lock().expect("window lock").next_url.take());
            if let Some(target) = &next {
                self.tell(&ToWindow::Server {
                    url: self.server_text(),
                });
                self.tell(&ToWindow::Native {
                    script: self.native_script(),
                });
                self.tell(&ToWindow::Navigate {
                    url: target.to_string(),
                });
            }
            if self.tell(&ToWindow::Show) {
                return;
            }
            if let Some(target) = next {
                self.inner.lock().expect("window lock").next_url = Some(target);
            }
        }
        let Some(app) = self.app().cloned() else {
            return;
        };
        let config = app.state::<AppState>().config();
        let url = url
            .or_else(|| self.inner.lock().expect("window lock").next_url.take())
            .or_else(crate::profile::given)
            .unwrap_or_else(|| soundscape_core::start_page(&app));
        let start = Start {
            url: url.to_string(),
            server: self.server_text(),
            native: soundscape_core::native_link::script_for(&app),
            size: config.window_size,
            maximized: config.maximized,
            smooth: config.smooth_scrolling,
            memory_limit: soundscape_core::allowed_memory_limit(
                config
                    .memory_limit
                    .unwrap_or(soundscape_core::MEMORY_LIMIT_MB),
            ),
            keep_window: soundscape_core::allowed_keep_window(
                config
                    .keep_window
                    .unwrap_or(soundscape_core::KEEP_WINDOW_SECONDS),
            ),
        };
        let Ok(me) = std::env::current_exe() else {
            return;
        };
        let spawned = Command::new(me)
            .arg(ipc::WINDOW_FLAG)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn();
        let mut process = match spawned {
            Ok(process) => process,
            Err(error) => {
                eprintln!("The window couldn't open: {error}");
                return;
            }
        };
        let (Some(mut input), Some(output)) = (process.stdin.take(), process.stdout.take()) else {
            let _ = process.kill();
            return;
        };
        if !ipc::send(&mut input, &ToWindow::Start(start)) {
            let _ = process.kill();
            return;
        }
        *self.running.lock().expect("running lock") = Running {
            cpu_rendering: config.cpu_rendering,
            faster_scripts: config.faster_scripts,
        };
        let generation = {
            let mut inner = self.inner.lock().expect("window lock");
            inner.generation += 1;
            inner.window = Some(Window {
                input,
                process,
                generation: inner.generation,
            });
            inner.url = Some(url);
            inner.hidden = false;
            inner.respawn = false;
            inner.generation
        };
        let inner = self.inner.clone();
        let running = self.running.clone();
        let host_app = app.clone();
        std::thread::spawn(move || listen(host_app, inner, running, output, generation));
    }

    fn open_mini_window(&self) -> bool {
        self.app().is_some_and(crate::mini_window::open)
    }

    fn open_mini(&self) {
        self.open_mini_window();
    }

    pub fn stop_window(&self) {
        let sent = {
            let mut inner = self.inner.lock().expect("window lock");
            inner.respawn = false;
            inner.done = false;
            match inner.window.as_mut() {
                Some(window) => ipc::send(&mut window.input, &ToWindow::Exit),
                None => return,
            }
        };
        let began = Instant::now();
        while sent && began.elapsed() < STOP_WAIT {
            {
                let mut inner = self.inner.lock().expect("window lock");
                let exited = inner
                    .window
                    .as_mut()
                    .is_none_or(|window| matches!(window.process.try_wait(), Ok(Some(_))));
                if inner.done || exited {
                    return;
                }
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        if let Some(window) = self.inner.lock().expect("window lock").window.as_mut() {
            let _ = window.process.kill();
        }
    }
}

fn listen(
    app: AppHandle,
    inner: Arc<Mutex<Inner>>,
    running: Arc<Mutex<Running>>,
    output: std::process::ChildStdout,
    generation: u64,
) {
    let mut reader = BufReader::new(output);
    while let Some(message) = ipc::read::<FromWindow>(&mut reader) {
        if let Some(message) = message {
            heard(&app, &inner, &running, message);
        }
    }
    let (respawn, waiting, process) = {
        let mut guard = inner.lock().expect("window lock");
        let ours = guard
            .window
            .as_ref()
            .is_some_and(|window| window.generation == generation);
        if !ours {
            return;
        }
        let window = guard.window.take();
        guard.hidden = true;
        let respawn = std::mem::take(&mut guard.respawn);
        let waiting: Vec<Reply> = guard.waiting.drain().map(|(_, reply)| reply).collect();
        (respawn, waiting, window.map(|window| window.process))
    };
    if let Some(mut process) = process {
        let _ = process.wait();
    }
    for reply in waiting {
        reply(None);
    }
    soundscape_core::tray::refresh(&app);
    if respawn {
        app.host().show(MAIN);
    }
}

fn heard(
    app: &AppHandle,
    inner: &Arc<Mutex<Inner>>,
    running: &Arc<Mutex<Running>>,
    message: FromWindow,
) {
    match message {
        FromWindow::Answer { id, text } => {
            let reply = inner.lock().expect("window lock").waiting.remove(&id);
            if let Some(reply) = reply {
                reply(text);
            }
        }
        FromWindow::Url { url } => {
            if let Ok(url) = Url::parse(&url) {
                inner.lock().expect("window lock").url = Some(url);
            }
        }
        FromWindow::Hidden { hidden } => inner.lock().expect("window lock").hidden = hidden,
        FromWindow::Page { message } => {
            soundscape_core::native_link::heard(app, &message);
        }
        FromWindow::Follow { url } => {
            if let Ok(url) = Url::parse(&url) {
                soundscape_core::follow_from_window(app, &url);
            }
        }
        FromWindow::Close => {
            inner.lock().expect("window lock").hidden = true;
            if !soundscape_core::window_closed(app) {
                app.exit(0);
            }
        }
        FromWindow::Shortcut { name } => match name.as_str() {
            "mini" => {
                let _ = soundscape_core::mini::open(app);
            }
            "settings" => soundscape_core::settings_page::open(app),
            _ => {}
        },
        FromWindow::Poke => soundscape_core::media::poke(),
        FromWindow::WindowState {
            width,
            height,
            maximized,
        } => {
            app.state::<AppState>().update(|config| {
                config.window_size = Some((width, height));
                config.maximized = maximized;
            });
        }
        FromWindow::Done => inner.lock().expect("window lock").done = true,
        FromWindow::CpuRendering { on } => {
            running.lock().expect("running lock").cpu_rendering = on;
            app.state::<AppState>()
                .update(|config| config.cpu_rendering = on);
        }
    }
}

impl Host for WindowHost {
    fn exists(&self, label: &str) -> bool {
        match label {
            MAIN => true,
            MINI => crate::mini_window::is_open(),
            _ => false,
        }
    }

    fn eval(&self, label: &str, script: String) {
        if label == MAIN {
            self.tell(&ToWindow::Eval { script });
        }
    }

    fn evaluate(&self, label: &str, script: String, reply: Reply) {
        if label != MAIN {
            reply(None);
            return;
        }
        let mut inner = self.inner.lock().expect("window lock");
        if inner.window.is_none() {
            drop(inner);
            reply(None);
            return;
        }
        inner.asked += 1;
        let id = inner.asked;
        inner.waiting.insert(id, reply);
        let sent = inner
            .window
            .as_mut()
            .is_some_and(|window| ipc::send(&mut window.input, &ToWindow::Evaluate { id, script }));
        if !sent && let Some(reply) = inner.waiting.remove(&id) {
            drop(inner);
            reply(None);
        }
    }

    fn show(&self, label: &str) {
        match label {
            MAIN => {
                if !self.tell(&ToWindow::Show) {
                    self.open(None);
                }
            }
            MINI => self.open_mini(),
            _ => {}
        }
    }

    fn hide(&self, label: &str) {
        match label {
            MAIN => {
                self.tell(&ToWindow::Hide);
            }
            MINI => crate::mini_window::close(),
            _ => {}
        }
    }

    fn focus(&self, label: &str) {
        match label {
            MAIN => {
                if !self.tell(&ToWindow::Focus) {
                    self.open(None);
                }
            }
            MINI => self.open_mini(),
            _ => {}
        }
    }

    fn close(&self, label: &str) {
        self.hide(label);
    }

    fn is_visible(&self, label: &str) -> bool {
        match label {
            MAIN => {
                let inner = self.inner.lock().expect("window lock");
                inner.window.is_some() && !inner.hidden
            }
            MINI => crate::mini_window::is_open(),
            _ => false,
        }
    }

    fn url(&self, label: &str) -> Option<Url> {
        if label != MAIN {
            return None;
        }
        self.inner.lock().expect("window lock").url.clone()
    }

    fn navigate(&self, label: &str, url: Url) {
        if label != MAIN {
            return;
        }
        if self.running_window() {
            self.tell(&ToWindow::Server {
                url: self.server_text(),
            });
            self.tell(&ToWindow::Native {
                script: self.native_script(),
            });
            self.tell(&ToWindow::Navigate {
                url: url.to_string(),
            });
        } else {
            self.inner.lock().expect("window lock").next_url = Some(url);
        }
    }

    fn open_window(&self, label: &str, _page: &str, _options: &WindowOptions) -> Result<()> {
        match label {
            SETUP => {
                self.tell(&ToWindow::Native {
                    script: self.native_script(),
                });
                if !self.tell(&ToWindow::Setup) {
                    let page = soundscape_core::setup::page("", None);
                    self.open(Some(page));
                }
            }
            MINI => {
                if !self.open_mini_window() {
                    return Err(soundscape_core::host::Error(
                        "The mini player couldn't open".into(),
                    ));
                }
            }
            _ => {
                return Err(soundscape_core::host::Error(format!(
                    "There is no {label} window"
                )));
            }
        }
        Ok(())
    }

    fn run_on_main(&self, work: Box<dyn FnOnce() + Send>) {
        work();
    }

    fn smooth_scrolling(&self, on: bool) {
        self.tell(&ToWindow::Smooth { on });
    }

    fn memory_limit(&self, mb: u32) {
        self.tell(&ToWindow::Memory { mb });
    }

    fn keep_window(&self, seconds: i64) {
        self.tell(&ToWindow::KeepWindow { seconds });
    }

    fn running(&self) -> Running {
        *self.running.lock().expect("running lock")
    }

    fn restart(&self) {
        let mut inner = self.inner.lock().expect("window lock");
        let Some(window) = inner.window.as_mut() else {
            return;
        };
        if ipc::send(&mut window.input, &ToWindow::Exit) {
            inner.respawn = true;
        }
    }

    fn exit(&self, _code: i32) {
        let _ = self.stop.send(());
    }

    fn mini_view(&self, view: &soundscape_core::mini::MiniView) {
        crate::mini_window::show(view);
    }

    fn mini_problem(&self, problem: &str) {
        crate::mini_window::problem(problem);
    }
}
