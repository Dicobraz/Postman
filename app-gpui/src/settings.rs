//! Настройки ящика на сервере в экране почты (S4-05): автоответ (панель и полоса), заполненность ящика в строке
//! состояния и тост при > 90 %. Сервер — команды `sync::Command::Settings`, ответы — `sync::Event::Settings`.

use gpui_kit::prelude::*;
use gpui_kit::*;
use jiff::tz::TimeZone;
use pets_i18n::tr;
use pets_ui::Level as Toast;
use postman_core::settings::{Area, Reply, Request, Usage, WARN_PERCENT};
use postman_core::sync::{Command, Event};
use postman_jmap::VacationResponse;

use crate::Mailer;
use crate::vacation::{self, BarEvent, VacationBar, VacationEvent, VacationPanel};

/// Панель настроек на месте содержимого окна.
pub(crate) enum Panel {
    Vacation(Entity<VacationPanel>),
}

impl Panel {
    pub(crate) fn view(&self) -> AnyView {
        match self {
            Panel::Vacation(p) => p.clone().into(),
        }
    }
}

#[derive(Default)]
pub(crate) struct ServerSettings {
    /// Автоответ на сервере (`None` — ещё не пришёл или сервер его не знает).
    pub(crate) vacation: Option<VacationResponse>,
    pub(crate) quota: Option<Usage>,
    /// Тост о заполненности уже показан (снова — после того, как место освободилось).
    quota_warned: bool,
    pub(crate) panel: Option<Panel>,
    /// Полоса «Автоответ включён» — пока он включён.
    pub(crate) bar: Option<Entity<VacationBar>>,
    _subscriptions: Vec<Subscription>,
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
    /// `settings.vacation`: панель «Автоматические ответы» (повторно — та же панель).
    pub(crate) fn open_vacation(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if matches!(self.settings.panel, Some(Panel::Vacation(_))) {
            return;
        }
        let live = self.backend.is_live();
        let state = self.settings.vacation.clone();
        let panel = cx.new(|cx| VacationPanel::new(state.as_ref(), live, window, cx));
        let sub = cx.subscribe_in(&panel, window, |this, _, ev: &VacationEvent, window, cx| match ev {
            VacationEvent::Save(v) => this.save_vacation(v.clone(), cx),
            VacationEvent::Close => {
                this.close_settings(window, cx);
            }
        });
        self.settings._subscriptions.push(sub);
        self.settings.panel = Some(Panel::Vacation(panel));
        cx.notify();
    }

    /// Закрыть панель настроек; `false` — её не было (Esc идёт дальше).
    pub(crate) fn close_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let open = self.settings.panel.take().is_some();
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
        if self.backend.is_live() {
            self.sync_stopped(cx);
        } else {
            self.needs_server(cx);
        }
        if let Some(Panel::Vacation(p)) = &self.settings.panel {
            p.update(cx, |p, cx| p.not_saved(cx));
        }
    }

    /// «Выключить» на полосе: тот же автоответ, выключенный.
    fn vacation_off(&mut self, cx: &mut Context<Self>) {
        let Some(v) = self.settings.vacation.clone() else { return };
        self.save_vacation(VacationResponse { is_enabled: false, ..v }, cx);
    }

    /// Событие синхронизации — своё берём (`Event::Settings`).
    pub(crate) fn on_settings_sync(&mut self, ev: &Event, cx: &mut Context<Self>) {
        let Event::Settings(reply) = ev else { return };
        match reply {
            Reply::Vacation(v) => {
                if let Some(Panel::Vacation(p)) = &self.settings.panel {
                    p.update(cx, |p, cx| p.server_state(v, cx));
                }
                self.set_vacation_state(Some(v.clone()), cx);
            }
            Reply::VacationSaved(v) => {
                if let Some(Panel::Vacation(p)) = &self.settings.panel {
                    p.update(cx, |p, cx| p.saved(v, cx));
                } else {
                    let text = if v.is_enabled { tr!("vacation-saved") } else { tr!("vacation-saved-off") };
                    self.toasts.update(cx, |t, cx| t.push(Toast::Success, text, cx));
                }
                self.set_vacation_state(Some(v.clone()), cx);
            }
            Reply::Quota(usage) => self.set_quota(*usage, cx),
            Reply::Unsupported(Area::Vacation) => {
                if let Some(Panel::Vacation(p)) = &self.settings.panel {
                    p.update(cx, |p, cx| p.failed(tr!("vacation-unsupported"), cx));
                }
            }
            Reply::Unsupported(Area::Quota) => {}
            Reply::Failed { area: Area::Vacation, error } => {
                tracing::warn!("vacation: {error}");
                let text = tr!("vacation-failed", error = error.clone());
                match &self.settings.panel {
                    Some(Panel::Vacation(p)) => p.update(cx, |p, cx| p.failed(text, cx)),
                    _ => self.toasts.update(cx, |t, cx| t.push(Toast::Error, text, cx)),
                }
            }
            // Квота не прочиталась — строка состояния без неё; в журнал.
            Reply::Failed { area: Area::Quota, error } => tracing::warn!("quota: {error}"),
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
