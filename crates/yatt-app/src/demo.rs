//! Development aid: `YATT_DEMO=<dir>` plays a short scripted session (new tasks
//! flying in from the capsule, a plain drag, a ⇧/⌘ selection dragged as a train,
//! real mouse drags through the window, a column folded into a stack and fanned,
//! the task modal,
//! timer-only mode) and saves window captures into `<dir>`, so the UI can be
//! checked without clicking through it by hand.

use std::rc::Rc;
use std::time::Duration;

use gpui::{App, AsyncApp, Entity};
use yatt_core::{Status, TaskId};
use yatt_macos::{DemoMouse, DemoPointer, MainWindow, Sink, UiEvent};

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
        if std::env::var_os("YATT_DEMO_COMPARE").is_some() {
            // The reference clip's actions, frame by frame at a fixed 1/60 s:
            // each action 50 frames, a layer capture and a CSV line per frame
            // (the lead card's centre, angle and scale).
            for i in 0..6 {
                sink(UiEvent::NewTask {
                    title: format!("Задача {}", i + 1),
                    status: Status::Backlog,
                });
            }
            wait(800).await;
            window.demo_toggle_stack();
            wait(900).await;
            window.demo_manual();
            let dt = 1.0 / 60.0;
            let backlog = ids(cx, 0);
            let lead = backlog[0];
            let frames = |name: &'static str, lead: TaskId, script: &dyn Fn(usize)| {
                for f in 0..50 {
                    script(f);
                    window.demo_step(dt);
                    window.capture_layers_png(&format!("{dir}/{name}-{f:02}.png"));
                    if f == 0 || f == 49 {
                        let drawn: Vec<usize> = (0..3).map(|c| window.demo_column_opacity(c).1).collect();
                        eprintln!("drawn,{name},{f},{drawn:?}");
                    }
                    if let Some((x, y, r, s)) = window.demo_card_state(lead) {
                        eprintln!("csv,{name},{f},{:.4},{x:.1},{y:.1},{r:.2},{s:.3}", f as f64 * dt);
                    }
                }
            };
            let top = centre(lead);
            frames("fan-open", lead, &|f| {
                // Fingers down over the stack, 7 pt a frame: the fan follows.
                if f < 20 {
                    window.demo_scroll_at(top, 7.0);
                }
            });
            // A new card dropped into the open fan: the old top card (now the
            // second) and a deeper one, as the fan re-lays itself out.
            sink(UiEvent::NewTask {
                title: "Новая".into(),
                status: Status::Backlog,
            });
            wait(300).await;
            let deep = backlog[3];
            frames("fan-insert", lead, &|_| {
                if let Some((x, y, r, s)) = window.demo_card_state(deep) {
                    eprintln!("deep,{x:.1},{y:.1},{r:.2},{s:.3}");
                }
            });
            frames("fan-fold", lead, &|f| {
                if f == 0 {
                    window.demo_scroll_at(top, -60.0);
                }
            });
            window.demo_toggle_stack();
            for _ in 0..90 {
                window.demo_step(dt);
            }
            // One card: grabbed off-centre (right of the middle, a little high,
            // where the reference's cursor holds it), pulled right over 30
            // frames, then held still.
            let c = centre(lead);
            let from = (c.0 + 70.0, c.1 - 15.0);
            let to = (window.demo_column_x(1), from.1 + 60.0);
            frames("single-drag", lead, &|f| {
                if f == 0 {
                    window.demo_pointer(DemoPointer::Down { at: from, shift: false, cmd: false });
                }
                let t = ((f + 1) as f64 / 30.0).min(1.0);
                window.demo_pointer(DemoPointer::Drag(ease(from, to, t)));
            });
            frames("single-drop", lead, &|f| {
                if f == 0 {
                    window.demo_pointer(DemoPointer::Up(to));
                }
            });
            // A group of four, the same way.
            let group = ids(cx, 0);
            if group.len() >= 4 {
                window.demo_select(&group[..4]);
                let c = centre(group[0]);
                let from = (c.0 + 70.0, c.1 - 15.0);
                let to = (window.demo_column_x(2), from.1 + 40.0);
                frames("group-drag", group[0], &|f| {
                    if f == 0 {
                        window.demo_pointer(DemoPointer::Down { at: from, shift: false, cmd: false });
                    }
                    let t = ((f + 1) as f64 / 30.0).min(1.0);
                    window.demo_pointer(DemoPointer::Drag(ease(from, to, t)));
                });
                frames("group-drop", group[0], &|f| {
                    if f == 0 {
                        window.demo_pointer(DemoPointer::Up(to));
                    }
                });
            }
            eprintln!("demo: finished");
            return;
        }
        if let Some(n) = std::env::var("YATT_DEMO_FAN").ok().and_then(|n| n.parse::<usize>().ok()) {
            // One stack of `n` cards, fanned open, for a look at the fan.
            for i in 0..n {
                sink(UiEvent::NewTask {
                    title: format!("Задача {}", i + 1),
                    status: Status::Backlog,
                });
            }
            wait(600).await;
            window.demo_toggle_stack();
            wait(700).await;
            if let Some(&top) = ids(cx, 0).first() {
                window.demo_scroll_at(centre(top), 60.0);
            }
            for _ in 0..6 {
                wait(40).await;
                let (low, drawn) = window.demo_column_opacity(0);
                eprintln!("demo: fan of {n} opening: {drawn} cards drawn, lowest opacity {low:.2}");
            }
            wait(660).await;
            shot(&format!("fan-{n}"));
            window.capture_layers_png(&format!("{dir}/fan-{n}-layers.png"));
            eprintln!("demo: finished");
            return;
        }
        if std::env::var_os("YATT_DEMO_TOOLBAR").is_some() {
            // Just the window as it opens, for a look at the toolbar.
            for i in 0..4 {
                sink(UiEvent::NewTask {
                    title: format!("Задача {}", i + 1),
                    status: Status::Backlog,
                });
            }
            wait(900).await;
            shot("toolbar");
            eprintln!(
                "demo: layer capture {}",
                window.capture_layers_png(&format!("{dir}/toolbar-layers.png"))
            );
            eprintln!("demo: finished");
            return;
        }
        if let Ok(n) = std::env::var("YATT_DEMO_PERF") {
            // A big board (`YATT_DEMO_PERF=<cards>`, 100 by default): a long
            // scroll timed step by step, then 20 cards dragged slowly across,
            // timed by YATT_PERF.
            let n: usize = n.parse().unwrap_or(100);
            for i in 0..n {
                sink(UiEvent::NewTask {
                    title: format!("Задача {}", i + 1),
                    status: [Status::Backlog, Status::InProgress, Status::Done][i % 3],
                });
                if i % 50 == 49 {
                    wait(1).await;
                }
            }
            wait(2000).await;
            let mut steps = Vec::new();
            for k in 0..240 {
                let y = 30.0 * (k % 120) as f64;
                let t = std::time::Instant::now();
                window.demo_scroll(0, y);
                wait(8).await;
                steps.push(t.elapsed().as_secs_f64() * 1000.0);
            }
            window.demo_scroll(0, 0.0);
            steps.sort_by(f64::total_cmp);
            let mean = steps.iter().sum::<f64>() / steps.len() as f64;
            eprintln!(
                "perf: scroll of a {n}-card board: step mean {mean:.1} ms, p50 {:.1} ms, p95 {:.1} ms, max {:.1} ms (8 ms asked)",
                steps[steps.len() / 2],
                steps[steps.len() * 95 / 100],
                steps[steps.len() - 1]
            );
            wait(500).await;
            let backlog = ids(cx, 0);
            window.demo_select(&backlog[..20.min(backlog.len())]);
            wait(300).await;
            let from = centre(backlog[0]);
            window.demo_pointer(DemoPointer::Down {
                at: from,
                shift: false,
                cmd: false,
            });
            let mid = (window.demo_column_x(2), 300.0);
            drag(from, mid, 90).await;
            drag(mid, (window.demo_column_x(1), 200.0), 90).await;
            window.demo_pointer(DemoPointer::Up((window.demo_column_x(1), 200.0)));
            wait(1200).await;
            // Fold the board into stacks and scroll the biggest one open.
            window.demo_toggle_stack();
            wait(1500).await;
            if let Some(&top) = ids(cx, 0).first() {
                window.demo_scroll_at(centre(top), 60.0);
                wait(1200).await;
                shot("perf-fan");
            }
            eprintln!("demo: finished");
            return;
        }
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
        // Every task gets notes and a picture of its own.
        let all: Vec<TaskId> = (0..3).flat_map(|c| ids(cx, c)).collect();
        for (i, id) in all.iter().enumerate() {
            sink(UiEvent::SetNotes(*id, notes(i)));
        }
        wait(200).await;
        shot("2-list");

        // A real single-card drag, every event sent through the window as the
        // system would: the top backlog card into "Готово".
        if let Some(&top) = ids(cx, 0).first() {
            let from = centre(top);
            let to = (window.demo_column_x(2), 120.0);
            window.demo_real_mouse(DemoMouse::Down, from);
            let mut followed = true;
            for i in 1..=20 {
                let p = ease(from, to, i as f64 / 20.0);
                window.demo_real_mouse(DemoMouse::Drag, p);
                wait(16).await;
                if i % 4 == 0 {
                    window.capture_layers_png(&format!("{dir}/motion-ours-{i:02}.png"));
                }
                // The card's view must be under the pointer all the way, not only
                // land at the end.
                if let Some(v) = window.demo_view_center(top) {
                    let off = (v.0 - p.0).hypot(v.1 - p.1);
                    if i > 2 && off > 30.0 {
                        followed = false;
                    }
                }
            }
            eprintln!(
                "demo: real-event drag of one card: the card {} the pointer",
                if followed { "followed" } else { "DID NOT FOLLOW (wrong)" }
            );
            window.demo_real_mouse(DemoMouse::Up, to);
            wait(700).await;
            let landed = ids(cx, 2).contains(&top);
            eprintln!(
                "demo: real-event drag of one card: {}",
                if landed { "landed in Готово" } else { "DID NOT MOVE (wrong)" }
            );
            shot("2b-real-drag");
        }

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
            window.capture_layers_png(&format!("{dir}/5b-train-layers.png"));
            let end = (dx, 70.0);
            drag(mid, end, 8).await;
            window.demo_pointer(DemoPointer::Up(end));
            // The cards land one after another.
            wait(40).await;
            shot("6a-landing");
            wait(60).await;
            shot("6b-landing");
            wait(700).await;
            shot("6-dropped");
        }

        // A click on empty board drops the selection.
        click((window.demo_column_x(0), 600.0), false, false);

        // Fold the board into stacks, fan "В работе" with a click, then drag it whole to "Готово".
        window.demo_toggle_stack();
        wait(600).await;
        shot("8-stack");
        // A big backlog: its fan wraps onto an inner ring instead of overflowing.
        let start = all.len() + 10;
        let mut k = 0;
        while ids(cx, 0).len() < 11 && k < 12 {
            sink(UiEvent::NewTask {
                title: format!("Задача {}", k + 1),
                status: Status::Backlog,
            });
            k += 1;
        }
        wait(300).await;
        for (i, id) in ids(cx, 0).iter().enumerate() {
            sink(UiEvent::SetNotes(*id, notes(start + i)));
        }
        wait(400).await;
        if let Some(&top) = ids(cx, 0).first() {
            window.demo_scroll_at(centre(top), 60.0);
            wait(700).await;
            shot("8c-big-fan");
            window.demo_scroll_at(centre(top), -60.0);
            wait(350).await;
            eprintln!(
                "demo: after scrolling up over the big fan it is {}",
                if window.demo_is_fanned() {
                    "OPEN (wrong)"
                } else {
                    "folded"
                }
            );
        }
        // A wide window: every stack must stay on its column's centre line.
        window.demo_resize(1800.0, 900.0);
        wait(500).await;
        for col in 0..3 {
            if let Some(&top) = ids(cx, col).first() {
                let (x, _) = centre(top);
                eprintln!(
                    "demo: column {col} centre {:.1}, stack centre {x:.1}",
                    window.demo_column_x(col)
                );
            }
        }
        shot("8b-stack-wide");
        window.demo_resize(1100.0, 720.0);
        wait(500).await;
        // Lists to stacks and a click on a stack straight away: the slivers must
        // never show over the top card, all the way through the fan opening.
        window.demo_toggle_stack();
        wait(900).await;
        window.demo_toggle_stack();
        wait(150).await;
        if let Some(&top) = ids(cx, 1).first() {
            window.demo_scroll_at(centre(top), 60.0);
            let mut at = 0;
            for (ms, name) in [
                (100, "8d-fan-0.1s"),
                (250, "8d-fan-0.25s"),
                (500, "8d-fan-0.5s"),
                (1000, "8d-fan-1s"),
                (2000, "8d-fan-2s"),
            ] {
                wait(ms - at).await;
                at = ms;
                shot(name);
                let (low, drawn) = window.demo_column_opacity(1);
                eprintln!("demo: fan opening at {name}: {drawn} cards drawn, lowest opacity {low:.2}");
            }
            click((window.demo_column_x(0), 640.0), false, false);
            wait(600).await;
        }
        if let Some(&top) = ids(cx, 1).first() {
            let at = centre(top);
            window.demo_scroll_at(at, 60.0);
            wait(150).await;
            let first = ids(cx, 1);
            if let Some(&id) = first.get(1) {
                window.demo_pointer(DemoPointer::Hover(Some(centre(id))));
            }
            wait(450).await;
            shot("9-fan");
            let misfits = window.demo_misfit_parts();
            eprintln!(
                "demo: card parts in an open fan: {}",
                if misfits.is_empty() { "all the card's size".into() } else { misfits.join("; ") }
            );
            window.demo_pointer(DemoPointer::Hover(None));
            // A real drag of one card out of the open fan into "Бэклог".
            if let Some(&one) = ids(cx, 1).get(1) {
                let from = centre(one);
                let to = (window.demo_column_x(0), 200.0);
                window.demo_real_mouse(DemoMouse::Down, from);
                for i in 1..=16 {
                    window.demo_real_mouse(DemoMouse::Drag, ease(from, to, i as f64 / 16.0));
                    wait(16).await;
                    if i == 8 {
                        window.capture_layers_png(&format!("{dir}/9c-out-of-fan.png"));
                        let misfits = window.demo_misfit_parts();
                        eprintln!(
                            "demo: card parts while pulling a card out of a fan: {}",
                            if misfits.is_empty() { "all the card's size".into() } else { misfits.join("; ") }
                        );
                    }
                }
                window.demo_real_mouse(DemoMouse::Up, to);
                wait(700).await;
                eprintln!(
                    "demo: real-event drag of one card out of a fan: {}",
                    if ids(cx, 0).contains(&one) { "landed in Бэклог" } else { "DID NOT MOVE (wrong)" }
                );
            }
            // Scrolling up over it folds the fan.
            window.demo_scroll_at(at, -60.0);
            wait(350).await;
            eprintln!(
                "demo: after scrolling up the fan is {}",
                if window.demo_is_fanned() {
                    "OPEN (wrong)"
                } else {
                    "folded"
                }
            );
            shot("9b-fan-folded");
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
            window.demo_toggle_stack();
            wait(900).await;
        }

        // The task modal and timer-only mode.
        sink(UiEvent::StartPause);
        if let Some(&id) = ids(cx, 2).first() {
            // Three frames on the way open, the panel at rest, three on the way back.
            click(centre(id), false, false);
            for (ms, name) in [(30, "11a-open"), (50, "11b-open"), (70, "11c-open")] {
                wait(ms).await;
                shot(name);
            }
            wait(800).await;
            shot("12-modal");
            window.debug_print_editor();
            sink(UiEvent::ToggleExpand(id));
            for (ms, name) in [(30, "12a-close"), (50, "12b-close"), (70, "12c-close")] {
                wait(ms).await;
                shot(name);
            }
            wait(700).await;
            shot("12d-closed");
        }
        window.toggle_minimal();
        wait(800).await;
        shot("13-minimal");
        window.toggle_minimal();
        wait(800).await;

        // A big group dragged slowly across the board, for YATT_PERF timings.
        let backlog = ids(cx, 0);
        if backlog.len() >= 8 {
            click(centre(backlog[0]), false, true);
            for &id in &backlog[1..8] {
                click(centre(id), false, true);
            }
            wait(300).await;
            let from = centre(backlog[0]);
            window.demo_pointer(DemoPointer::Down {
                at: from,
                shift: false,
                cmd: false,
            });
            let mid = (window.demo_column_x(2), 300.0);
            drag(from, mid, 60).await;
            shot("14-big-group");
            drag(mid, (window.demo_column_x(1), 200.0), 60).await;
            window.demo_pointer(DemoPointer::Up((window.demo_column_x(1), 200.0)));
            wait(900).await;
        }
        eprintln!("demo: finished");
    })
    .detach();
}

