//! Вкладка «Настройки» в экране почты (S5-09, `settings_tab.rs`) и настройки ящика на сервере: автоответ (раздел
//! и полоса), заполненность ящика в строке состояния и тост при > 90 % (S4-05), правила почты (S4-07, `rules.rs`).
//! Сервер — команды `sync::Command::Settings` и `Command::Categories`, ответы — `sync::Event::Settings`.

use gpui_kit::prelude::*;
use gpui_kit::*;
use jiff::tz::TimeZone;
use pets_i18n::tr;
use pets_ui::Level as Toast;
use postman_core::categories;
use postman_core::labels::Categories;
use postman_core::rules::{Action, Field, Rule};
use postman_core::settings::{Area, Foreign, Reply, Request, RulesState, Usage, WARN_PERCENT};
use postman_core::sync::{Command, Event};
use postman_jmap::VacationResponse;

use crate::Mailer;
use crate::rules::{RulesEvent, RulesPanel};
use crate::settings_tab::{Info, Section, SettingsEvent, SettingsView};
use crate::signin::SignIn;
use crate::vacation::{self, BarEvent, VacationBar, VacationEvent, VacationPanel};

#[derive(Default)]
pub(crate) struct ServerSettings {
    /// Автоответ на сервере (`None` — ещё не пришёл или сервер его не знает).
    pub(crate) vacation: Option<VacationResponse>,
    pub(crate) quota: Option<Usage>,
    /// Правила на сервере (`None` — ещё не пришли или сервер их не знает).
    pub(crate) rules: Option<RulesState>,
    /// Тост о заполненности уже показан (снова — после того, как место освободилось).
    quota_warned: bool,
    /// Вкладка «Настройки» открыта.
    pub(crate) view: Option<Entity<SettingsView>>,
    /// Разделы «Автоответы» и «Правила» — панели, которые слушает экран почты.
    pub(crate) vacation_panel: Option<Entity<VacationPanel>>,
    pub(crate) rules_panel: Option<Entity<RulesPanel>>,
    /// Карточка «Добавить аккаунт» в разделе «Аккаунты».
    add: Option<Entity<SignIn>>,
    /// Полоса «Автоответ включён» — пока он включён.
    pub(crate) bar: Option<Entity<VacationBar>>,
    _subscriptions: Vec<Subscription>,
    /// Подписки вкладки: уходят вместе с ней.
    _tab: Vec<Subscription>,
}

impl ServerSettings {
    /// «Ящик: 12 % из 2 ГБ» для строки состояния; квоты нет — пусто.
    pub(crate) fn quota_text(&self) -> String {
        self.quota.map(quota_text).unwrap_or_default()
    }
}

/// Предел квоты: меньше гигабайта — в МБ (целые), иначе в ГБ с одним знаком.
pub(crate) fn limit_parts(bytes: u64) -> (&'static str, f64) {
    const MB: f64 = 1024.0 * 1024.0;
    let mb = bytes as f64 / MB;
    if mb < 1024.0 { ("mb", mb.round()) } else { ("gb", (mb / 1024.0 * 10.0).round() / 10.0) }
}

fn quota_text(u: Usage) -> String {
    let (unit, n) = limit_parts(u.limit);
    tr!("settings-quota", percent = u.percent(), unit = unit, n = n)
}

impl Mailer {
    /// Что вкладке нужно знать об экране: учётная запись, общие ящики, папки, адреса отправителя.
    fn settings_info(&self, cx: &App) -> Info {
        let folders = self.folders.read(cx);
        let mut identities = self.compose.sender_addresses();
        if identities.is_empty()
            && let Some(me) = self.reading.read(cx).me.clone()
        {
            identities.push(me);
        }
        Info {
            live: self.backend.is_live(),
            account: self.live_account().cloned(),
            status: self.live.status.clone(),
            shared: folders.accounts.iter().map(|a| a.name.clone()).collect(),
            folders: folders.of(None).map(|m| (m.id.clone(), m.name.clone())).collect(),
            identities,
            cache: self.account_cache(),
        }
    }

    /// Таблица категорий своей учётной записи (как в списке писем).
    fn own_categories(&self, cx: &App) -> Categories {
        self.list.read(cx).labels.for_account(None).cloned().unwrap_or_else(crate::labels::default_table)
    }

