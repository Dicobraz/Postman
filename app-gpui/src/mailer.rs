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
    Contacts,
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
    /// Живой ящик: статус синхронизации, строка входа, поиск на сервере (`live.rs`).
    pub(crate) live: crate::live::Live,
    /// Письма: вкладки и ответ в области чтения (S5-06б).
    pub(crate) compose: crate::compose::Composers,
    /// Окно экрана: полям ввода нового письма нужно окно, а письмо открывается и из событий синхронизации.
    pub(crate) window: AnyWindowHandle,
    /// Ответ, пересылка, удаление (S3-02).
    pub(crate) actions: crate::actions::Actions,
    /// Уровень песочницы последнего разбора; о слабой песочнице — один тост за запуск.
    pub(crate) sandbox: Option<Level>,
    /// Последнее нажатие и сработавшее действие — проверка хоткеев в любой раскладке (S1-01).
    pub(crate) last_key: String,
    pub(crate) bench: Option<Bench>,
    /// `--screens`: снимки экранов без человека (S2-05).
    pub(crate) screens: Option<screens::Driver>,
    pub(crate) section: Section,
    /// Календарь — своя кэшируемая view (S3-05); окна новой встречи — `meeting.rs`.
    pub(crate) cal: Entity<calendar::CalendarView>,
    pub(crate) meetings: crate::meeting::Meetings,
    /// Автоответ, квота (S4-05): панель настроек, полоса автоответа, строка состояния (`settings.rs`).
    pub(crate) settings: crate::settings::ServerSettings,
    /// Контакты и справочник — своя кэшируемая view (S4-02).
    pub(crate) contacts: Entity<crate::contacts::ContactsView>,
    /// «Отправить позже» и окно «Отменить» отправки (S5-06, `send_later.rs`).
    pub(crate) send_later: crate::send_later::SendLater,
    /// Вкладки в заголовке окна (S5-01).
    pub(crate) tabs: crate::tabs::Tabs,
    /// Меню «…» командной строки.
    pub(crate) more: crate::chrome::MoreMenu,
    pub(crate) _subscriptions: Vec<Subscription>,
}

