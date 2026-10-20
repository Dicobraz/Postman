//! Контакты (S4-02): раздел «Контакты» (Ctrl+3) — личные контакты (JMAP ContactCard) и справочник сотрудников
//! (Principals: сотрудники, переговорные, ресурсы), поиск, карточка, «Написать письмо»; создать, изменить и удалить
//! личный контакт (`ContactCard/set`), удаление — после строки «Удалить контакт? Да / Нет». Справочник — только
//! просмотр.
//!
//! Демо: контакты из `postman_demo::contacts`; правка и удаление — только в памяти этого окна (на сервер ничего
//! не уходит, после перезапуска — снова демо-набор). Живой ящик: данные — событиями синхронизации
//! (`ContactsLoaded`, `DirectoryLoaded`), изменения — командами (`SaveContact`, `DeleteContact`).
//!
//! Своя кэшируемая view: перерисовывается только по своему `notify`. С экраном говорит событиями [`ContactsAction`].

use std::ops::Range;

use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::scroll::{Scrollbar, ScrollbarMode};
use gpui_kit::prelude::*;
use gpui_kit::*;
use pets_i18n::tr;
use pets_ui::Level as Toast;
use postman_compose::{Address, Draft, parse_addresses};
use postman_core::contacts::AddressBook;
use postman_core::sync::{Command, Event as SyncEvent};
use postman_jmap::{ContactCard, Principal};

use crate::calendar::{ACCENT, BORDER, BUTTON, ERROR, MUTED, PICKED, button};

const LIST_W: f32 = 320.;
const ROW_H: f32 = 40.;
const LABEL_W: f32 = 110.;

/// Откуда список: личная адресная книга или справочник организации.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Source {
    Mine,
    Directory,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Kind {
    Contact,
    Person,
    Room,
    Resource,
}

impl Kind {
    /// Тип Principal (RFC 9670): `individual`, `location`, `resource`…
    fn of_principal(kind: &str) -> Self {
        match kind {
            "location" => Kind::Room,
            "resource" => Kind::Resource,
            _ => Kind::Person,
        }
    }

    fn label(self) -> String {
        tr!(match self {
            Kind::Contact => "contacts-kind-contact",
            Kind::Person => "contacts-kind-person",
            Kind::Room => "contacts-kind-room",
            Kind::Resource => "contacts-kind-resource",
        })
    }
}

/// Строка списка: карточка и слова для поиска (считаются один раз).
#[derive(Clone, Debug)]
pub(crate) struct Entry {
    pub(crate) card: ContactCard,
    pub(crate) kind: Kind,
    keys: Vec<String>,
    /// Цифры телефонов: «495 123» находит «+7 (495) 123-45-67».
    digits: Vec<String>,
}

impl Entry {
    pub(crate) fn new(card: ContactCard, kind: Kind) -> Self {
        let mut keys = Vec::new();
        for text in [&card.name, &card.organization, &card.title] {
            keys.extend(
                normalize(text).split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).map(str::to_string),
            );
        }
        for e in &card.emails {
            let e = normalize(e);
            keys.extend(e.split(['.', '-', '_', '@']).filter(|p| !p.is_empty()).map(str::to_string));
            keys.extend(e.split_once('@').map(|(_, domain)| domain.to_string()));
            keys.push(e);
        }
        let digits = card.phones.iter().map(|p| p.chars().filter(char::is_ascii_digit).collect()).collect();
        Self { card, kind, keys, digits }
    }

    fn from_principal(p: Principal) -> Self {
        let card = ContactCard { id: p.id, name: p.name, emails: p.email.into_iter().collect(), ..Default::default() };
        Self::new(card, Kind::of_principal(&p.kind))
    }

    /// Слово запроса: начало слова имени, организации, должности или адреса; номер — часть телефона.
    fn matches(&self, word: &str) -> bool {
        let phone_like = word.chars().all(|c| c.is_ascii_digit() || "+-()".contains(c));
        let digits: String = word.chars().filter(char::is_ascii_digit).collect();
        if phone_like && !digits.is_empty() && self.digits.iter().any(|d| d.contains(&digits)) {
            return true;
        }
        self.keys.iter().any(|k| k.starts_with(word))
    }

    fn name(&self) -> String {
        match (self.card.name.trim(), self.card.emails.first()) {
            ("", Some(e)) => e.clone(),
            ("", None) => tr!("contacts-no-name"),
            (n, _) => n.to_string(),
        }
    }
}

fn normalize(s: &str) -> String {
    s.to_lowercase().replace('ё', "е") // i18n-ok: ё = е в поиске
}

