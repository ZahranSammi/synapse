mod app;
mod backend;
mod theme;
mod widgets;

use std::sync::Arc;

use gpui::{px, size, App, AppContext, Application, Bounds, TitlebarOptions, WindowBounds, WindowOptions};

use crate::app::SynapseApp;

fn main() {
    let tokio_rt = Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("failed to initialize Tokio runtime"),
    );

    Application::new().run(move |cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(1120.0), px(740.0)), cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(TitlebarOptions {
                title: Some("Synapse — Runtime Manager".into()),
                ..Default::default()
            }),
            ..Default::default()
        };

        let rt = Arc::clone(&tokio_rt);
        cx.open_window(options, move |_window, cx| cx.new(|cx| SynapseApp::new(rt, cx)))
            .expect("failed to open Synapse window");
        cx.activate(true);
    });
}