/// The `i`-th demo task's notes: a few lines of its own and a picture.
fn notes(i: usize) -> String {
    const TEXTS: [&str; 12] = [
        "## План\n\n- стекло\n- **физика**",
        "Пружины: `k` и `ζ`, пару значений на пробу.",
        "- [ ] шапка\n- [ ] таймер\n- [ ] прогресс",
        "> Бесшовность: тот же элемент, без подмен.",
        "Сетка колонок, отступы 16 pt.",
        "Проверить на трекпаде **двумя пальцами**.",
        "### Вопросы\n\n1. Сколько карточек в стопке?\n2. Куда падает тень?",
        "Ссылки на референсы в `docs/reference`.",
        "Снять видео перетаскивания, *медленно*.",
        "- шрифт 15/20\n- мета 11, трекинг +0.15",
        "Сравнить блюр 4 и 6 pt на светлом фоне.",
        "Созвон в четверг, показать веер.",
    ];
    format!(
        "{}\n\n![Набросок {}]({})",
        TEXTS[i % TEXTS.len()],
        i + 1,
        picture(i)
    )
}

/// A small picture of its own for the `i`-th demo task: a gradient with a few
/// soft shapes, as an SVG data URL that markdown notes can show.
fn picture(i: usize) -> String {
    // Hues a golden angle apart, so no two tasks share colours.
    let a = (i * 137 + 200) % 360;
    let b = (a + 50) % 360;
    let shapes: String = (0..3 + i % 3)
        .map(|k| {
            let (x, y, r) = (
                40 + (k * 97 + i * 53) % 320,
                30 + (k * 61 + i * 29) % 140,
                18 + (k * 13 + i * 7) % 36,
            );
            format!("<circle cx='{x}' cy='{y}' r='{r}' fill='white' fill-opacity='0.35'/>")
        })
        .collect();
    let svg = format!(
        "<svg xmlns='http://www.w3.org/2000/svg' width='400' height='200'>\
         <defs><linearGradient id='g' x2='1' y2='1'>\
         <stop offset='0' stop-color='hsl({a},70%,62%)'/><stop offset='1' stop-color='hsl({b},70%,52%)'/>\
         </linearGradient></defs><rect width='400' height='200' fill='url(#g)'/>{shapes}</svg>"
    );
    let encoded: String = svg
        .bytes()
        .map(|c| match c {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'.' | b'_' => (c as char).to_string(),
            _ => format!("%{c:02X}"),
        })
        .collect();
    format!("data:image/svg+xml,{encoded}")
}