    /// `settings.open` (рейл, Ctrl+,): вкладка «Настройки» на разделе `section`; открыта — тот же раздел или новый.
    pub(crate) fn open_settings(&mut self, section: Option<Section>, window: &mut Window, cx: &mut Context<Self>) {
        self.tabs.open(crate::tabs::TabKind::Settings);
        let info = self.settings_info(cx);
        match self.settings.view.clone() {
            Some(view) => {
                view.update(cx, |v, cx| {
                    v.set_info(info, cx);
                    if let Some(s) = section {
                        v.select(s, window, cx);
                    }
                });
                // Панель раздела — сразу, не дожидаясь события вкладки.
                if let Some(s) = section {
                    self.section_opened(s, window, cx);
                }
            }
            None => {
                let cats = self.own_categories(cx);
                let section = section.unwrap_or(Section::General);
                let view = cx.new(|cx| SettingsView::new(section, info, cats, window, cx));
                let sub = cx.subscribe_in(&view, window, |this, _, ev: &SettingsEvent, window, cx| this.on_settings(ev, window, cx));
                self.settings._tab.push(sub);
                self.settings.view = Some(view);
                self.section_opened(section, window, cx);
            }
        }
        cx.notify();
    }

    /// `settings.vacation`: раздел «Автоответы».
    pub(crate) fn open_vacation(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_settings(Some(Section::Vacation), window, cx);
    }

    /// `settings.rules`: раздел «Правила»; живой ящик — правила с сервера заново.
    pub(crate) fn open_rules(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_settings(Some(Section::Rules), window, cx);
    }

    pub(crate) fn on_settings(&mut self, ev: &SettingsEvent, window: &mut Window, cx: &mut Context<Self>) {
        match ev {
            SettingsEvent::Opened(s) => self.section_opened(*s, window, cx),
            SettingsEvent::Categories(table) => self.edit_categories(table.clone(), cx),
            SettingsEvent::Purge(keyword) => {
                let request = categories::Request::Purge { account: None, keyword: keyword.clone(), ticket: 0 };
                if !self.backend.send(Command::Categories(request)) {
                    self.not_sent(cx);
                }
            }
            SettingsEvent::AddAccount => {
                let panel = cx.new(|cx| SignIn::new(None, false, window, cx));
                self.watch_signin(&panel, window, cx);
                self.settings.add = Some(panel.clone());
                if let Some(v) = &self.settings.view {
                    v.update(cx, |v, cx| {
                        v.add = Some(panel.into());
                        cx.notify();
                    });
                }
            }
            SettingsEvent::AccountPrefs(p) => self.compose.prefs = p.clone(),
            SettingsEvent::RemoveAccount => {
                self.close_settings(window, cx);
                self.remove_account(window, cx);
            }
        }
    }

    /// Раздел открыт: правилам и автоответам — их панели (правила — заново с сервера).
    fn section_opened(&mut self, section: Section, window: &mut Window, cx: &mut Context<Self>) {
        match section {
            Section::Vacation if self.settings.vacation_panel.is_none() => {
                let live = self.backend.is_live();
                let state = self.settings.vacation.clone();
                let warning = self.foreign_warning();
                let panel = cx.new(|cx| {
                    let mut p = VacationPanel::new(state.as_ref(), live, window, cx);
                    p.warning = warning;
                    p
                });
                let sub = cx.subscribe_in(&panel, window, |this, _, ev: &VacationEvent, _, cx| match ev {
                    VacationEvent::Save(v) => this.save_vacation(v.clone(), cx),
                });
                self.settings._tab.push(sub);
                self.settings.vacation_panel = Some(panel.clone());
                self.give_panel(section, panel.into(), cx);
            }
            Section::Rules if self.settings.rules_panel.is_none() => {
                let live = self.backend.is_live();
                // Правила (Sieve) — своего ящика: папки общих ящиков (S4-06) в них не выбрать.
                let mailboxes: Vec<(String, String)> =
                    self.folders.read(cx).of(None).map(|m| (m.id.clone(), m.name.clone())).collect();
                let state = if live { self.settings.rules.clone() } else { Some(rules_sample(&mailboxes)) };
                if live && !self.backend.send(Command::Settings(Request::Rules)) {
                    self.sync_stopped(cx);
                }
                // Категории своего ящика (S5-26): действие «Категория».
                let categories: Vec<crate::rules::CategoryChoice> = self
                    .list
                    .read(cx)
                    .labels
                    .for_account(None)
                    .cloned()
                    .unwrap_or_else(crate::labels::default_table)
                    .visible()
                    .map(|c| (c.keyword.clone(), c.name.clone(), c.shown_color()))
                    .collect();
                let panel = cx.new(|cx| RulesPanel::new(state, mailboxes, categories, live, window, cx));
                let sub = cx.subscribe_in(&panel, window, |this, _, ev: &RulesEvent, _, cx| match ev {
                    RulesEvent::Save { rules, replace } => this.save_rules(rules.clone(), *replace, cx),
                });
                self.settings._tab.push(sub);
                self.settings.rules_panel = Some(panel.clone());
                self.give_panel(section, panel.into(), cx);
            }
            _ => {}
        }
    }

