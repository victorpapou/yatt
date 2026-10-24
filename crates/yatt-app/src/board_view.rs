//! Placeholder board UI: just enough to drive the backend by hand.
//! The real look comes from the mockups; layout and motion get replaced then.

use std::time::Duration;

use gpui::{
    App, AppContext, ClickEvent, Context, ElementId, FocusHandle, Hsla, InteractiveElement,
    IntoElement, ParentElement, Pixels, Point, Render, SharedString, StatefulInteractiveElement,
    Styled, Window, actions, div, prelude::FluentBuilder, px, rgb,
};
use jiff::Timestamp;
use yatt_core::{Mode, Phase, Placement, Status, TaskId};

use crate::state::AppState;

actions!(yatt, [Undo, Redo, NewTask, StartPause, Stop, Skip]);

/// What travels with a dragged card.
#[derive(Clone)]
struct DraggedTask {
    id: TaskId,
    title: SharedString,
}

impl Render for DraggedTask {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        card_shell(false).opacity(0.9).child(self.title.clone())
    }
}

pub struct BoardView {
    state: AppState,
    focus: FocusHandle,
    new_count: usize,
}

impl BoardView {
    pub fn new(state: AppState, cx: &mut Context<Self>) -> Self {
        // Redraw the clock a few times a second. The timer itself is computed
        // from timestamps, so the interval only affects smoothness, not accuracy.
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(200))
                    .await;
                let alive = this.update(cx, |view, cx| {
                    view.state.tick();
                    if view.state.timer.is_running() {
                        cx.notify();
                    }
                });
                if alive.is_err() {
                    break;
                }
            }
        })
        .detach();
        Self {
            state,
            focus: cx.focus_handle(),
            new_count: 0,
        }
    }

    pub fn focus_handle(&self) -> FocusHandle {
        self.focus.clone()
    }

    fn header(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let now = Timestamp::now();
        let timer = &self.state.timer;
        let clock = match timer.remaining_ms(now) {
            Some(ms) => format_clock(ms),
            None => format_clock(timer.elapsed_ms(now)),
        };
        let phase = match timer.phase {
            Phase::Idle => "Готов",
            Phase::Work if timer.mode == Mode::Stopwatch => "Секундомер",
            Phase::Work => "Работа",
            Phase::ShortBreak => "Перерыв",
            Phase::LongBreak => "Длинный перерыв",
        };
        let start_label = if timer.is_running() {
            "Пауза"
        } else if timer.phase == Phase::Idle {
            "Старт"
        } else {
            "Продолжить"
        };
        let active = timer
            .active_tasks
            .iter()
            .filter_map(|id| self.state.board.get(*id))
            .map(|t| t.title.as_str())
            .collect::<Vec<_>>()
            .join(", ");

        div()
            .flex()
            .items_center()
            .gap_3()
            .p_4()
            .border_b_1()
            .border_color(rgb(0x2a2a2e))
            .child(div().text_3xl().child(SharedString::from(clock)))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .child(SharedString::from(phase))
                    .child(
                        div()
                            .text_sm()
                            .text_color(rgb(0x8a8a93))
                            .child(SharedString::from(if active.is_empty() {
                                "нет активных задач".to_string()
                            } else {
                                active
                            })),
                    ),
            )
            .child(div().flex_1())
            .child(button("start", start_label, cx, |this, cx| {
                this.state.start_or_pause(Mode::Pomodoro);
                cx.notify();
            }))
            .when(timer.phase == Phase::Idle, |el| {
                el.child(button(
                    "stopwatch",
                    "Секундомер",
                    cx,
                    |this, cx| {
                        this.state.start_or_pause(Mode::Stopwatch);
                        cx.notify();
                    },
                ))
            })
            .when(timer.phase != Phase::Idle, |el| {
                el.child(button("skip", "Пропустить", cx, |this, cx| {
                    this.state.skip();
                    cx.notify();
                }))
                .child(button("stop", "Стоп", cx, |this, cx| {
                    this.state.stop();
                    cx.notify();
                }))
            })
            .child(button("new", "+ Задача", cx, |this, cx| {
                this.add_task();
                cx.notify();
            }))
    }

    fn add_task(&mut self) {
        self.new_count += 1;
        self.state
            .create_task(format!("Новая задача {}", self.new_count));
    }

    fn column(&self, status: Status, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let title = match status {
            Status::Backlog => "Бэклог",
            Status::InProgress => "В работе",
            Status::Done => "Готово",
        };
        let tasks = self.state.board.column(status, &self.state.filter);
        let count = tasks.len();

        let cards = tasks.into_iter().enumerate().map(|(index, task)| {
            let id = task.id;
            let active = self.state.timer.active_tasks.contains(&id);
            let time = self.state.times.get(&id).copied().unwrap_or_default();
            let meta = format!(
                "{} · сегодня {} · 🍅 {}",
                format_duration(time.total_ms),
                format_duration(time.today_ms),
                time.pomodoros
            );
            let dragged = DraggedTask {
                id,
                title: task.title.clone().into(),
            };
            card_shell(active)
                .id(ElementId::Name(SharedString::from(format!("card-{id}"))))
                .cursor_pointer()
                .child(SharedString::from(task.title.clone()))
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(0x8a8a93))
                        .child(SharedString::from(meta)),
                )
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    this.state.toggle_active(id);
                    cx.notify();
                }))
                .on_drag(
                    dragged,
                    |d: &DraggedTask, _: Point<Pixels>, _: &mut Window, cx: &mut App| {
                        cx.new(|_| d.clone())
                    },
                )
                .drag_over::<DraggedTask>(|style, _, _, _| style.border_color(accent()))
                .on_drop(cx.listener(move |this, d: &DraggedTask, _, cx| {
                    this.state.move_task(d.id, status, Placement::Index(index));
                    cx.notify();
                }))
        });

        div()
            .id(ElementId::Name(SharedString::from(format!(
                "column-{}",
                status.as_str()
            ))))
            .flex()
            .flex_col()
            .flex_1()
            .gap_2()
            .p_3()
            .rounded_lg()
            .bg(rgb(0x18181b))
            .child(
                div()
                    .flex()
                    .justify_between()
                    .text_sm()
                    .text_color(rgb(0x8a8a93))
                    .child(SharedString::from(title))
                    .child(SharedString::from(count.to_string())),
            )
            .children(cards)
            .child(div().flex_1().min_h(px(40.)))
            .drag_over::<DraggedTask>(|style, _, _, _| style.bg(rgb(0x1f1f24)))
            .on_drop(cx.listener(move |this, d: &DraggedTask, _, cx| {
                this.state.move_task(d.id, status, Placement::Bottom);
                cx.notify();
            }))
    }
}

