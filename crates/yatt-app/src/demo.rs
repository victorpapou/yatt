//! Development aid: `YATT_DEMO=<dir>` plays a short scripted session (create,
//! move, expand, start the timer, finish a task) and saves window captures into
//! `<dir>`, so the UI can be checked without clicking through it by hand.

use std::rc::Rc;
use std::time::Duration;

use gpui::{App, AsyncApp, Entity};
use yatt_core::Status;
use yatt_macos::{MainWindow, Sink, UiEvent};

use crate::workspace::Workspace;

pub fn start(cx: &mut App, dir: String, ws: Entity<Workspace>, window: Rc<MainWindow>, sink: Sink) {
    cx.spawn(async move |cx: &mut AsyncApp| {
        let executor = cx.background_executor().clone();
        let wait = |ms| executor.timer(Duration::from_millis(ms));
        let shot = |name: &str| {
            let path = format!("{dir}/{name}.png");
            eprintln!("demo: capture {path}: {}", window.capture_png(&path));
        };
        wait(600).await;
        for title in [
            "Сверстать доску",
            "Таймер в шапке",
            "Перетаскивание карточек",
            "Заметки к задаче",
        ] {
            sink(UiEvent::NewTask(title.into()));
        }
        wait(500).await;
        shot("1-backlog");
        let ids = |cx: &mut AsyncApp, col: usize| {
            ws.read_with(cx, |ws, _| {
                ws.snapshot().columns[col]
                    .cards
                    .iter()
                    .map(|c| c.id)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
        };
        let backlog = ids(cx, 0);
        for id in backlog.iter().take(2) {
            sink(UiEvent::Move {
                id: *id,
                status: Status::InProgress,
                index: 0,
            });
        }
        sink(UiEvent::StartPause);
        wait(2600).await;
        shot("2-running");
        if let Some(id) = ids(cx, 1).first().copied() {
            sink(UiEvent::SetNotes(
                id,
                "## План\n\n- колонки\n- **модалка**".into(),
            ));
            sink(UiEvent::ToggleExpand(id));
            wait(150).await;
            shot("3a-modal-morphing");
            wait(700).await;
            shot("3b-modal");
            window.debug_print_editor();
            sink(UiEvent::ToggleExpand(id));
            wait(700).await;
            sink(UiEvent::Move {
                id,
                status: Status::Done,
                index: 0,
            });
            wait(700).await;
            shot("4-done");
        }
        window.toggle_minimal();
        wait(800).await;
        shot("5-minimal");
        window.toggle_minimal();
        wait(800).await;
        shot("6-board-again");
        eprintln!("demo: finished");
    })
    .detach();
}