    fn give_panel(&mut self, section: Section, panel: AnyView, cx: &mut Context<Self>) {
        let Some(v) = &self.settings.view else { return };
        v.update(cx, |v, cx| {
            match section {
                Section::Vacation => v.vacation = Some(panel),
                _ => v.rules = Some(panel),
            }
            cx.notify();
        });
    }

    /// Правка таблицы категорий в настройках: в список и локальную копию сразу, на сервер — сведением (FileNode).
    fn edit_categories(&mut self, table: Categories, cx: &mut Context<Self>) {
        let mut book = self.list.read(cx).labels.clone();
        book.edit(None, table.clone());
        self.save_labels(&book, cx);
        self.list.update(cx, |l, cx| l.set_labels(book, cx));
        if self.backend.is_live() && !self.backend.send(Command::Categories(categories::Request::Save { account: None, table })) {
            self.sync_stopped(cx);
        }
    }

    /// Таблица категорий изменилась (сервер): вкладке — новую.
    pub(crate) fn categories_changed(&mut self, cx: &mut Context<Self>) {
        let cats = self.own_categories(cx);
        if let Some(v) = &self.settings.view {
            v.update(cx, |v, cx| v.set_categories(cats, cx));
        }
    }

    /// Вход подтверждён сервером: карточка добавления в настройках больше не нужна.
    pub(crate) fn signed_in(&mut self, cx: &mut Context<Self>) {
        self.settings.add = None;
        let info = self.settings_info(cx);
        if let Some(v) = &self.settings.view {
            v.update(cx, |v, cx| {
                v.add = None;
                v.set_info(info, cx);
            });
        }
    }

    /// «Отмена» в карточке добавления.
    pub(crate) fn cancel_add_account(&mut self, cx: &mut Context<Self>) {
        let panel = self.settings.add.take();
        // Вход с этой карточки уже показан на месте экрана почты — убрать и там.
        if panel.is_some() && self.live.signin.as_ref().map(|p| p.entity_id()) == panel.as_ref().map(|p| p.entity_id()) {
            self.live.signin = None;
        }
        if let Some(v) = &self.settings.view {
            v.update(cx, |v, cx| {
                v.add = None;
                cx.notify();
            });
        }
        cx.notify();
    }

    /// Esc во вкладке: сначала — запись клавиши или правка на месте; `false` — закрывать вкладку.
    pub(crate) fn settings_escape(&mut self, cx: &mut Context<Self>) -> bool {
        match &self.settings.view {
            Some(v) if self.tabs.is_active(crate::tabs::TabKind::Settings) => v.update(cx, |v, cx| v.escape(cx)),
            _ => false,
        }
    }