impl Render for BoardView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .track_focus(&self.focus)
            .key_context("Board")
            .on_action(cx.listener(|this, _: &Undo, _, cx| {
                this.state.undo();
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &Redo, _, cx| {
                this.state.redo();
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &NewTask, _, cx| {
                this.add_task();
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &StartPause, _, cx| {
                this.state.start_or_pause(Mode::Pomodoro);
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &Stop, _, cx| {
                this.state.stop();
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &Skip, _, cx| {
                this.state.skip();
                cx.notify();
            }))
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(0x0f0f11))
            .text_color(rgb(0xe8e8ea))
            .child(self.header(cx))
            .child(
                div()
                    .flex()
                    .flex_1()
                    .gap_3()
                    .p_4()
                    .children(Status::ALL.map(|s| self.column(s, cx))),
            )
    }
}

fn card_shell(active: bool) -> gpui::Div {
    div()
        .flex()
        .flex_col()
        .gap_1()
        .p_3()
        .rounded_md()
        .border_1()
        .border_color(if active {
            accent()
        } else {
            rgb(0x2a2a2e).into()
        })
        .bg(rgb(0x232327))
}

fn button(
    id: &'static str,
    label: &'static str,
    cx: &mut Context<BoardView>,
    on_click: impl Fn(&mut BoardView, &mut Context<BoardView>) + 'static,
) -> impl IntoElement {
    div()
        .id(id)
        .px_3()
        .py_1()
        .rounded_md()
        .bg(rgb(0x2a2a2e))
        .hover(|s| s.bg(rgb(0x3a3a40)))
        .cursor_pointer()
        .child(label)
        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| on_click(this, cx)))
}

fn accent() -> Hsla {
    rgb(0x4f8cff).into()
}

fn format_clock(ms: i64) -> String {
    // Round up so a countdown shows 25:00 at the start and hits 00:00 at the end.
    let secs = (ms + 999) / 1000;
    format!("{:02}:{:02}", secs / 60, secs % 60)
}

fn format_duration(ms: i64) -> String {
    let mins = ms / 60_000;
    if mins < 60 {
        format!("{mins}м")
    } else {
        format!("{}ч {:02}м", mins / 60, mins % 60)
    }
}