impl Mailer {
    pub(crate) fn new(cache: Cache, bench: Option<Bench>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| InputState::new(window, cx).placeholder(tr!("frame-search")));
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
        let accounts = cache.accounts().unwrap_or_default();
        let inbox = crate::live::inbox(&mailboxes);
        let list = cx.new(|cx| {
            let mut l = MailList::new(cache.clone());
            l.set_rights(&mailboxes);
            if let Some(id) = inbox.clone() {
                l.show_mailbox(id, cx);
            }
            l
        });
        let folders = cx.new(|_| Folders::new(mailboxes, accounts, inbox));
        let reading = cx.new(|_| ReadingPane::new(cache.clone()));
        let bar = cx.new(|_| CommandBar::new(search.clone()));
        let toasts = cx.new(|_| Toasts::new());
        let cal = cx.new(|cx| calendar::CalendarView::new(window, cx));
        let contacts = cx.new(|cx| crate::contacts::ContactsView::new(window, cx));
        let subscriptions = vec![
            cx.subscribe(&contacts, |this, _, action: &crate::contacts::ContactsAction, cx| this.on_contacts(action, cx)),
            cx.subscribe(&cal, |this, _, action: &calendar::CalendarAction, cx| this.on_calendar(action, cx)),
            on_search,
            cx.subscribe(&bar, |this, bar, ev: &BarEvent, cx| match ev {
                BarEvent::More => this.toggle_more(cx),
                BarEvent::Scope => {
                    // Охват сменился посреди поиска — искать заново.
                    let q = bar.read(cx).search.read(cx).value().to_string();
                    if !q.trim().is_empty() {
                        this.run_search(&q, cx);
                    }
                }
            }),
            Self::observe_keys(cx),
            cx.subscribe(&list, |this, _, OpenRow(row), cx| this.open_letter(row.clone(), cx)),
            cx.subscribe(&list, |this, _, NeedMore(mailbox), cx| this.load_more(mailbox.clone(), cx)),
            cx.subscribe(&list, |this, _, _: &ReadOnly, cx| {
                this.toasts.update(cx, |t, cx| t.push(Toast::Info, tr!("shared-read-only-flags"), cx));
            }),
            cx.subscribe(&list, |this, _, n: &LabelNotice, cx| {
                let text = match n {
                    LabelNotice::NoCategory(key) => tr!("labels-no-category", key = *key),
                    LabelNotice::ReadOnly => tr!("labels-read-only"),
                };
                this.toasts.update(cx, |t, cx| t.push(Toast::Info, text, cx));
            }),
            crate::actions::watch_list(&list, cx),
            cx.subscribe(&folders, |this, _, FolderEvent::Open(id), cx| {
                this.tabs.activate_kind(crate::tabs::TabKind::Mail);
                this.open_mailbox(id.clone(), cx)
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
                ReadingEvent::Open(row) => this.open_letter(row.clone(), cx),
                ReadingEvent::Unsend(s) => this.unsend_scheduled(s.clone(), cx),
                ReadingEvent::Sync(req) => {
                    if !this.backend.send(req.command()) {
                        this.needs_server(cx);
                        this.reading.update(cx, |r, cx| r.not_sent(req, cx));
                    }
                }
            }),
            // Значок уведомлений в строке состояния.
            cx.observe(&toasts, |_, _, cx| cx.notify()),
            // Настройки (S5-09): «Панель чтения» меняет раскладку.
            crate::prefs::observe(cx),
            // Ответ в области чтения — над своим письмом: открыли другое — ответ уходит во вкладку или закрывается.
            // Скрытая панель чтения (S5-09) — письмо на месте списка: раскладка меняется вместе с письмом.
            cx.observe(&reading, |this, _, cx| {
                this.inline_follows_letter(cx);
                if crate::prefs::get(cx).reading.pane == postman_core::prefs::Pane::Hidden {
                    cx.notify();
                }
            }),
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
            live: Default::default(),
            compose: Default::default(),
            window: window.window_handle(),
            actions: Default::default(),
            sandbox: None,
            last_key: String::new(),
            bench,
            screens: None,
            section: Section::Mail,
            cal,
            meetings: Default::default(),
            settings: Default::default(),
            contacts,
            send_later: Default::default(),
            tabs: Default::default(),
            more: Default::default(),
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

    /// Событие синхронизации: каждая часть берёт своё (`on_sync`); общее (папки, список, статус, ошибки,
    /// поиск) — `live.rs`.
    pub(crate) fn on_sync(&mut self, ev: &postman_core::sync::Event, cx: &mut Context<Self>) {
        self.reading.update(cx, |r, cx| r.on_sync(ev, cx));
        let sent = self.compose.on_sync(ev, cx);
        self.on_send_sync(ev, sent, cx);
        self.cal.update(cx, |c, cx| c.on_sync(ev, cx));
        self.contacts.update(cx, |c, cx| c.on_sync(ev, cx));
        let created = self.meetings.on_sync(ev, cx);
        self.meetings_created(created, cx);
        self.on_settings_sync(ev, cx);
    }

    /// Действию нужен сервер, а его нет (демо): сказать тостом.
    pub(crate) fn needs_server(&mut self, cx: &mut Context<Self>) {
        self.toasts.update(cx, |t, cx| t.push(Toast::Info, tr!("needs-server"), cx));
    }

    pub(crate) fn open_mailbox(&mut self, id: String, cx: &mut Context<Self>) {
        self.live.clear_search();
        self.folders.update(cx, |f, cx| {
            f.current = Some(id.clone());
            f.results = None;
            cx.notify();
        });
        self.list.update(cx, |l, cx| l.show_mailbox(id, cx));
        self.sync_total(cx);
        self.reading.update(cx, |r, cx| r.clear(cx));
    }

    pub(crate) fn run_search(&mut self, q: &str, cx: &mut Context<Self>) {
        if self.backend.is_live() {
            return self.run_live_search(q, cx);
        }
        let q = q.trim();
        let mailbox = self.search_mailbox(cx);
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

    /// Папка поиска по охвату командной строки: «Текущая папка» — открытая, «Везде» — все (`None`).
    pub(crate) fn search_mailbox(&self, cx: &App) -> Option<String> {
        match self.bar.read(cx).scope {
            crate::bar::Scope::All => None,
            crate::bar::Scope::Folder => self.list.read(cx).mailbox.clone(),
        }
    }

    pub(crate) fn set_section(&mut self, section: Section, cx: &mut Context<Self>) {
        // Модуль рейла — во вкладке «Почта» (вкладка настроек остаётся открытой).
        self.tabs.activate_kind(crate::tabs::TabKind::Mail);
        if section == Section::Calendar && self.section != section {
            self.cal.update(cx, |c, cx| c.refresh(cx));
        }
        if section == Section::Contacts && self.section != section {
            self.contacts.update(cx, |c, cx| c.refresh(cx));
        }
        self.section = section;
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
        // Сервер отклонил сохранённый вход: строка входа (ей нужно окно — здесь оно есть).
        if let Some(prefill) = self.live.reauth.take() {
            cx.defer_in(window, move |this, window, cx| this.show_reauth(prefill, window, cx));
        }
        self.drive_bench(window, cx);
        screens::drive(self, window, cx);
        let l = crate::roles::look(cx);
        // Вкладка «Настройки» (S5-09) — на месте рейла и модуля.
        let settings_tab = self.tabs.is_active(crate::tabs::TabKind::Settings);
        let settings: Option<AnyView> = self.settings.view.clone().filter(|_| settings_tab).map(Into::into);
        // Вкладка письма (S5-06б): письмо на всю ширину, без командной строки и рейла (макет «Новое письмо»).
        let letter = match self.tabs.active().kind {
            crate::tabs::TabKind::Compose(id) => self.compose.get(id).cloned(),
            _ => None,
        };
        let free = settings.is_none();
        let first_run = self.section == Section::Mail && self.live.signin.is_some();
        // Командная строка и полоса автоответа — у почты; во вкладках «Настройки», письма и при первом запуске их нет.
        let bars = free && !first_run && letter.is_none();
        // Контекст раздела: клавиши писем (j/k, Delete, Ctrl+R…) — только в «Почте» без открытых настроек (`commands.rs`).
        let context = match self.section {
            _ if letter.is_some() => "Mailer ComposeView",
            _ if !free => "Mailer SettingsView",
            Section::Mail => "Mailer MailView",
            Section::Calendar => "Mailer CalendarView",
            Section::Contacts => "Mailer ContactsView",
        };
        let root = crate::actions::handlers(div(), cx)
            .key_context(context)
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
            .on_action(cx.listener(|this, a: &mail::Categorize, _, cx| this.list.update(cx, |l, cx| l.toggle_category(a.key, cx))))
            .on_action(cx.listener(|this, _: &commands::thread::Next, _, cx| this.thread_step(1, cx)))
            .on_action(cx.listener(|this, _: &commands::thread::Prev, _, cx| this.thread_step(-1, cx)))
            .on_action(cx.listener(|this, _: &commands::thread::Toggle, _, cx| this.reading.update(cx, |r, cx| r.toggle_feed(cx))))
            .on_action(cx.listener(|this, _: &commands::thread::Quotes, _, cx| this.reading.update(cx, |r, cx| r.toggle_quotes(cx))))
            .on_action(cx.listener(|this, _: &mail::New, _, cx| {
                this.open_compose(Default::default(), cx);
            }))
            .on_action(cx.listener(|this, _: &view::Mail, _, cx| this.set_section(Section::Mail, cx)))
            .on_action(cx.listener(|this, _: &view::Calendar, _, cx| this.set_section(Section::Calendar, cx)))
            .on_action(cx.listener(|this, _: &view::Contacts, _, cx| this.set_section(Section::Contacts, cx)))
            .on_action(cx.listener(|this, _: &commands::calendar::NewMeeting, _, cx| {
                this.open_meeting(None, cx);
            }))
            .on_action(cx.listener(|this, _: &commands::calendar::PrevWeek, _, cx| this.calendar_week(Some(-1), cx)))
            .on_action(cx.listener(|this, _: &commands::calendar::NextWeek, _, cx| this.calendar_week(Some(1), cx)))
            .on_action(cx.listener(|this, _: &commands::calendar::Today, _, cx| this.calendar_week(None, cx)))
            .on_action(cx.listener(|this, _: &search::Focus, window, cx| {
                if this.section == Section::Contacts {
                    return this.contacts.update(cx, |c, cx| c.focus_search(window, cx));
                }
                this.search.update(cx, |s, cx| s.focus(window, cx));
            }))
            .on_action(cx.listener(|this, _: &commands::settings::Open, window, cx| this.open_settings(None, window, cx)))
            .on_action(cx.listener(|this, _: &commands::settings::Vacation, window, cx| this.open_vacation(window, cx)))
            .on_action(cx.listener(|this, _: &commands::settings::Rules, window, cx| this.open_rules(window, cx)))
            .on_action(cx.listener(|this, _: &commands::tab::Next, window, cx| this.step_tab(1, window, cx)))
            .on_action(cx.listener(|this, _: &commands::tab::Prev, window, cx| this.step_tab(-1, window, cx)))
            .on_action(cx.listener(|this, _: &commands::tab::Close, window, cx| {
                let id = this.tabs.active().id;
                this.close_tab(id, window, cx);
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
                // …затем меню «…».
                if this.close_more(cx) {
                    return;
                }
                // …затем строку «Удалить навсегда?».
                if this.cancel_purge(cx) {
                    return;
                }
                // …затем запись клавиши и правку на месте в настройках, затем саму вкладку.
                if this.settings_escape(cx) {
                    return;
                }
                if this.tabs.is_active(crate::tabs::TabKind::Settings) && this.close_settings(window, cx) {
                    return;
                }
                // Панель чтения скрыта (Настройки → Чтение): письмо на месте списка — назад к списку.
                if this.section == Section::Mail && this.reading_instead_of_list(cx) {
                    this.reading.update(cx, |r, cx| r.clear(cx));
                    return;
                }
                // В контактах — строка «Удалить контакт?», форма, поиск контактов.
                if this.section == Section::Contacts && this.contacts.update(cx, |c, cx| c.escape(window, cx)) {
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
            .bg(l.base())
            .text_color(l.text())
            .text_size(px(13.))
            .child(self.titlebar(&l, window, cx))
            .when(bars, |d| d.child(self.bar.clone().cached(StyleRefinement::default().w_full().h(px(BAR_HEIGHT)).flex_none())))
            .when_some(self.settings.bar.clone().filter(|_| bars), |d, bar| {
                d.child(bar.cached(StyleRefinement::default().w_full().h(px(crate::vacation::BAR_HEIGHT)).flex_none()))
            })
            .map(|d| match letter {
                Some(view) => d.child(
                    div()
                        .flex()
                        .flex_1()
                        .min_h_0()
                        .px(px(8.))
                        .child(view.cached(StyleRefinement::default().size_full())),
                ),
                None => d.child(self.body(&l, settings, free, cx)),
            })
            .child(self.status_bar(&l, cx))
            .children(self.more_menu(&l, cx))
            // Окно «Отменить» отправки — внизу, над строкой состояния.
            .children(self.undo_card(cx))
            // Тосты — поверх всего, последним ребёнком.
            .child(self.toasts.clone());
        if let Some(b) = &mut self.bench {
            b.ui_time(started.elapsed());
        }
        root
    }
}

// --- раскладка (DESIGN «Главное окно»): рейл | папки | список | чтение ---

/// Ширина панели папок и карточки списка (макет главного окна).
const W_FOLDERS: f32 = 236.0;
const W_LIST: f32 = 452.0;
/// Доля высоты области чтения под ответом (остальное — письмо, на которое отвечают).
const INLINE_SHARE: f32 = 0.6;

/// Карточка на фоне каркаса: скругление сверху, рамка без нижней стороны.
fn card(l: &crate::roles::Look) -> Div {
    div()
        .h_full()
        .min_w_0()
        .flex()
        .flex_col()
        .overflow_hidden()
        .bg(l.panel())
        .rounded_t(l.px(pets_ui::theme::Metric::RadiusOverlay))
        .border_1()
        .border_b_0()
        .border_color(l.c(pets_ui::theme::Role::StrokeCard))
}

impl Mailer {
    /// Панель чтения скрыта и письмо открыто: оно — на месте списка (Esc — назад).
    pub(crate) fn reading_instead_of_list(&self, cx: &App) -> bool {
        crate::prefs::get(cx).reading.pane == postman_core::prefs::Pane::Hidden && self.reading.read(cx).reading.is_some()
    }

    fn body(&self, l: &crate::roles::Look, settings: Option<AnyView>, free: bool, cx: &App) -> Div {
        use postman_core::prefs::Pane;
        let full = || StyleRefinement::default().size_full();
        let mail = free && self.section == Section::Mail;
        // Вкладка «Настройки» — на всю ширину: своя навигация вместо рейла (макет «Настройки»).
        if let Some(p) = settings {
            return div().flex().flex_1().min_h_0().px(px(8.)).child(p.cached(full()));
        }
        // Нет учётной записи (первый запуск) — добавление аккаунта на месте всего экрана почты.
        if mail && let Some(s) = self.live.signin.clone() {
            return div().flex().flex_1().min_h_0().px(px(8.)).child(card(l).flex_1().child(s));
        }
        let mut body = div().flex().flex_1().min_h_0().pr(px(8.)).child(self.rail(l));
        if mail {
            body = body.child(self.folders.clone().cached(StyleRefinement::default().w(px(W_FOLDERS)).h_full().flex_none()));
            let list = || self.list.clone().cached(full());
            // Ответ — в области чтения над письмом (S5-06б): своя кэшируемая view сверху карточки.
            let reading = || match self.compose.inline_view() {
                Some(reply) => card(l)
                    .flex_1()
                    .child(reply.clone().cached(StyleRefinement::default().w_full().h(relative(INLINE_SHARE)).flex_none()))
                    .child(self.reading.clone().cached(StyleRefinement::default().w_full().flex_1().min_h_0())),
                None => card(l).flex_1().child(self.reading.clone().cached(full())),
            };
            // Панель чтения — справа, снизу или скрыта (Настройки → Чтение).
            return match crate::prefs::get(cx).reading.pane {
                Pane::Right => body
                    .child(card(l).w(px(W_LIST)).flex_none().mr(px(8.)).child(list()))
                    .child(reading()),
                Pane::Bottom => body.child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .h_full()
                        .flex()
                        .flex_col()
                        .gap(px(8.))
                        .child(card(l).h(relative(0.45)).flex_none().rounded_b(l.px(pets_ui::theme::Metric::RadiusOverlay)).border_b_1().child(list()))
                        .child(reading()),
                ),
                Pane::Hidden if self.reading_instead_of_list(cx) => body.child(reading()),
                Pane::Hidden => body.child(card(l).flex_1().child(list())),
            };
        }
        match self.section {
            Section::Calendar => body.child(card(l).flex_1().child(self.cal.clone().cached(full()))),
            Section::Contacts => body.child(card(l).flex_1().child(self.contacts.clone().cached(full()))),
            Section::Mail => body,
        }
    }
}

// --- замер: общий сценарий из postman_core::bench ---

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
