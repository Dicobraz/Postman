//! Экран почты: связывает части, держит действия (хоткеи), поиск, календарь, тосты и замер.

use std::time::Instant;

use gpui_kit::*;
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::prelude::*;
use pets_i18n::tr;
use pets_ui::{Level as Toast, Toasts};
use postman_cache::Cache;
use postman_core::bench::{self, Bench, Step};
use postman_worker::Level;

use crate::*;
use crate::backend::Backend;
use crate::bar::*;
use crate::commands::{mail, search, view};
use crate::folders::*;
use crate::list::*;
use crate::reading::*;

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum Section {
    Mail,
    Calendar,
}

// --- экран ---

/// Экран почты: связывает части, держит хоткеи, поиск, календарь и строку состояния.
/// Части — отдельные кэшируемые view; заново строится та, что получила `notify`.
pub(crate) struct Mailer {
    pub(crate) cache: Cache,
    pub(crate) focus: FocusHandle,
    pub(crate) list: Entity<MailList>,
    pub(crate) folders: Entity<Folders>,
    pub(crate) reading: Entity<ReadingPane>,
    pub(crate) bar: Entity<CommandBar>,
    pub(crate) search: Entity<InputState>,
    pub(crate) toasts: Entity<Toasts>,
    /// Сервер или демо (S3-04 подключает живой ящик).
    pub(crate) backend: Backend,
    /// Окна письма (S3-01).
    pub(crate) compose: crate::compose::Composers,
    /// Уровень песочницы последнего разбора; о слабой песочнице — один тост за запуск.
    pub(crate) sandbox: Option<Level>,
    /// Последнее нажатие и сработавшее действие — проверка хоткеев в любой раскладке (S1-01).
    pub(crate) last_key: String,
    pub(crate) bench: Option<Bench>,
    /// `--screens`: снимки экранов без человека (S2-05).
    pub(crate) screens: Option<screens::Driver>,
    pub(crate) section: Section,
    pub(crate) cal: calendar::CalendarView,
    pub(crate) _subscriptions: Vec<Subscription>,
}