/// Индексы строк, подходящих под запрос: каждое слово запроса где-то совпало. Пустой запрос — все.
pub(crate) fn filter(entries: &[Entry], query: &str) -> Vec<usize> {
    let words: Vec<String> = normalize(query).split_whitespace().map(str::to_string).collect();
    (0..entries.len()).filter(|&i| words.iter().all(|w| entries[i].matches(w))).collect()
}

/// По имени (без регистра), затем по адресу.
fn sort(entries: &mut [Entry]) {
    entries.sort_by_cached_key(|e| (normalize(&e.name()), e.card.emails.first().cloned()));
}

// --- форма карточки (без gpui) ---

/// Текст полей формы.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct FormText {
    pub(crate) name: String,
    pub(crate) emails: String,
    pub(crate) phones: String,
    pub(crate) organization: String,
    pub(crate) title: String,
}

#[derive(Debug, PartialEq)]
pub(crate) enum FormError {
    NeedName,
    BadEmail(String),
}

impl FormError {
    fn text(&self) -> String {
        match self {
            FormError::NeedName => tr!("contacts-need-name"),
            FormError::BadEmail(e) => tr!("contacts-bad-email", email = e.clone()),
        }
    }
}

impl FormText {
    pub(crate) fn of(card: &ContactCard) -> Self {
        Self {
            name: card.name.clone(),
            emails: card.emails.join("; "),
            phones: card.phones.join("; "),
            organization: card.organization.clone(),
            title: card.title.clone(),
        }
    }

    /// Карточка из полей (`id` — прежней карточки, у новой пустой): адреса и телефоны — через `;` или `,`.
    pub(crate) fn card(&self, id: &str) -> Result<ContactCard, FormError> {
        let (ok, bad) = parse_addresses(&self.emails);
        if let Some(b) = bad.into_iter().next() {
            return Err(FormError::BadEmail(b));
        }
        let card = ContactCard {
            id: id.to_string(),
            name: self.name.trim().to_string(),
            emails: ok.into_iter().map(|a| a.email).collect(),
            phones: self
                .phones
                .split([';', ','])
                .map(str::trim)
                .filter(|p| !p.is_empty())
                .map(str::to_string)
                .collect(),
            organization: self.organization.trim().to_string(),
            title: self.title.trim().to_string(),
        };
        if card.name.is_empty() && card.emails.is_empty() {
            return Err(FormError::NeedName);
        }
        Ok(card)
    }
}

/// Поля формы на экране.
struct Form {
    /// Прежняя карточка (правка); `None` — новая.
    old: Option<ContactCard>,
    name: Entity<InputState>,
    emails: Entity<InputState>,
    phones: Entity<InputState>,
    organization: Entity<InputState>,
    title: Entity<InputState>,
    error: Option<String>,
}

impl Form {
    fn text(&self, cx: &App) -> FormText {
        let v = |s: &Entity<InputState>| s.read(cx).value().to_string();
        FormText {
            name: v(&self.name),
            emails: v(&self.emails),
            phones: v(&self.phones),
            organization: v(&self.organization),
            title: v(&self.title),
        }
    }
}

enum Mode {
    View,
    Edit(Box<Form>),
    /// Строка «Удалить контакт? Да / Нет».
    Confirm,
}

/// Что раздел просит у экрана: сервер, окно письма, тост.
pub(crate) enum ContactsAction {
    /// Перечитать контакты и справочник (`Command::Contacts`).
    Load,
    Save {
        card: Box<ContactCard>,
        old: Option<Box<ContactCard>>,
        ticket: u64,
    },
    Delete {
        id: String,
        ticket: u64,
    },
    /// Окно письма с адресом.
    Write(Address),
    Toast(Toast, String),
}