    /// Закрыть вкладку «Настройки»; `false` — её не было (Esc идёт дальше).
    pub(crate) fn close_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let open = self.settings.view.take().is_some();
        self.settings.vacation_panel = None;
        self.settings.rules_panel = None;
        self.settings.add = None;
        self.settings._tab.clear();
        self.tabs.close_kind(crate::tabs::TabKind::Settings);
        if open {
            // Клавиши — снова экрану почты.
            window.focus(&self.focus, cx);
            cx.notify();
        }
        open
    }

    fn save_vacation(&mut self, v: VacationResponse, cx: &mut Context<Self>) {
        if self.backend.send(Command::Settings(Request::SetVacation(v))) {
            return;
        }
        self.not_sent(cx);
        if let Some(p) = &self.settings.vacation_panel {
            p.update(cx, |p, cx| p.not_saved(cx));
        }
    }

    fn save_rules(&mut self, rules: Vec<Rule>, replace: bool, cx: &mut Context<Self>) {
        if self.backend.send(Command::Settings(Request::SaveRules { rules, replace })) {
            return;
        }
        self.not_sent(cx);
        if let Some(p) = &self.settings.rules_panel {
            p.update(cx, |p, cx| p.not_saved(cx));
        }
    }

    /// Команда не ушла: демо — «нужен сервер», живой ящик — синхронизация остановилась.
    fn not_sent(&mut self, cx: &mut Context<Self>) {
        if self.backend.is_live() { self.sync_stopped(cx) } else { self.needs_server(cx) }
    }

    /// «Выключить» на полосе: тот же автоответ, выключенный.
    fn vacation_off(&mut self, cx: &mut Context<Self>) {
        let Some(v) = self.settings.vacation.clone() else { return };
        self.save_vacation(VacationResponse { is_enabled: false, ..v }, cx);
    }

    /// Активный скрипт сервера не из Postman: включение автоответа его выключит (`docs/STALWART.md`).
    fn foreign_warning(&self) -> Option<String> {
        let name = self.settings.rules.as_ref()?.active_foreign()?;
        Some(tr!("vacation-foreign-warning", name = name.to_owned()))
    }

    /// Событие синхронизации — своё берём (`Event::Settings`).
    pub(crate) fn on_settings_sync(&mut self, ev: &Event, cx: &mut Context<Self>) {
        let Event::Settings(reply) = ev else { return };
        let vacation = self.settings.vacation_panel.clone();
        let rules = self.settings.rules_panel.clone();
        match reply {
            Reply::Vacation(v) => {
                if let Some(p) = &vacation {
                    p.update(cx, |p, cx| p.server_state(v, cx));
                }
                self.set_vacation_state(Some(v.clone()), cx);
            }
            Reply::VacationSaved(v) => {
                match &vacation {
                    Some(p) => p.update(cx, |p, cx| p.saved(v, cx)),
                    None => {
                        let text = if v.is_enabled { tr!("vacation-saved") } else { tr!("vacation-saved-off") };
                        self.toasts.update(cx, |t, cx| t.push(Toast::Success, text, cx));
                    }
                }
                self.set_vacation_state(Some(v.clone()), cx);
            }
            Reply::Quota(usage) => self.set_quota(*usage, cx),
            Reply::Rules(s) => self.set_rules_state(s.clone(), false, cx),
            Reply::RulesSaved(s) => self.set_rules_state(s.clone(), true, cx),
            Reply::RulesInvalid(e) => {
                tracing::warn!("rules: server rejected the script: {e}");
                let text = tr!("rules-invalid", error = e.clone());
                self.rules_problem(text, cx);
            }
            Reply::RulesConflict { name, overwrite } => {
                let (name, overwrite) = (name.clone(), *overwrite);
                if let Some(p) = &rules {
                    p.update(cx, |p, cx| {
                        let pending = p.pending().cloned().unwrap_or_default();
                        p.conflict(name, overwrite, pending, cx)
                    });
                }
            }
            Reply::Unsupported(Area::Vacation) => {
                if let Some(p) = &vacation {
                    p.update(cx, |p, cx| p.failed(tr!("vacation-unsupported"), cx));
                }
            }
            Reply::Unsupported(Area::Rules) => {
                if let Some(p) = &rules {
                    p.update(cx, |p, cx| p.failed(tr!("rules-unsupported"), cx));
                }
            }
            Reply::Unsupported(Area::Quota) => {}
            Reply::Failed { area: Area::Vacation, error } => {
                tracing::warn!("vacation: {error}");
                let text = tr!("vacation-failed", error = error.clone());
                match &vacation {
                    Some(p) => p.update(cx, |p, cx| p.failed(text, cx)),
                    None => self.toasts.update(cx, |t, cx| t.push(Toast::Error, text, cx)),
                }
            }
            Reply::Failed { area: Area::Rules, error } => {
                tracing::warn!("rules: {error}");
                let text = tr!("rules-failed", error = error.clone());
                self.rules_problem(text, cx);
            }
            // Квота не прочиталась — строка состояния без неё; в журнал.
            Reply::Failed { area: Area::Quota, error } => tracing::warn!("quota: {error}"),
        }
    }

    fn set_rules_state(&mut self, state: RulesState, saved: bool, cx: &mut Context<Self>) {
        self.settings.rules = Some(state.clone());
        let warning = self.foreign_warning();
        if let Some(p) = &self.settings.vacation_panel {
            p.update(cx, |p, cx| {
                p.warning = warning;
                cx.notify();
            });
        }
        match &self.settings.rules_panel {
            Some(p) if saved => p.update(cx, |p, cx| p.saved(state, cx)),
            Some(p) => p.update(cx, |p, cx| p.server_state(state, cx)),
            None if saved => self.toasts.update(cx, |t, cx| t.push(Toast::Success, tr!("rules-saved"), cx)),
            None => {}
        }
    }

    /// Ошибка правил — в разделе «Правила» (редактор остаётся), без него — тостом.
    fn rules_problem(&mut self, text: String, cx: &mut Context<Self>) {
        match &self.settings.rules_panel {
            Some(p) => p.update(cx, |p, cx| p.failed(text, cx)),
            None => self.toasts.update(cx, |t, cx| t.push(Toast::Error, text, cx)),
        }
    }

    /// Автоответ на сервере изменился: полоса появляется, меняет текст или уходит.
    pub(crate) fn set_vacation_state(&mut self, v: Option<VacationResponse>, cx: &mut Context<Self>) {
        let text = v.as_ref().filter(|v| v.is_enabled).map(|v| vacation::bar_text(v, &TimeZone::system()));
        self.settings.vacation = v;
        match (text, &self.settings.bar) {
            (Some(text), Some(bar)) => bar.update(cx, |b, cx| {
                b.text = text;
                cx.notify();
            }),
            (Some(text), None) => {
                let bar = cx.new(|_| VacationBar { text });
                self.settings
                    ._subscriptions
                    .push(cx.subscribe(&bar, |this, _, BarEvent::TurnOff, cx| this.vacation_off(cx)));
                self.settings.bar = Some(bar);
            }
            (None, _) => self.settings.bar = None,
        }
        cx.notify();
    }

    pub(crate) fn set_quota(&mut self, usage: Option<Usage>, cx: &mut Context<Self>) {
        self.settings.quota = usage;
        let percent = usage.map_or(0, |u| u.percent());
        if percent > WARN_PERCENT && !self.settings.quota_warned {
            self.settings.quota_warned = true;
            let text = tr!("settings-quota-warn", percent = percent);
            self.toasts.update(cx, |t, cx| t.push(Toast::Warning, text, cx));
        } else if percent <= WARN_PERCENT {
            self.settings.quota_warned = false;
        }
        cx.notify();
    }
}