impl Mailer {
    pub(crate) fn new(cache: Cache, bench: Option<Bench>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| InputState::new(window, cx).placeholder(tr!("search-placeholder")));
        let keylog = std::env::var_os("POSTMAN_KEYLOG").is_some();
        let on_search = cx.subscribe_in(&search, window, move |this: &mut Self, state, ev: &InputEvent, _window, cx| match ev {
            InputEvent::PressEnter { .. } => {
                let q = state.read(cx).value().to_string();
                this.run_search(&q, cx);
            }
            InputEvent::Change if keylog => println!("search \"{}\"", state.read(cx).value()),
            _ => {}
        });
        let mailboxes = cache.mailboxes().unwrap_or_default();
        let inbox = mailboxes.iter().find(|b| b.role.as_deref() == Some("inbox")).or(mailboxes.first()).map(|b| b.id.clone());
        let list = cx.new(|cx| {
            let mut l = MailList::new(cache.clone());
            if let Some(id) = inbox.clone() {
                l.show_mailbox(id, cx);
            }
            l
        });
        let folders = cx.new(|_| Folders { mailboxes, current: inbox, results: None, section: Section::Mail });
        let reading = cx.new(|_| ReadingPane::new(cache.clone()));
        let bar = cx.new(|_| CommandBar { search: search.clone() });
        let toasts = cx.new(|_| Toasts::new());
        let subscriptions = vec![
            on_search,
            Self::observe_keys(cx),
            cx.subscribe(&list, |this, _, OpenRow(row), cx| {
                this.reading.update(cx, |r, cx| r.open(row.clone(), cx));
            }),
            cx.subscribe(&folders, |this, _, ev: &FolderEvent, cx| match ev {
                FolderEvent::Open(id) => this.open_mailbox(id.clone(), cx),
                FolderEvent::Section(s) => this.set_section(*s, cx),
            }),
            cx.subscribe(&reading, |this, _, ev: &ReadingEvent, cx| match ev {
                ReadingEvent::Sandbox(level) => {
                    if *level != Level::Full && this.sandbox.is_none() {
                        let text = tr!("sandbox-weak", level = sandbox_label(*level));
                        this.toasts.update(cx, |t, cx| t.push(Toast::Warning, text, cx));
                    }
                    this.sandbox = Some(*level);
                    cx.notify();
                }
                ReadingEvent::Error(text) => this.toasts.update(cx, |t, cx| t.push(Toast::Error, text.clone(), cx)),
                ReadingEvent::Toast(level, text) => this.toasts.update(cx, |t, cx| t.push(*level, text.clone(), cx)),
                ReadingEvent::NeedsServer => this.needs_server(cx),
                ReadingEvent::Sync(req) => {
                    if !this.backend.send(req.command()) {
                        this.needs_server(cx);
                        this.reading.update(cx, |r, cx| r.not_sent(req, cx));
                    }
                }
            }),
            // Значок уведомлений в строке состояния.
            cx.observe(&toasts, |_, _, cx| cx.notify()),
        ];
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        Self {
            cache,
            focus,
            list,
            folders,
            reading,
            bar,
            search,
            toasts,
            backend: Backend::Demo,
            compose: Default::default(),
            sandbox: None,
            last_key: String::new(),
            bench,
            screens: None,
            section: Section::Mail,
            cal: calendar::CalendarView::demo(),
            _subscriptions: subscriptions,
        }
    }

    /// «Ctrl-e (у) → FocusSearch» в строке состояния; `POSTMAN_KEYLOG=1` — то же строкой `key …` в stdout.
    pub(crate) fn observe_keys(cx: &mut Context<Self>) -> Subscription {
        let to_stdout = std::env::var_os("POSTMAN_KEYLOG").is_some();
        cx.observe_keystrokes(move |this, ev, _window, cx| {
            let k = &ev.keystroke;
            let action = ev.action.as_ref().map_or("—", |a| a.name());
            this.last_key = format!("{} ({}) → {action}", k.unparse(), k.key_char.as_deref().unwrap_or(""));
            if to_stdout {
                println!("key {}", this.last_key);
            }
            cx.notify();
        })
    }

    /// Событие синхронизации: каждая часть берёт своё (`on_sync`). Поток событий подключает S3-04.
    #[expect(dead_code, reason = "phase 2 seam, wired by S3-01, S3-03, S3-04")]
    pub(crate) fn on_sync(&mut self, ev: &postman_core::sync::Event, cx: &mut Context<Self>) {
        self.reading.update(cx, |r, cx| r.on_sync(ev, cx));
        self.compose.on_sync(ev, cx);
    }

    /// Действию нужен сервер, а его нет (демо): сказать тостом.
    pub(crate) fn needs_server(&mut self, cx: &mut Context<Self>) {
        self.toasts.update(cx, |t, cx| t.push(Toast::Info, tr!("needs-server"), cx));
    }

    pub(crate) fn open_mailbox(&mut self, id: String, cx: &mut Context<Self>) {
        self.folders.update(cx, |f, cx| {
            f.current = Some(id.clone());
            f.results = None;
            cx.notify();
        });
        self.list.update(cx, |l, cx| l.show_mailbox(id, cx));
        self.reading.update(cx, |r, cx| r.clear(cx));
    }

    pub(crate) fn run_search(&mut self, q: &str, cx: &mut Context<Self>) {
        let q = q.trim();
        let mailbox = self.list.read(cx).mailbox.clone();
        let ids = (!q.is_empty()).then(|| self.cache.search(q, mailbox.as_deref(), 1000).unwrap_or_default());
        let results = ids.as_ref().map(Vec::len);
        if results == Some(0) {
            let text = tr!("search-nothing", query = q.to_string());
            self.toasts.update(cx, |t, cx| t.push(Toast::Info, text, cx));
        }
        self.folders.update(cx, |f, cx| {
            f.results = results;
            cx.notify();
        });
        self.list.update(cx, |l, cx| l.show_search(ids, cx));
        self.reading.update(cx, |r, cx| r.clear(cx));
    }

    pub(crate) fn set_section(&mut self, section: Section, cx: &mut Context<Self>) {
        self.section = section;
        self.folders.update(cx, |f, cx| {
            f.section = section;
            cx.notify();
        });
        cx.notify();
    }

    /// Сдвиг выделения; `delta` считается по состоянию списка (страница, число писем).
    pub(crate) fn step(&mut self, delta: impl Fn(&MailList) -> isize, cx: &mut Context<Self>) {
        self.list.update(cx, |l, cx| {
            let d = delta(l);
            l.step(d, cx);
        });
    }
}