pub(crate) struct ContactsView {
    live: bool,
    source: Source,
    mine: Vec<Entry>,
    directory: Vec<Entry>,
    /// Ждём контакты с сервера.
    loading: bool,
    /// Контакты пришли с сервера (снимки `--screens --live` ждут их).
    pub(crate) loaded: bool,
    search: Entity<InputState>,
    query: String,
    /// Индексы показанных строк текущего источника (после поиска).
    shown: Vec<usize>,
    selected: Option<String>,
    mode: Mode,
    /// Тикет сохранения или удаления, которого ждём с сервера.
    pending: Option<u64>,
    next_ticket: u64,
    next_local: u64,
    /// Ошибка загрузки — строкой над списком.
    error: Option<String>,
    /// Демо: о том, что правка только в окне, — один тост за запуск.
    demo_noted: bool,
    /// Снимок `contacts`: выделить первую строку, как только она появится (живой ящик — после ответа сервера).
    want_first: bool,
    scroll: UniformListScrollHandle,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<ContactsAction> for ContactsView {}

impl ContactsView {
    /// Демо-контакты; живой ящик переключает [`ContactsView::go_live`].
    pub(crate) fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| InputState::new(window, cx).placeholder(tr!("contacts-search-placeholder")));
        let on_search = cx.subscribe(&search, |this: &mut Self, state, ev: &InputEvent, cx| {
            if let InputEvent::Change = ev {
                this.query = state.read(cx).value().to_string();
                this.refilter(cx);
            }
        });
        let mut this = Self {
            live: false,
            source: Source::Mine,
            mine: demo_mine(),
            directory: demo_directory(),
            loading: false,
            loaded: false,
            search,
            query: String::new(),
            shown: Vec::new(),
            selected: None,
            mode: Mode::View,
            pending: None,
            next_ticket: 1,
            next_local: 1,
            error: None,
            demo_noted: false,
            want_first: false,
            scroll: UniformListScrollHandle::new(),
            _subscriptions: vec![on_search],
        };
        this.shown = filter(this.entries(), "");
        this
    }

    /// Живой ящик: контакты — с сервера (поток синхронизации присылает их сам после входа).
    pub(crate) fn go_live(&mut self, cx: &mut Context<Self>) {
        self.live = true;
        self.mine.clear();
        self.directory.clear();
        self.loading = true;
        self.loaded = false;
        self.selected = None;
        self.mode = Mode::View;
        self.pending = None;
        self.refilter(cx);
    }

    /// Раздел открыт: перечитать с сервера.
    pub(crate) fn refresh(&mut self, cx: &mut Context<Self>) {
        if self.live {
            self.loading = true;
            cx.emit(ContactsAction::Load);
            cx.notify();
        }
    }

    /// Поток синхронизации не принял команду: ждать нечего.
    pub(crate) fn not_sent(&mut self, cx: &mut Context<Self>) {
        self.loading = false;
        self.pending = None;
        cx.notify();
    }

    fn entries(&self) -> &[Entry] {
        match self.source {
            Source::Mine => &self.mine,
            Source::Directory => &self.directory,
        }
    }

    fn selected_entry(&self) -> Option<&Entry> {
        let id = self.selected.as_deref()?;
        self.entries().iter().find(|e| e.card.id == id)
    }

    fn refilter(&mut self, cx: &mut Context<Self>) {
        self.shown = filter(self.entries(), &self.query);
        cx.notify();
    }

    fn set_source(&mut self, source: Source, cx: &mut Context<Self>) {
        if self.source != source {
            self.source = source;
            self.selected = None;
            self.mode = Mode::View;
            self.scroll.scroll_to_item(0, ScrollStrategy::Top);
            self.refilter(cx);
        }
    }

    fn select(&mut self, id: String, cx: &mut Context<Self>) {
        if matches!(self.mode, Mode::Edit(_)) && self.selected.as_deref() == Some(id.as_str()) {
            return;
        }
        self.selected = Some(id);
        self.mode = Mode::View;
        cx.notify();
    }

    /// Первая строка списка (снимок `contacts`); личных нет — первый сотрудник.
    pub(crate) fn select_first(&mut self, cx: &mut Context<Self>) {
        self.want_first = true;
        if self.mine.is_empty() && !self.directory.is_empty() {
            self.set_source(Source::Directory, cx);
        }
        self.selected = self.shown.first().and_then(|&i| self.entries().get(i)).map(|e| e.card.id.clone());
        self.mode = Mode::View;
        cx.notify();
    }

    /// Что показывать уже есть: демо — сразу, живой ящик — после ответа сервера.
    pub(crate) fn ready(&self) -> bool {
        (!self.live || self.loaded) && (self.selected.is_some() || self.shown.is_empty())
    }

    /// Правка выделенного личного контакта (снимок `contact-edit`).
    pub(crate) fn edit_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(card) = self.selected_entry().filter(|e| e.kind == Kind::Contact).map(|e| e.card.clone()) {
            self.start_edit(Some(card), window, cx);
        }
    }

    pub(crate) fn editing(&self) -> bool {
        matches!(self.mode, Mode::Edit(_))
    }

    /// Ctrl+E в разделе.
    pub(crate) fn focus_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.search.update(cx, |s, cx| s.focus(window, cx));
    }

    /// Esc: строка подтверждения, затем форма, затем поиск. `false` — сбрасывать было нечего.
    pub(crate) fn escape(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        match self.mode {
            Mode::Confirm | Mode::Edit(_) => {
                self.mode = Mode::View;
                self.pending = None;
            }
            Mode::View if !self.query.is_empty() => {
                self.search.update(cx, |s, cx| s.set_value("", window, cx));
                self.query.clear();
                self.shown = filter(self.entries(), "");
            }
            Mode::View => return false,
        }
        cx.notify();
        true
    }

    fn start_edit(&mut self, old: Option<ContactCard>, window: &mut Window, cx: &mut Context<Self>) {
        let text = old.as_ref().map(FormText::of).unwrap_or_default();
        let mut field = |value: String, placeholder: Option<&str>, cx: &mut Context<Self>| {
            let state = cx.new(|cx| {
                let s = InputState::new(window, cx).default_value(value);
                match placeholder {
                    Some(p) => s.placeholder(tr!(p)),
                    None => s,
                }
            });
            self._subscriptions.push(cx.subscribe_in(&state, window, |this, _, ev: &InputEvent, _, cx| {
                if let InputEvent::PressEnter { .. } = ev {
                    this.save(cx);
                }
            }));
            state
        };
        let name = field(text.name, Some("contacts-name-placeholder"), cx);
        let emails = field(text.emails, Some("contacts-emails-placeholder"), cx);
        let phones = field(text.phones, Some("contacts-phones-placeholder"), cx);
        let organization = field(text.organization, None, cx);
        let title = field(text.title, None, cx);
        name.update(cx, |s, cx| s.focus(window, cx));
        if old.is_none() {
            self.set_source(Source::Mine, cx);
        }
        self.pending = None;
        self.mode = Mode::Edit(Box::new(Form { old, name, emails, phones, organization, title, error: None }));
        cx.notify();
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        let Mode::Edit(form) = &mut self.mode else { return };
        if self.pending.is_some() {
            return;
        }
        let id = form.old.as_ref().map(|c| c.id.clone()).unwrap_or_default();
        let card = match form.text(cx).card(&id) {
            Ok(c) => c,
            Err(e) => {
                form.error = Some(e.text());
                cx.notify();
                return;
            }
        };
        form.error = None;
        let old = form.old.clone();
        if !self.live {
            return self.save_local(card, old.is_none(), cx);
        }
        let ticket = self.ticket();
        self.pending = Some(ticket);
        cx.emit(ContactsAction::Save { card: Box::new(card), old: old.map(Box::new), ticket });
        cx.notify();
    }

    /// Демо: правка в памяти окна.
    fn save_local(&mut self, mut card: ContactCard, new: bool, cx: &mut Context<Self>) {
        if new {
            card.id = format!("local-{}", self.next_local);
            self.next_local += 1;
        }
        let name = card.name.clone();
        self.mine.retain(|e| e.card.id != card.id);
        self.selected = Some(card.id.clone());
        self.mine.push(Entry::new(card, Kind::Contact));
        sort(&mut self.mine);
        self.mode = Mode::View;
        self.refilter(cx);
        cx.emit(ContactsAction::Toast(Toast::Success, tr!("contacts-saved", name = name)));
        self.note_demo(cx);
    }

    fn note_demo(&mut self, cx: &mut Context<Self>) {
        if !std::mem::replace(&mut self.demo_noted, true) {
            cx.emit(ContactsAction::Toast(Toast::Info, tr!("contacts-demo-local")));
        }
    }

    fn ticket(&mut self) -> u64 {
        let t = self.next_ticket;
        self.next_ticket += 1;
        t
    }

    fn delete(&mut self, cx: &mut Context<Self>) {
        let Some(e) = self.selected_entry().filter(|e| e.kind == Kind::Contact) else { return };
        let (id, name) = (e.card.id.clone(), e.name());
        if !self.live {
            self.mine.retain(|e| e.card.id != id);
            self.selected = None;
            self.mode = Mode::View;
            self.refilter(cx);
            cx.emit(ContactsAction::Toast(Toast::Success, tr!("contacts-deleted", name = name)));
            return self.note_demo(cx);
        }
        let ticket = self.ticket();
        self.pending = Some(ticket);
        cx.emit(ContactsAction::Delete { id, ticket });
        cx.notify();
    }

    pub(crate) fn on_sync(&mut self, ev: &SyncEvent, cx: &mut Context<Self>) {
        match ev {
            SyncEvent::ContactsLoaded(cards) if self.live => {
                self.mine = cards.iter().cloned().map(|c| Entry::new(c, Kind::Contact)).collect();
                sort(&mut self.mine);
                self.loading = false;
                self.loaded = true;
                self.error = None;
            }
            SyncEvent::DirectoryLoaded(people) if self.live => {
                self.directory = people.iter().cloned().map(Entry::from_principal).collect();
                sort(&mut self.directory);
            }
            SyncEvent::ContactSaved { ticket, id } if self.pending == Some(*ticket) => {
                self.pending = None;
                let name = match &self.mode {
                    Mode::Edit(f) => f.text(cx).name,
                    _ => String::new(),
                };
                self.mode = Mode::View;
                self.selected = Some(id.clone());
                cx.emit(ContactsAction::Toast(Toast::Success, tr!("contacts-saved", name = name)));
            }
            SyncEvent::ContactDeleted { ticket, id } if self.pending == Some(*ticket) => {
                self.pending = None;
                let name = self.mine.iter().find(|e| &e.card.id == id).map(Entry::name).unwrap_or_default();
                self.mode = Mode::View;
                self.selected = None;
                cx.emit(ContactsAction::Toast(Toast::Success, tr!("contacts-deleted", name = name)));
            }
            SyncEvent::ContactFailed { ticket, error } => {
                let text = tr!("contacts-failed", error = error.clone());
                if *ticket == 0 {
                    self.loading = false;
                    self.error = Some(text);
                } else if self.pending == Some(*ticket) {
                    self.pending = None;
                    match &mut self.mode {
                        Mode::Edit(f) => f.error = Some(text.clone()),
                        _ => self.mode = Mode::View,
                    }
                    cx.emit(ContactsAction::Toast(Toast::Error, text));
                } else {
                    return;
                }
            }
            _ => return,
        }
        self.refilter(cx);
        if self.want_first && self.selected.is_none() && matches!(self.mode, Mode::View) {
            self.select_first(cx);
        }
    }

    fn sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let tab = |id: &'static str, text: String, source: Source, cx: &mut Context<Self>| {
            div()
                .id(id)
                .px_2()
                .py_0p5()
                .rounded_md()
                .cursor_pointer()
                .when(self.source == source, |d| d.bg(rgb(PICKED)).font_weight(FontWeight::SEMIBOLD))
                .when(self.source != source, |d| d.hover(|d| d.bg(rgb(BUTTON))))
                .child(text)
                .on_click(cx.listener(move |this, _, _, cx| this.set_source(source, cx)))
        };
        let note = if self.loading && self.entries().is_empty() {
            Some(tr!("contacts-loading"))
        } else if self.shown.is_empty() {
            Some(tr!(if self.entries().is_empty() { "contacts-empty" } else { "contacts-nothing" }))
        } else {
            None
        };
        let list = uniform_list(
            "contacts",
            self.shown.len(),
            cx.processor(|this, range: Range<usize>, _window, cx| {
                range
                    .filter_map(|i| {
                        let e = this.entries().get(*this.shown.get(i)?)?;
                        let selected = this.selected.as_deref() == Some(e.card.id.as_str());
                        Some(row(i, e, selected, cx))
                    })
                    .collect::<Vec<_>>()
            }),
        )
        .track_scroll(&self.scroll)
        .size_full();
        div()
            .w(px(LIST_W))
            .flex_none()
            .h_full()
            .flex()
            .flex_col()
            .border_r_1()
            .border_color(rgb(BORDER))
            .child(
                div()
                    .flex()
                    .gap_1()
                    .px_2()
                    .pt_2()
                    .child(tab("src-mine", tr!("contacts-mine", n = self.mine.len()), Source::Mine, cx))
                    .child(tab("src-dir", tr!("contacts-directory", n = self.directory.len()), Source::Directory, cx)),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .p_2()
                    .border_b_1()
                    .border_color(rgb(BORDER))
                    .child(div().flex_1().min_w_0().child(Input::new(&self.search)))
                    .child(
                        button("contact-new", tr!("contacts-new"), true)
                            .on_click(cx.listener(|this, _, window, cx| this.start_edit(None, window, cx))),
                    ),
            )
            .when_some(self.error.clone(), |d, e| d.child(div().px_3().py_1().text_color(rgb(ERROR)).child(e)))
            .when_some(note, |d, n| d.child(div().px_3().py_2().text_color(rgb(MUTED)).child(n)))
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .child(list)
                    .child(Scrollbar::vertical(&self.scroll).mode(ScrollbarMode::Always)),
            )
    }

    fn card_pane(&self, e: &Entry, cx: &mut Context<Self>) -> AnyElement {
        let c = &e.card;
        let mine = e.kind == Kind::Contact;
        let busy = self.pending.is_some();
        let field = |label: &str, values: Vec<AnyElement>| {
            div()
                .flex()
                .gap_2()
                .child(div().w(px(LABEL_W)).flex_none().text_color(rgb(MUTED)).child(tr!(label)))
                .child(div().flex_1().min_w_0().flex().flex_col().children(values))
        };
        let confirm = matches!(self.mode, Mode::Confirm).then(|| {
            let answer = |id: &'static str, text: String, yes: bool, cx: &mut Context<Self>| {
                div()
                    .id(id)
                    .px_3()
                    .py_0p5()
                    .rounded_md()
                    .cursor_pointer()
                    .when(yes, |d| d.bg(rgb(ERROR)).text_color(rgb(0xffffff)).hover(|d| d.bg(rgb(0xa52517))))
                    .when(!yes, |d| d.bg(rgb(BUTTON)).hover(|d| d.bg(rgb(0xe0e0e0))))
                    .child(text)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if yes {
                            this.delete(cx);
                        } else {
                            this.mode = Mode::View;
                            cx.notify();
                        }
                    }))
            };
            div()
                .flex()
                .items_center()
                .gap_2()
                .px_3()
                .py_1p5()
                .bg(rgb(0xfff4ce))
                .border_b_1()
                .border_color(rgb(BORDER))
                .child(div().flex_1().child(if busy {
                    tr!("contacts-deleting")
                } else {
                    tr!("contacts-delete-confirm", name = e.name())
                }))
                .when(!busy, |d| {
                    d.child(answer("delete-yes", tr!("contacts-yes"), true, cx)).child(answer(
                        "delete-no",
                        tr!("contacts-no"),
                        false,
                        cx,
                    ))
                })
        });
        let write = c.emails.first().map(|email| {
            let address = Address::new(&c.name, email);
            button("contact-write", tr!("contacts-write"), true)
                .on_click(cx.listener(move |_, _, _, cx| cx.emit(ContactsAction::Write(address.clone()))))
        });
        let actions = div().flex().gap_2().children(write).when(mine, |d| {
            let old = c.clone();
            d.child(
                button("contact-edit", tr!("contacts-edit"), false)
                    .on_click(cx.listener(move |this, _, window, cx| this.start_edit(Some(old.clone()), window, cx))),
            )
            .child(button("contact-delete", tr!("contacts-delete"), false).on_click(cx.listener(
                |this, _, _, cx| {
                    this.mode = Mode::Confirm;
                    cx.notify();
                },
            )))
        });
        let text = |s: &String| div().child(s.clone()).into_any_element();
        div()
            .flex()
            .flex_col()
            .size_full()
            .children(confirm)
            .child(
                div()
                    .id("contact-card")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .p_4()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .child(div().text_size(px(18.)).font_weight(FontWeight::SEMIBOLD).child(e.name()))
                            .child(div().text_color(rgb(MUTED)).child(e.kind.label())),
                    )
                    .child(actions)
                    .when(!c.emails.is_empty(), |d| {
                        let emails =
                            c.emails.iter().map(|m| div().text_color(rgb(ACCENT)).child(m.clone()).into_any_element());
                        d.child(field("contacts-emails", emails.collect()))
                    })
                    .when(!c.phones.is_empty(), |d| {
                        d.child(field("contacts-phones", c.phones.iter().map(text).collect()))
                    })
                    .when(!c.organization.is_empty(), |d| {
                        d.child(field("contacts-organization", vec![text(&c.organization)]))
                    })
                    .when(!c.title.is_empty(), |d| d.child(field("contacts-title", vec![text(&c.title)])))
                    .when(!mine, |d| d.child(div().text_xs().text_color(rgb(MUTED)).child(tr!("contacts-readonly")))),
            )
            .into_any_element()
    }

    fn form_pane(&self, f: &Form, cx: &mut Context<Self>) -> AnyElement {
        let busy = self.pending.is_some();
        let field = |label: &str, state: &Entity<InputState>| {
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(div().w(px(LABEL_W)).flex_none().text_color(rgb(MUTED)).child(tr!(label)))
                .child(div().flex_1().min_w_0().max_w(px(520.)).child(Input::new(state)))
        };
        let title = tr!(if f.old.is_some() { "contacts-form-edit" } else { "contacts-form-new" });
        div()
            .id("contact-form")
            .size_full()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap_2()
            .p_4()
            .child(div().text_size(px(18.)).font_weight(FontWeight::SEMIBOLD).child(title))
            .child(field("contacts-name", &f.name))
            .child(field("contacts-emails", &f.emails))
            .child(field("contacts-phones", &f.phones))
            .child(field("contacts-organization", &f.organization))
            .child(field("contacts-title", &f.title))
            .child(div().pl(px(LABEL_W + 8.)).text_xs().text_color(rgb(MUTED)).child(tr!("contacts-form-hint")))
            .when_some(f.error.clone(), |d, e| d.child(div().pl(px(LABEL_W + 8.)).text_color(rgb(ERROR)).child(e)))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .pl(px(LABEL_W + 8.))
                    .when(!busy, |d| {
                        d.child(
                            button("form-save", tr!("contacts-save"), true)
                                .on_click(cx.listener(|this, _, _, cx| this.save(cx))),
                        )
                    })
                    .when(busy, |d| d.child(div().text_color(rgb(MUTED)).child(tr!("contacts-saving"))))
                    .child(button("form-cancel", tr!("contacts-cancel"), false).on_click(cx.listener(
                        |this, _, _, cx| {
                            this.mode = Mode::View;
                            this.pending = None;
                            cx.notify();
                        },
                    ))),
            )
            .into_any_element()
    }
}