/// Образец для снимка `vacation` в демо: включён на неделю с сегодняшнего дня.
pub(crate) fn screen_sample() -> VacationResponse {
    let tz = TimeZone::system();
    let today = jiff::Zoned::now().with_time_zone(tz.clone()).date();
    let at = |d: jiff::civil::Date| d.to_zoned(tz.clone()).ok().map(|z| z.timestamp().to_string());
    VacationResponse {
        is_enabled: true,
        from_date: at(today),
        to_date: today.checked_add(jiff::Span::new().days(8)).ok().and_then(at),
        subject: Some("Нет на месте".into()), // i18n-ok: образец автоответа
        text_body: Some("Здравствуйте! Я в отпуске до конца недели.\nПо срочным вопросам — Иван Петров.".into()), // i18n-ok: образец
        html_body: None,
    }
}

/// Образец для демо и снимка `rules`: правила по папкам демо-ящика и чужой скрипт сервера.
pub(crate) fn rules_sample(mailboxes: &[(String, String)]) -> RulesState {
    let (mailbox_id, name) = mailboxes.get(4).or(mailboxes.last()).cloned().unwrap_or_default();
    let rule = |enabled, field, value: &str, action| Rule { enabled, field, value: value.into(), action };
    RulesState {
        rules: vec![
            rule(true, Field::Subject, "Тендер", Action::Category(postman_core::labels::category_keyword("tenders"))), // i18n-ok: образец правила (S5-26, снимок `rules` — его правка)
            rule(true, Field::Subject, "Отчёт", Action::Move { mailbox_id, name }), // i18n-ok: образец правила
            rule(true, Field::From, "ci@corp.example", Action::MarkRead),
            rule(true, Field::To, "director@corp.example", Action::Flag),
            rule(false, Field::Subject, "Розыгрыш", Action::Discard), // i18n-ok: образец правила
        ],
        active: true,
        foreign: vec![Foreign {
            name: "old-filters".into(),
            text: "require \"fileinto\";\nif address :is \"from\" \"news@example.com\" {\n    fileinto \"News\";\n}"
                .into(),
            active: false,
        }],
    }
}

#[cfg(test)]
mod tests {
    use super::limit_parts;

    #[test]
    fn quota_limit_units() {
        assert_eq!(limit_parts(500 * 1024 * 1024), ("mb", 500.0));
        assert_eq!(limit_parts(2 * 1024 * 1024 * 1024), ("gb", 2.0));
        assert_eq!(limit_parts(1536 * 1024 * 1024), ("gb", 1.5));
    }
}
