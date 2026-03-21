#![windows_subsystem = "windows"]

#[allow(dead_code)]
mod app;
#[allow(dead_code)]
mod config;
#[allow(dead_code)]
mod error;
#[allow(dead_code)]
mod keys;
#[allow(dead_code)]
mod models;
#[allow(dead_code)]
mod ssh;
#[allow(dead_code)]
mod storage;
mod ui;

use std::sync::OnceLock;

use app::SharedState;
use slint::ComponentHandle;
use ui::window::MainWindow;

static TOKIO_RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();

pub fn runtime() -> &'static tokio::runtime::Runtime {
    TOKIO_RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("Failed to create Tokio runtime")
    })
}

fn install_panic_hook() {
    std::panic::set_hook(Box::new(|panic_info| {
        let backtrace = std::backtrace::Backtrace::force_capture();
        let message = format!("panic: {panic_info}\n\nbacktrace:\n{backtrace}\n");

        eprintln!("{message}");

        if let Some(path) = panic_log_path() {
            let _ = std::fs::write(path, message);
        }
    }));
}

fn panic_log_path() -> Option<std::path::PathBuf> {
    let path = config::config_dir().join("panic.log");
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    Some(path)
}

fn main() {
    env_logger::init();
    std::env::set_var("SLINT_STYLE", "fluent-dark");
    install_panic_hook();

    if let Err(e) = config::ensure_directories() {
        eprintln!("Failed to create application directories: {e}");
        std::process::exit(1);
    }

    // Initialize the Tokio runtime eagerly
    let _ = runtime();

    let main_window = MainWindow::new().unwrap();
    let state = SharedState::new();

    ui::window::setup(&main_window, state);

    main_window.run().unwrap();
}