/// Строка списка: имя, ниже — адрес (если имя не он сам) или тип записи справочника, организация.
fn row(i: usize, e: &Entry, selected: bool, cx: &mut Context<ContactsView>) -> AnyElement {
    let name = e.name();
    let email = e.card.emails.first().filter(|m| !m.eq_ignore_ascii_case(&name)).cloned();
    let email = match (email, e.kind) {
        (None, Kind::Person | Kind::Room | Kind::Resource) => Some(e.kind.label()),
        (m, _) => m,
    };
    let second = match (email.as_ref(), e.card.organization.as_str()) {
        (Some(m), "") => m.clone(),
        (Some(m), org) => format!("{m} · {org}"),
        (None, org) => org.to_string(),
    };
    let id = e.card.id.clone();
    div()
        .id(("contact", i))
        .w_full()
        .h(px(ROW_H))
        .px_3()
        .flex()
        .flex_col()
        .justify_center()
        .border_b_1()
        .border_color(rgb(0xeeeeee))
        .cursor_pointer()
        .when(selected, |d| d.bg(rgb(PICKED)))
        .when(!selected, |d| d.hover(|d| d.bg(rgb(0xe8f1fb))))
        .child(div().truncate().font_weight(FontWeight::MEDIUM).child(name))
        .child(div().truncate().text_xs().text_color(rgb(MUTED)).child(second))
        .on_click(cx.listener(move |this, _, _, cx| this.select(id.clone(), cx)))
        .into_any_element()
}

