//! Development aid: `YATT_DEMO=<dir>` plays a short scripted session (new tasks
//! flying in from the capsule, a plain drag, a ⇧/⌘ selection dragged as a train,
//! a two-finger swipe, a column folded into a stack and fanned, the task modal,
//! timer-only mode) and saves window captures into `<dir>`, so the UI can be
//! checked without clicking through it by hand.

use std::rc::Rc;
use std::time::Duration;

use gpui::{App, AsyncApp, Entity};
use yatt_core::{Status, TaskId};
use yatt_macos::{DemoPhase, DemoPointer, MainWindow, Sink, UiEvent};

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
        let centre = |id: TaskId| window.demo_center(id).unwrap_or((0.0, 0.0));
        let ease = |from: (f64, f64), to: (f64, f64), t: f64| {
            let e = t * t * (3.0 - 2.0 * t);
            (from.0 + (to.0 - from.0) * e, from.1 + (to.1 - from.1) * e)
        };
        // Moves the pointer from `from` to `to` over `steps` frames, like a hand would.
        let drag = |from: (f64, f64), to: (f64, f64), steps: u32| {
            let window = window.clone();
            let executor = executor.clone();
            async move {
                for i in 1..=steps {
                    let p = ease(from, to, i as f64 / steps as f64);
                    window.demo_pointer(DemoPointer::Drag(p));
                    executor.timer(Duration::from_millis(16)).await;
                }
            }
        };
        let click = |at: (f64, f64), shift: bool, cmd: bool| {
            window.demo_pointer(DemoPointer::Down { at, shift, cmd });
            window.demo_pointer(DemoPointer::Up(at));
        };

        wait(600).await;
        for title in [
            "Заметки к задаче",
            "Перетаскивание карточек",
            "Таймер в шапке",
            "Сверстать доску",
        ] {
            sink(UiEvent::NewTask {
                title: title.into(),
                status: Status::Backlog,
            });
        }
        wait(400).await;
        window.demo_new_task("Стекло и физика", false);
        wait(140).await;
        shot("1-fly");
        wait(700).await;
        window.demo_new_task("Срочное", true);
        wait(900).await;
        shot("2-list");

        // A plain drag: the top backlog card into "В работе", under "Срочное".
        let bx = window.demo_column_x(1);
        if let Some(&top) = ids(cx, 0).first() {
            let from = centre(top);
            window.demo_pointer(DemoPointer::Down {
                at: from,
                shift: false,
                cmd: false,
            });
            let mid = (bx - 60.0, 150.0);
            drag(from, mid, 14).await;
            shot("3-single-drag");
            let end = (bx, 130.0);
            drag(mid, end, 8).await;
            window.demo_pointer(DemoPointer::Up(end));
            wait(600).await;
        }

        // ⌘-click the first backlog card, ⇧-click the third: three selected.
        let backlog = ids(cx, 0);
        if backlog.len() >= 3 {
            click(centre(backlog[0]), false, true);
            click(centre(backlog[2]), true, false);
            wait(300).await;
            shot("4-selection");
            // Drag the selection to "Готово": the others trail as a train.
            let from = centre(backlog[1]);
            let dx = window.demo_column_x(2);
            window.demo_pointer(DemoPointer::Down {
                at: from,
                shift: false,
                cmd: false,
            });
            let mid = (dx - 140.0, 200.0);
            drag(from, mid, 9).await;
            shot("5-train");
            let end = (dx, 70.0);
            drag(mid, end, 8).await;
            window.demo_pointer(DemoPointer::Up(end));
            wait(800).await;
            shot("6-dropped");
        }

        // A click on empty board drops the selection; then a two-finger swipe carries
        // one done card back to "В работе".
        click((window.demo_column_x(0), 600.0), false, false);
        if let Some(&card) = ids(cx, 2).last() {
            let at = centre(card);
            let swipe = |finger: (f64, f64), phase: DemoPhase| {
                window.demo_pointer(DemoPointer::Swipe { at, finger, phase });
            };
            swipe((0.0, 0.0), DemoPhase::Began);
            let step = ((bx - at.0) / 16.0, 3.0);
            for _ in 0..16 {
                swipe(step, DemoPhase::Changed);
                wait(16).await;
            }
            shot("7-swipe");
            swipe((0.0, 0.0), DemoPhase::Ended);
            wait(700).await;
        }

        // Fold "В работе" into a stack, fan it with a click, then drag it whole to "Готово".
        window.demo_toggle_stack(1);
        wait(600).await;
        shot("8-stack");
        if let Some(&top) = ids(cx, 1).first() {
            let at = centre(top);
            click(at, false, false);
            wait(150).await;
            let first = ids(cx, 1);
            if let Some(&id) = first.get(1) {
                window.demo_pointer(DemoPointer::Hover(Some(centre(id))));
            }
            wait(450).await;
            shot("9-fan");
            window.demo_pointer(DemoPointer::Hover(None));
            click((window.demo_column_x(0), 600.0), false, false);
            wait(500).await;
            let at = centre(top);
            window.demo_pointer(DemoPointer::Down {
                at,
                shift: false,
                cmd: false,
            });
            let dx = window.demo_column_x(2);
            let mid = (dx - 60.0, 330.0);
            drag(at, mid, 10).await;
            shot("10-stack-drag");
            let end = (dx, 90.0);
            drag(mid, end, 8).await;
            window.demo_pointer(DemoPointer::Up(end));
            wait(700).await;
            window.demo_toggle_stack(1);
        }

        // The task modal and timer-only mode.
        sink(UiEvent::StartPause);
        if let Some(&id) = ids(cx, 2).first() {
            sink(UiEvent::SetNotes(
                id,
                "## План\n\n- стекло\n- **физика**".into(),
            ));
            click(centre(id), false, false);
            wait(120).await;
            shot("11-morph");
            wait(800).await;
            shot("12-modal");
            window.debug_print_editor();
            sink(UiEvent::ToggleExpand(id));
            wait(700).await;
        }
        window.toggle_minimal();
        wait(800).await;
        shot("13-minimal");
        window.toggle_minimal();
        wait(800).await;
        eprintln!("demo: finished");
    })
    .detach();
}
