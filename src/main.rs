#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
//! Windows Dynamic Notch: a Dynamic Island style notch for the top edge of
//! the primary monitor, rendered natively with wgpu (DX12) and vello.

mod animation;
mod app;
mod logging;
mod renderer;
mod services;
mod ui;
mod window;

use std::time::Duration;

use winit::event_loop::{ControlFlow, EventLoop};

fn main() {
    logging::init();
    log::info!("Windows Dynamic Notch {} starting", env!("CARGO_PKG_VERSION"));

    let Some(_instance) = window::win32::SingleInstance::acquire() else {
        log::info!("another instance is already running");
        return;
    };

    if let Err(err) = window::win32::init_ole() {
        log::error!("OleInitialize failed: {err}");
        return;
    }

    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .max_blocking_threads(6)
        .thread_name("notch-worker")
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(err) => {
            log::error!("cannot start tokio runtime: {err}");
            return;
        }
    };

    let event_loop = match EventLoop::<app::UserEvent>::with_user_event().build() {
        Ok(event_loop) => event_loop,
        Err(err) => {
            log::error!("cannot create event loop: {err}");
            return;
        }
    };
    event_loop.set_control_flow(ControlFlow::Wait);

    let proxy = event_loop.create_proxy();
    let (bus, inbox) = services::EventBus::new(proxy.clone());
    let services = services::start(runtime.handle(), bus);
    let mut app = app::App::new(runtime.handle().clone(), proxy, inbox, services);

    if let Err(err) = event_loop.run_app(&mut app) {
        log::error!("event loop error: {err}");
    }
    drop(app);
    runtime.shutdown_timeout(Duration::from_millis(800));
    window::win32::uninit_ole();
    log::info!("bye");
}