impl Render for ContactsView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let pane = match &self.mode {
            Mode::Edit(f) => self.form_pane(f, cx),
            _ => match self.selected_entry() {
                Some(e) => self.card_pane(e, cx),
                None => div()
                    .size_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(rgb(MUTED))
                    .child(tr!("contacts-pick"))
                    .into_any_element(),
            },
        };
        div()
            .size_full()
            .flex()
            .min_w_0()
            .bg(rgb(0xffffff))
            .child(self.sidebar(cx))
            .child(div().flex_1().min_w_0().h_full().child(pane))
    }
}

impl crate::Mailer {
    /// Просьба раздела: команда синхронизации, окно письма или тост. Демо сюда правки не шлёт (они в окне).
    pub(crate) fn on_contacts(&mut self, action: &ContactsAction, cx: &mut Context<Self>) {
        let command = match action {
            ContactsAction::Load => Command::Contacts,
            ContactsAction::Save { card, old, ticket } => {
                Command::SaveContact { card: card.clone(), old: old.clone(), ticket: *ticket }
            }
            ContactsAction::Delete { id, ticket } => Command::DeleteContact { id: id.clone(), ticket: *ticket },
            ContactsAction::Write(address) => {
                self.open_compose(Draft { to: vec![address.clone()], ..Default::default() }, cx);
                return;
            }
            ContactsAction::Toast(level, text) => {
                self.toasts.update(cx, |t, cx| t.push(*level, text.clone(), cx));
                return;
            }
        };
        if !self.backend.send(command) {
            self.sync_stopped(cx);
            self.contacts.update(cx, |c, cx| c.not_sent(cx));
        }
    }
}

