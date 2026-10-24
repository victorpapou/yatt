//! Development aid: `YATT_DEMO=<dir>` plays a short scripted session (stacks, hover
//! fan, dragging one card, shift-dragging a whole stack, the task modal, timer-only
//! mode) and saves window captures into `<dir>`, so the UI can be checked without
//! clicking through it by hand.

use std::rc::Rc;
use std::time::Duration;

use gpui::{App, AsyncApp, Entity};
use yatt_core::TaskId;
use yatt_macos::{DemoPointer, MainWindow, Sink, UiEvent};

use crate::workspace::Workspace;

pub fn start(cx: &mut App, dir: String, ws: Entity<Workspace>, window: Rc<MainWindow>, sink: Sink) {
    cx.spawn(async move |cx: &mut AsyncApp| {
        let executor = cx.background_executor().clone();
        let wait = |ms| executor.timer(Duration::from_millis(ms));
        let shot = |name: &str| {
            let path = format!("{dir}/{name}.png");
            eprintln!("demo: capture {path}: {}", window.capture_png(&path));
        };
        let ids = |cx: &mut AsyncApp, col: usize| -> Vec<TaskId> {
            ws.read_with(cx, |ws, _| {
                ws.snapshot().columns[col]
                    .cards
                    .iter()
                    .map(|c| c.id)
                    .collect()
            })
            .unwrap_or_default()
        };
        // Moves the pointer from `from` to `to` over `steps` frames, like a hand would.
        let drag = |id: TaskId, from: (f64, f64), to: (f64, f64), steps: u32| {
            let window = window.clone();
            let executor = executor.clone();
            async move {
                for i in 1..=steps {
                    let t = i as f64 / steps as f64;
                    let e = t * t * (3.0 - 2.0 * t);
                    let p = (from.0 + (to.0 - from.0) * e, from.1 + (to.1 - from.1) * e);
                    window.demo_pointer(DemoPointer::Drag(id, p));
                    executor.timer(Duration::from_millis(16)).await;
                }
            }
        };

        wait(600).await;
        for title in [
            "Заметки к задаче",
            "Перетаскивание карточек",
            "Таймер в шапке",
            "Сверстать доску",
            "Стекло и физика",
        ] {
            sink(UiEvent::NewTask(title.into()));
        }
        wait(900).await;
        shot("1-stack");

        // Hover the backlog stack: it fans out.
        let (ax, ay) = window.demo_anchor(0);
        let over = (ax + 90.0, ay + 40.0);
        window.demo_pointer(DemoPointer::Hover(Some(over)));
        wait(500).await;
        shot("2-fan");

        // Pull the top card out alone and carry it to "В работе".
        let (bx, by) = window.demo_anchor(1);
        if let Some(&top) = ids(cx, 0).first() {
            window.demo_pointer(DemoPointer::Down {
                id: top,
                at: over,
                shift: false,
            });
            let mid = (bx + 40.0, by + 160.0);
            drag(top, over, mid, 14).await;
            shot("3-single-lifted");
            let end = (bx + 110.0, by + 50.0);
            drag(top, mid, end, 10).await;
            window.demo_pointer(DemoPointer::Up(top, end));
            wait(700).await;
        }

        // Shift-drag the rest of the backlog to "Готово": the stack trails as a train.
        window.demo_pointer(DemoPointer::Hover(None));
        wait(400).await;
        let (dx, dy) = window.demo_anchor(2);
        if let Some(&top) = ids(cx, 0).first() {
            let start = (ax + 90.0, ay + 40.0);
            window.demo_pointer(DemoPointer::Hover(Some(start)));
            wait(350).await;
            window.demo_pointer(DemoPointer::Down {
                id: top,
                at: start,
                shift: true,
            });
            let mid = (dx - 120.0, dy + 120.0);
            drag(top, start, mid, 9).await;
            shot("4-train");
            let end = (dx + 100.0, dy + 50.0);
            drag(top, mid, end, 8).await;
            window.demo_pointer(DemoPointer::Up(top, end));
            wait(700).await;
            window.demo_pointer(DemoPointer::Hover(None));
            wait(500).await;
            shot("5-settled");
        }

        // The task modal and timer-only mode.
        sink(UiEvent::StartPause);
        if let Some(&id) = ids(cx, 1).first() {
            sink(UiEvent::SetNotes(
                id,
                "## План\n\n- стекло\n- **физика**".into(),
            ));
            sink(UiEvent::ToggleExpand(id));
            wait(900).await;
            shot("6-modal");
            window.debug_print_editor();
            sink(UiEvent::ToggleExpand(id));
            wait(700).await;
        }
        window.toggle_minimal();
        wait(800).await;
        shot("7-minimal");
        window.toggle_minimal();
        wait(800).await;
        eprintln!("demo: finished");
    })
    .detach();
}
