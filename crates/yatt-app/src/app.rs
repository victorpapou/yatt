//! The macOS app: GPUI application, AppKit window, menus.

use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use futures::StreamExt;
use futures::channel::mpsc;
use gpui::{App, AppContext, Application, KeyBinding, Menu, MenuItem, OsAction, actions};
use yatt_engine::Engine;
use yatt_macos::{MainThreadMarker, MainWindow, UiEvent};
use yatt_store::Store;

use crate::demo;
use crate::workspace::Workspace;

actions!(
    yatt,
    [
        Quit, NewTask, Undo, Redo, StartPause, Stopwatch, Stop, Skip, Cut, Copy, Paste, SelectAll
    ]
);

fn data_path() -> PathBuf {
    if let Some(p) = std::env::var_os("YATT_DB") {
        return PathBuf::from(p);
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    home.join("Library/Application Support/yatt/yatt.sqlite")
}

pub fn run() -> anyhow::Result<()> {
    let store = Store::open(&data_path())?;
    let engine = Engine::load(store)?;

    Application::new().run(move |cx: &mut App| {
        let mtm = MainThreadMarker::new().expect("GPUI runs the app on the main thread");
        let workspace = cx.new(|_| Workspace::new(engine));

        let (tx, mut rx) = mpsc::unbounded::<UiEvent>();
        let sink: yatt_macos::Sink = Rc::new(move |event| {
            let _ = tx.unbounded_send(event);
        });
        let window = Rc::new(MainWindow::new(mtm, sink.clone()));
        window.render(&workspace.read(cx).snapshot());

        // UI events -> engine.
        let ws = workspace.clone();
        cx.spawn(async move |cx| {
            while let Some(event) = rx.next().await {
                let keep_going = ws
                    .update(cx, |ws, cx| {
                        let keep = ws.handle(event);
                        cx.notify();
                        keep
                    })
                    .unwrap_or(false);
                if !keep_going {
                    let _ = cx.update(|cx| cx.quit());
                    break;
                }
            }
        })
        .detach();

        // State -> window.
        {
            let window = window.clone();
            cx.observe(&workspace, move |ws, cx| {
                window.render(&ws.read(cx).snapshot())
            })
            .detach();
        }

        // The clock: the timer is computed from timestamps, so the interval only sets smoothness.
        let ws = workspace.clone();
        cx.spawn(async move |cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(200))
                    .await;
                let alive = ws.update(cx, |ws, cx| {
                    if ws.tick() {
                        cx.notify();
                    }
                });
                if alive.is_err() {
                    break;
                }
            }
        })
        .detach();

        install_menus(cx, &window, &sink);
        if let Ok(dir) = std::env::var("YATT_DEMO") {
            demo::start(cx, dir, workspace.clone(), window.clone(), sink.clone());
        }
        // The observer and the menu actions hold the window for the rest of the run.
        cx.activate(true);
    });
    Ok(())
}

fn install_menus(cx: &mut App, window: &Rc<MainWindow>, sink: &yatt_macos::Sink) {
    // Single-key shortcuts (space) live in the window, so they never fire while typing.
    cx.bind_keys([
        KeyBinding::new("cmd-q", Quit, None),
        KeyBinding::new("cmd-n", NewTask, None),
        KeyBinding::new("cmd-z", Undo, None),
        KeyBinding::new("cmd-shift-z", Redo, None),
        KeyBinding::new("cmd-.", Stop, None),
        KeyBinding::new("cmd-]", Skip, None),
        KeyBinding::new("cmd-x", Cut, None),
        KeyBinding::new("cmd-c", Copy, None),
        KeyBinding::new("cmd-v", Paste, None),
        KeyBinding::new("cmd-a", SelectAll, None),
    ]);

    cx.on_action(|_: &Quit, cx| cx.quit());
    {
        let window = window.clone();
        cx.on_action(move |_: &NewTask, _| window.focus_new_task());
    }
    let send = |event: UiEvent| {
        let sink = sink.clone();
        move |_: &dyn std::any::Any| sink(event.clone())
    };
    {
        let (window, sink) = (window.clone(), sink.clone());
        cx.on_action(move |_: &Undo, _| {
            if !window.text_undo(false) {
                sink(UiEvent::Undo);
            }
        });
    }
    {
        let (window, sink) = (window.clone(), sink.clone());
        cx.on_action(move |_: &Redo, _| {
            if !window.text_undo(true) {
                sink(UiEvent::Redo);
            }
        });
    }
    let f = send(UiEvent::StartPause);
    cx.on_action(move |a: &StartPause, _| f(a));
    let f = send(UiEvent::StartStopwatch);
    cx.on_action(move |a: &Stopwatch, _| f(a));
    let f = send(UiEvent::Stop);
    cx.on_action(move |a: &Stop, _| f(a));
    let f = send(UiEvent::Skip);
    cx.on_action(move |a: &Skip, _| f(a));

    cx.set_menus(vec![
        Menu {
            name: "yatt".into(),
            items: vec![MenuItem::action("Выйти из yatt", Quit)],
        },
        Menu {
            name: "Файл".into(),
            items: vec![MenuItem::action("Новая задача", NewTask)],
        },
        Menu {
            name: "Правка".into(),
            items: vec![
                MenuItem::os_action("Отменить", Undo, OsAction::Undo),
                MenuItem::os_action("Повторить", Redo, OsAction::Redo),
                MenuItem::separator(),
                MenuItem::os_action("Вырезать", Cut, OsAction::Cut),
                MenuItem::os_action("Скопировать", Copy, OsAction::Copy),
                MenuItem::os_action("Вставить", Paste, OsAction::Paste),
                MenuItem::os_action("Выбрать все", SelectAll, OsAction::SelectAll),
            ],
        },
        Menu {
            name: "Таймер".into(),
            items: vec![
                MenuItem::action("Старт / пауза (пробел)", StartPause),
                MenuItem::action("Секундомер", Stopwatch),
                MenuItem::action("Пропустить фазу", Skip),
                MenuItem::action("Стоп", Stop),
            ],
        },
    ]);
}