fn demo_mine() -> Vec<Entry> {
    let mut v: Vec<Entry> = postman_demo::contacts::mine()
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let card = ContactCard {
                id: format!("demo-contact-{i}"),
                name: c.name.into(),
                emails: c.emails.iter().map(|s| s.to_string()).collect(),
                phones: c.phones.iter().map(|s| s.to_string()).collect(),
                organization: c.organization.into(),
                title: c.title.into(),
            };
            Entry::new(card, Kind::Contact)
        })
        .collect();
    sort(&mut v);
    v
}

fn demo_directory() -> Vec<Entry> {
    let mut v: Vec<Entry> = postman_demo::contacts::directory()
        .iter()
        .enumerate()
        .map(|(i, &(name, email, kind))| {
            Entry::from_principal(Principal {
                id: format!("demo-principal-{i}"),
                name: name.into(),
                email: Some(email.into()),
                kind: kind.into(),
            })
        })
        .collect();
    sort(&mut v);
    v
}

/// Демо: личные контакты и справочник — в подсказки адресов окна письма и окна встречи.
pub(crate) fn add_demo(book: &mut AddressBook) {
    let mine = postman_demo::contacts::mine();
    book.add_contacts(mine.iter().flat_map(|c| c.emails.iter().map(|e| (c.name, *e))));
    book.add_directory(postman_demo::contacts::directory().iter().map(|&(n, e, _)| (n, e)));
}

