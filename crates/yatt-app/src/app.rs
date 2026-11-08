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
        Quit, NewTask, Undo, Redo, StartPause, Stopwatch, Stop, Skip, Minimal, Cut, Copy, Paste,
        SelectAll, Find
    ]
);

fn data_path() -> PathBuf {
    if let Some(p) = std::env::var_os("YATT_DB") {
        return PathBuf::from(p);
    }
    // The demo moves, renames and rewrites cards: never on the real board.
    if let Some(dir) = std::env::var_os("YATT_DEMO") {
        return PathBuf::from(dir).join("demo.sqlite");
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    home.join("Library/Application Support/yatt/yatt.sqlite")
}

/// Takes the single-instance lock next to the database. A second copy writing to
/// the same board would double every task's time, so it must not start. The lock
/// is released by the system when the process ends, however it ends.
fn lock_instance(db: &std::path::Path) -> anyhow::Result<std::fs::File> {
    use std::fs::{File, TryLockError};
    if let Some(dir) = db.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let file = File::options()
        .create(true)
        .truncate(false)
        .write(true)
        .open(db.with_extension("lock"))?;
    match file.try_lock() {
        Ok(()) => Ok(file),
        Err(TryLockError::WouldBlock) => anyhow::bail!(
            "yatt уже запущен. Две копии, пишущие в одну базу, удвоили бы время задач. \
             Найдите открытую копию в Dock или в строке меню."
        ),
        Err(TryLockError::Error(e)) => Err(e.into()),
    }
}

/// Everything that can stop the app before its window exists. A bundled .app has
/// no terminal, so the reason goes into an alert instead of stderr.
fn open_board() -> anyhow::Result<(std::fs::File, Engine)> {
    let path = data_path();
    let lock = lock_instance(&path)?;
    let store = Store::open(&path)?;
    let mut engine = Engine::load(store)?;
    engine.on_phase_finished = Some(Box::new(|phase, next| {
        use yatt_core::Phase;
        use yatt_platform::Notifier;
        let (title, body) = match (phase, next) {
            (Phase::Work, Phase::LongBreak) => ("Помидор готов", "Пора на длинный перерыв"),
            (Phase::Work, _) => ("Помидор готов", "Пора на перерыв"),
            (_, Phase::Work) => ("Перерыв закончился", "Пора работать"),
            _ => ("Фаза закончилась", "Следующая фаза готова"),
        };
        yatt_macos::MacNotifier.notify(title, body);
    }));
    Ok((lock, engine))
}

pub fn run() -> anyhow::Result<()> {
    let (_lock, engine) = match open_board() {
        Ok(opened) => opened,
        Err(e) => {
            if let Some(mtm) = MainThreadMarker::new() {
                yatt_macos::alert(mtm, "yatt не может открыть доску", &format!("{e:#}"));
            }
            return Err(e);
        }
    };

    // The window, once it exists, for the Dock icon to bring back.
    let shown: Rc<std::cell::RefCell<Option<Rc<MainWindow>>>> = Rc::default();
    let app = Application::new();
    {
        let shown = shown.clone();
        app.on_reopen(move |cx| {
            if let Some(window) = shown.borrow().as_ref() {
                window.show();
            }
            cx.activate(true);
        });
    }
    app.run(move |cx: &mut App| {
        let mtm = MainThreadMarker::new().expect("GPUI runs the app on the main thread");
        let workspace = cx.new(|_| Workspace::new(engine));

        let (tx, mut rx) = mpsc::unbounded::<UiEvent>();
        let sink: yatt_macos::Sink = Rc::new(move |event| {
            let _ = tx.unbounded_send(event);
        });
        let window = Rc::new(MainWindow::new(mtm, sink.clone()));
        window.render(&workspace.read(cx).snapshot());
        *shown.borrow_mut() = Some(window.clone());

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
                report_problems(&ws, cx, mtm);
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
                report_problems(&ws, cx, mtm);
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
        // Development aid: `YATT_SHOT=<file.png>` saves the real window (the real
        // database, or the one in `YATT_DB`) after a moment and quits.
        if let Ok(path) = std::env::var("YATT_SHOT") {
            let (window, sink) = (window.clone(), sink.clone());
            cx.spawn(async move |cx: &mut gpui::AsyncApp| {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(3000))
                    .await;
                eprintln!("shot: {path}: {}", window.capture_png(&path));
                sink(yatt_macos::UiEvent::Quit);
            })
            .detach();
        }
        // The observer and the menu actions hold the window for the rest of the run.
        cx.activate(true);
    });
    Ok(())
}

/// Tells the user about a failed save. The board is in memory and looks fine, so
/// without this a full disk would only show after the next launch.
fn report_problems(ws: &gpui::Entity<Workspace>, cx: &mut gpui::AsyncApp, mtm: MainThreadMarker) {
    let problem = ws
        .update(cx, |ws, _| ws.engine.take_problem())
        .ok()
        .flatten();
    if let Some(text) = problem {
        yatt_macos::alert(mtm, "yatt не сохранил изменения", &text);
    }
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
        KeyBinding::new("cmd-shift-m", Minimal, None),
        KeyBinding::new("cmd-x", Cut, None),
        KeyBinding::new("cmd-c", Copy, None),
        KeyBinding::new("cmd-v", Paste, None),
        KeyBinding::new("cmd-a", SelectAll, None),
        KeyBinding::new("cmd-f", Find, None),
    ]);

    {
        // The title and notes being typed are saved first; the quit goes in behind them.
        let (window, sink) = (window.clone(), sink.clone());
        cx.on_action(move |_: &Quit, cx| {
            let sink = sink.clone();
            window.save_edits_then(move || sink(UiEvent::Quit));
            // Should the editor never answer, quit anyway.
            cx.spawn(async |cx| {
                cx.background_executor()
                    .timer(Duration::from_millis(1500))
                    .await;
                let _ = cx.update(|cx| cx.quit());
            })
            .detach();
        });
    }
    {
        let window = window.clone();
        cx.on_action(move |_: &NewTask, _| window.focus_new_task());
    }
    {
        let window = window.clone();
        cx.on_action(move |_: &Find, _| window.focus_search());
    }
    {
        let window = window.clone();
        cx.on_action(move |_: &Minimal, _| window.toggle_minimal());
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

    cx.set_menus(
        vec![
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
                    MenuItem::separator(),
                    MenuItem::action("Найти", Find),
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
        ]
        .into_iter()
        .chain(yatt_macos::model::MINIMAL_MODE.then(|| Menu {
            name: "Вид".into(),
            items: vec![MenuItem::action("Только таймер", Minimal)],
        }))
        .collect(),
    );
}