impl Render for Mailer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let started = Instant::now();
        self.drive_bench(window, cx);
        screens::drive(self, window, cx);
        let status = tr!("status-items", n = self.list.read(cx).count);
        let unread = self.toasts.read(cx).unread();
        let root = div()
            .key_context("Mailer")
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &mail::Next, _, cx| this.step(|_| 1, cx)))
            .on_action(cx.listener(|this, _: &mail::Prev, _, cx| this.step(|_| -1, cx)))
            .on_action(cx.listener(|this, _: &mail::First, _, cx| this.step(|l| -(l.count as isize), cx)))
            .on_action(cx.listener(|this, _: &mail::Last, _, cx| this.step(|l| l.count as isize, cx)))
            .on_action(cx.listener(|this, _: &mail::PageDown, _, cx| this.step(MailList::page, cx)))
            .on_action(cx.listener(|this, _: &mail::PageUp, _, cx| this.step(|l| -l.page(), cx)))
            .on_action(cx.listener(|this, _: &mail::MarkRead, _, cx| this.list.update(cx, |l, cx| l.set_flags(Some(true), false, cx))))
            .on_action(cx.listener(|this, _: &mail::MarkUnread, _, cx| this.list.update(cx, |l, cx| l.set_flags(Some(false), false, cx))))
            .on_action(cx.listener(|this, _: &mail::ToggleFlag, _, cx| this.list.update(cx, |l, cx| l.set_flags(None, true, cx))))
            .on_action(cx.listener(|this, _: &mail::New, _, cx| {
                this.open_compose(Default::default(), cx);
            }))
            .on_action(cx.listener(|this, _: &view::Mail, _, cx| this.set_section(Section::Mail, cx)))
            .on_action(cx.listener(|this, _: &view::Calendar, _, cx| this.set_section(Section::Calendar, cx)))
            .on_action(cx.listener(|this, _: &commands::calendar::NewMeeting, _, cx| {
                this.cal.assistant = !this.cal.assistant;
                this.set_section(Section::Calendar, cx);
            }))
            .on_action(cx.listener(|this, _: &search::Focus, window, cx| {
                this.search.update(cx, |s, cx| s.focus(window, cx));
            }))
            .on_action(cx.listener(|this, _: &view::Notifications, _, cx| {
                this.toasts.update(cx, |t, cx| t.toggle_history(cx));
            }))
            .on_action(cx.listener(|this, _: &search::Clear, window, cx| {
                // Esc сначала закрывает историю уведомлений.
                if this.toasts.read(cx).is_history_open() {
                    this.toasts.update(cx, |t, cx| t.toggle_history(cx));
                    return;
                }
                this.search.update(cx, |s, cx| s.set_value("", window, cx));
                this.run_search("", cx);
                window.focus(&this.focus, cx);
            }))
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .bg(rgb(0xfafafa))
            .text_color(rgb(0x222222))
            .text_size(px(13.))
            .child(self.bar.clone().cached(StyleRefinement::default().w_full().h(px(BAR_HEIGHT)).flex_none()))
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .child(self.folders.clone().cached(StyleRefinement::default().w(px(200.)).h_full().flex_none()))
                    .when(self.section == Section::Mail, |d| {
                        d.child(self.list.clone().cached(StyleRefinement::default().w(px(380.)).h_full().flex_none()))
                            .child(self.reading.clone().cached(StyleRefinement::default().flex_1().min_w_0().h_full()))
                    })
                    .when(self.section == Section::Calendar, |d| d.child(calendar::render(self, cx))),
            )
            .child(
                div()
                    .flex()
                    .gap_3()
                    .px_2()
                    .py_0p5()
                    .border_t_1()
                    .border_color(rgb(0xdddddd))
                    .text_color(rgb(0x888888))
                    .child(status)
                    .child(div().flex_1())
                    .child(self.last_key.clone())
                    .child(self.sandbox.map(sandbox_label).unwrap_or_default())
                    .child(
                        div()
                            .id("notifications")
                            .px_1()
                            .rounded_md()
                            .cursor_pointer()
                            .hover(|d| d.bg(rgb(0xececec)))
                            .when(unread > 0, |d| d.text_color(rgb(0x0f6cbd)))
                            .child(tr!("status-notifications", n = unread))
                            .on_click(cx.listener(|this, _, _, cx| this.toasts.update(cx, |t, cx| t.toggle_history(cx)))),
                    )
                    .child("gpui"),
            )
            // Тосты — поверх всего, последним ребёнком.
            .child(self.toasts.clone());
        if let Some(b) = &mut self.bench {
            b.ui_time(started.elapsed());
        }
        root
    }
}

// --- замер: общий сценарий из postman_core::bench, тот же, что в egui-версии ---

impl Mailer {
    pub(crate) fn drive_bench(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let count = self.list.read(cx).count;
        let Some(b) = &mut self.bench else { return };
        match b.frame(count) {
            Step::Wait => {}
            Step::Select(i) => self.list.update(cx, |l, cx| l.select(i, cx)),
            Step::Scroll(i) => self.list.update(cx, |l, cx| l.scroll_to_top(i, cx)),
            Step::Resize(w, h) => window.resize(size(px(w), px(h))),
            Step::Done(report) => {
                // Дальше — простой: кадры больше не запрашиваем, процесс завершит замер.
                bench::finish(format!("bench ui=gpui {report}"));
                self.bench = None;
                return;
            }
        }
        window.request_animation_frame();
    }
}