#[cfg(test)]
mod tests {
    // Не `super::*`: из `gpui_kit::*` пришёл бы атрибут `test` gpui вместо встроенного.
    use super::{ContactCard, Entry, FormError, FormText, Kind, demo_directory, demo_mine, filter};

    fn names(entries: &[Entry], q: &str) -> Vec<String> {
        filter(entries, q).into_iter().map(|i| entries[i].card.name.clone()).collect()
    }

    #[test]
    fn search_by_name_address_phone_and_company() {
        let mine = demo_mine();
        assert_eq!(filter(&mine, "").len(), mine.len(), "пустой запрос — все");
        assert_eq!(names(&mine, "бел"), ["Белова Ольга"]);
        assert_eq!(names(&mine, "ольга бел"), ["Белова Ольга"], "слова в любом порядке");
        assert_eq!(names(&mine, "gusev"), ["Гусев Дмитрий"], "часть адреса");
        assert_eq!(names(&mine, "integrator.example"), ["Гусев Дмитрий"], "домен");
        assert_eq!(names(&mine, "495 123"), ["Белова Ольга"], "цифры телефона");
        assert_eq!(names(&mine, "+7 905"), ["Тарасов Олег"]);
        assert_eq!(names(&mine, "интегратор"), ["Гусев Дмитрий"], "организация");
        assert_eq!(names(&mine, "архитектор"), ["Гусев Дмитрий"], "должность");
        assert!(names(&mine, "сидоров").is_empty());
        let dir = demo_directory();
        assert_eq!(names(&dir, "федорова"), ["Фёдорова Наталья"], "ё = е");
        assert_eq!(names(&dir, "ПЕРЕГОВОРНАЯ 3"), ["Переговорная 3"]);
        let room = dir.iter().find(|e| e.card.name == "Переговорная 3").map(|e| e.kind);
        assert_eq!(room, Some(Kind::Room));
    }

