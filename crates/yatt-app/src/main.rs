mod board_view;

use std::path::PathBuf;

use gpui::{
    App, AppContext, Application, Bounds, KeyBinding, WindowBounds, WindowOptions, actions, px,
    size,
};
use yatt_engine::Engine;
use yatt_store::Store;

use crate::board_view::{BoardView, NewTask, Redo, Skip, StartPause, Stop, Undo};

actions!(yatt, [Quit]);

fn data_path() -> PathBuf {
    if let Some(p) = std::env::var_os("YATT_DB") {
        return PathBuf::from(p);
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    home.join("Library/Application Support/yatt/yatt.sqlite")
}

fn main() -> anyhow::Result<()> {
    let store = Store::open(&data_path())?;
    let state = Engine::load(store)?;

    Application::new().run(move |cx: &mut App| {
        cx.bind_keys([
            KeyBinding::new("cmd-q", Quit, None),
            KeyBinding::new("cmd-z", Undo, Some("Board")),
            KeyBinding::new("cmd-shift-z", Redo, Some("Board")),
            KeyBinding::new("cmd-n", NewTask, Some("Board")),
            KeyBinding::new("space", StartPause, Some("Board")),
            KeyBinding::new("cmd-.", Stop, Some("Board")),
            KeyBinding::new("cmd-]", Skip, Some("Board")),
        ]);
        cx.on_action(|_: &Quit, cx| cx.quit());

        let bounds = Bounds::centered(None, size(px(1100.), px(720.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |window, cx| {
                let view = cx.new(|cx| BoardView::new(state, cx));
                window.focus(&view.read(cx).focus_handle());
                view
            },
        )
        .expect("open window");
        cx.activate(true);
    });
    Ok(())
}