    #[test]
    fn form_becomes_a_card_and_a_patch() {
        let old = ContactCard {
            id: "c1".into(),
            name: "Петров Алексей".into(),
            emails: vec!["a.petrov@x.ru".into()],
            phones: vec!["+7 495 000-00-01".into()],
            organization: "ООО Ромашка".into(),
            title: String::new(),
        };
        let mut text = FormText::of(&old);
        assert_eq!(text.card("c1"), Ok(old.clone()), "форма без правок — та же карточка");
        assert!(text.card("c1").unwrap().patch(&old).is_empty());

        text.emails = "a.petrov@x.ru, Алексей <a@home.ru>;".into();
        text.phones = " ".into();
        text.title = " Директор ".into();
        let card = text.card("c1").unwrap();
        assert_eq!(card.emails, ["a.petrov@x.ru", "a@home.ru"]);
        let patch = card.patch(&old);
        let mut keys: Vec<&str> = patch.keys().map(String::as_str).collect();
        keys.sort();
        assert_eq!(keys, ["emails", "phones", "titles"]);
        assert!(patch["phones"].is_null(), "убранный телефон — null");
        assert_eq!(patch["titles"]["t1"]["name"], "Директор");

        text.emails = "a.petrov@x.ru; petr@".into();
        assert_eq!(text.card("c1"), Err(FormError::BadEmail("petr@".into())));
        let empty = FormText { organization: "ООО".into(), ..Default::default() };
        assert_eq!(empty.card(""), Err(FormError::NeedName));
        let only_email = FormText { emails: "x@y.ru".into(), ..Default::default() };
        assert_eq!(only_email.card("").map(|c| c.emails), Ok(vec!["x@y.ru".to_string()]));
    }
}
