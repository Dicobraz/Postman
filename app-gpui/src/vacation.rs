//! «Автоматические ответы» (S4-05), как в Outlook: вкл/выкл, период, тема, текст — панель на месте содержимого
//! окна (не модальное окно); полоса «Автоответ включён… Выключить», пока он включён. Сам ответ шлёт сервер
//! (`VacationResponse`), и когда Postman закрыт.
//!
//! Даты в полях — местное время `ДД.ММ.ГГГГ` или `ДД.ММ.ГГГГ ЧЧ:ММ`; «по» без времени — включая этот день.

use gpui_kit::component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::prelude::*;
use gpui_kit::*;
use jiff::Timestamp;
use jiff::civil::{Date, DateTime};
use jiff::tz::TimeZone;
use pets_i18n::tr;
use postman_jmap::VacationResponse;

/// Высота полосы «Автоответ включён»: кэшированное view не измеряется по содержимому.
pub(crate) const BAR_HEIGHT: f32 = 32.0;

/// Поля панели как текст (чистая модель — тестируется без окна).
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Form {
    pub(crate) enabled: bool,
    pub(crate) period: bool,
    pub(crate) from: String,
    pub(crate) to: String,
    pub(crate) subject: String,
    pub(crate) body: String,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum FormError {
    From,
    To,
    Order,
}

impl FormError {
    fn text(self) -> String {
        match self {
            FormError::From => tr!("vacation-bad-from"),
            FormError::To => tr!("vacation-bad-to"),
            FormError::Order => tr!("vacation-bad-order"),
        }
    }
}

/// Дата и время из поля: пусто — `Ok(None)`; дата без времени — начало дня (`end` — начало следующего).
pub(crate) fn parse_local(text: &str, end: bool, tz: &TimeZone) -> Result<Option<Timestamp>, ()> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(None);
    }
    let civil = match DateTime::strptime("%d.%m.%Y %H:%M", text) {
        Ok(dt) => dt,
        Err(_) => {
            let d = Date::strptime("%d.%m.%Y", text).map_err(|_| ())?;
            let d = if end { d.tomorrow().map_err(|_| ())? } else { d };
            d.to_datetime(jiff::civil::Time::midnight())
        }
    };
    civil.to_zoned(tz.clone()).map(|z| Some(z.timestamp())).map_err(|_| ())
}

/// UTCDate сервера → поле: полночь — только дата («по» — предыдущий день, как его и вводили).
pub(crate) fn format_local(utc: &str, end: bool, tz: &TimeZone) -> String {
    let Ok(ts) = utc.parse::<Timestamp>() else { return String::new() };
    let z = ts.to_zoned(tz.clone());
    if z.time() == jiff::civil::Time::midnight() {
        let d = if end { z.date().yesterday().unwrap_or(z.date()) } else { z.date() };
        d.strftime("%d.%m.%Y").to_string()
    } else {
        z.strftime("%d.%m.%Y %H:%M").to_string()
    }
}

pub(crate) fn to_response(f: &Form, tz: &TimeZone) -> Result<VacationResponse, FormError> {
    let (from, to) = if f.period {
        let from = parse_local(&f.from, false, tz).map_err(|_| FormError::From)?;
        let to = parse_local(&f.to, true, tz).map_err(|_| FormError::To)?;
        if let (Some(a), Some(b)) = (from, to)
            && b <= a
        {
            return Err(FormError::Order);
        }
        (from, to)
    } else {
        (None, None)
    };
    let text = |s: &str| Some(s.trim().to_owned()).filter(|s| !s.is_empty());
    Ok(VacationResponse {
        is_enabled: f.enabled,
        from_date: from.map(|t| t.to_string()),
        to_date: to.map(|t| t.to_string()),
        subject: text(&f.subject),
        text_body: Some(f.body.trim_end().to_owned()).filter(|s| !s.is_empty()),
        html_body: None,
    })
}

pub(crate) fn from_response(v: &VacationResponse, tz: &TimeZone) -> Form {
    let from = v.from_date.as_deref().map(|d| format_local(d, false, tz)).unwrap_or_default();
    let to = v.to_date.as_deref().map(|d| format_local(d, true, tz)).unwrap_or_default();
    Form {
        enabled: v.is_enabled,
        period: v.from_date.is_some() || v.to_date.is_some(),
        from,
        to,
        subject: v.subject.clone().unwrap_or_default(),
        body: v.text_body.clone().unwrap_or_default(),
    }
}

/// Текст полосы «Автоответ включён»: с периодом, если он задан.
pub(crate) fn bar_text(v: &VacationResponse, tz: &TimeZone) -> String {
    let f = from_response(v, tz);
    match (f.from.is_empty(), f.to.is_empty()) {
        (false, false) => tr!("vacation-bar-period", from = f.from, to = f.to),
        (false, true) => tr!("vacation-bar-from", from = f.from),
        (true, false) => tr!("vacation-bar-to", to = f.to),
        (true, true) => tr!("vacation-bar-on"),
    }
}

/// Что панель просит у экрана.
pub(crate) enum VacationEvent {
    Save(VacationResponse),
}

impl EventEmitter<VacationEvent> for VacationPanel {}

pub(crate) struct VacationPanel {
    form: Form,
    from: Entity<InputState>,
    to: Entity<InputState>,
    subject: Entity<InputState>,
    body: Entity<TextareaState>,
    /// Поля заново из `form` при следующей отрисовке (пришло состояние сервера, а `Window` нет).
    sync_inputs: bool,
    /// Пользователь правил поля: позднее состояние сервера их не затирает.
    touched: bool,
    /// Живой ящик; в демо сохранить нельзя.
    live: bool,
    /// Состояние сервера ещё не пришло.
    pub(crate) loading: bool,
    pub(crate) busy: bool,
    error: Option<String>,
    note: Option<String>,
    /// Предупреждение под переключателем (S4-07: другой фильтр сервера будет выключен).
    pub(crate) warning: Option<String>,
    _subscriptions: Vec<Subscription>,
}

impl VacationPanel {
    pub(crate) fn new(
        state: Option<&VacationResponse>,
        live: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let tz = TimeZone::system();
        let form = state.map(|v| from_response(v, &tz)).unwrap_or_default();
        let mut line = |value: &str, placeholder: String, cx: &mut Context<Self>| {
            let value = value.to_owned();
            cx.new(|cx| InputState::new(window, cx).placeholder(placeholder).default_value(value))
        };
        let from = line(&form.from, tr!("vacation-date-placeholder"), cx);
        let to = line(&form.to, tr!("vacation-date-placeholder"), cx);
        let subject = line(&form.subject, tr!("vacation-subject-placeholder"), cx);
        let body_text = form.body.clone();
        let body = cx.new(|cx| {
            TextareaState::new(window, cx).placeholder(tr!("vacation-body-placeholder")).default_value(body_text)
        });
        let touch = |this: &mut Self, ev: &InputEvent, cx: &mut Context<Self>| {
            if let InputEvent::Change = ev {
                this.touched = true;
                this.note = None;
                cx.notify();
            }
        };
        let subscriptions = vec![
            cx.subscribe_in(&from, window, move |this, _, ev: &InputEvent, _, cx| touch(this, ev, cx)),
            cx.subscribe_in(&to, window, move |this, _, ev: &InputEvent, _, cx| touch(this, ev, cx)),
            cx.subscribe_in(&subject, window, move |this, _, ev: &InputEvent, _, cx| touch(this, ev, cx)),
            cx.subscribe_in(&body, window, move |this, _, ev: &InputEvent, _, cx| touch(this, ev, cx)),
        ];
        Self {
            form,
            from,
            to,
            subject,
            body,
            sync_inputs: false,
            touched: false,
            live,
            loading: live && state.is_none(),
            busy: false,
            error: None,
            note: None,
            warning: None,
            _subscriptions: subscriptions,
        }
    }

    /// Состояние сервера: поля заново, если пользователь их ещё не правил.
    pub(crate) fn server_state(&mut self, v: &VacationResponse, cx: &mut Context<Self>) {
        self.loading = false;
        if !self.touched && !self.busy {
            self.form = from_response(v, &TimeZone::system());
            self.sync_inputs = true;
        }
        cx.notify();
    }

    /// Сервер сохранил: поля — как на сервере, строка «Сохранено».
    pub(crate) fn saved(&mut self, v: &VacationResponse, cx: &mut Context<Self>) {
        self.busy = false;
        self.touched = false;
        self.error = None;
        self.form = from_response(v, &TimeZone::system());
        self.sync_inputs = true;
        self.note = Some(if v.is_enabled { tr!("vacation-saved") } else { tr!("vacation-saved-off") });
        cx.notify();
    }

    pub(crate) fn failed(&mut self, error: String, cx: &mut Context<Self>) {
        self.busy = false;
        self.loading = false;
        self.error = Some(error);
        cx.notify();
    }

    /// Демо: сохранить нельзя — сказать в панели (экран говорит тостом).
    pub(crate) fn not_saved(&mut self, cx: &mut Context<Self>) {
        self.busy = false;
        cx.notify();
    }

    fn read_inputs(&mut self, cx: &App) {
        self.form.from = self.from.read(cx).value().to_string();
        self.form.to = self.to.read(cx).value().to_string();
        self.form.subject = self.subject.read(cx).value().to_string();
        self.form.body = self.body.read(cx).value().to_string();
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        if self.busy || self.loading {
            return;
        }
        self.read_inputs(cx);
        match to_response(&self.form, &TimeZone::system()) {
            Ok(v) => {
                self.error = None;
                self.note = None;
                self.busy = self.live;
                cx.emit(VacationEvent::Save(v));
            }
            Err(e) => self.error = Some(e.text()),
        }
        cx.notify();
    }

    fn toggle(&mut self, period: bool, cx: &mut Context<Self>) {
        if period {
            self.form.period = !self.form.period;
        } else {
            self.form.enabled = !self.form.enabled;
        }
        self.touched = true;
        self.note = None;
        cx.notify();
    }
}

/// Квадратик флажка: включён — залит акцентом с галочкой.
pub(crate) fn checkbox(on: bool) -> Div {
    let l = crate::roles::now();
    div()
        .size(px(16.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded_sm()
        .border_1()
        .border_color(if on { l.accent() } else { l.c(pets_ui::theme::Role::StrokeStrong) })
        .when(on, |d| d.bg(l.accent()).text_color(l.on_accent()).text_size(px(11.)).child("✓"))
}

/// Флажок с подписью, нажатие — переключить.
fn check(id: &'static str, on: bool, label: String, enabled: bool) -> Stateful<Div> {
    let l = crate::roles::now();
    div()
        .id(id)
        .flex()
        .items_center()
        .gap_2()
        .when(enabled, |d| d.cursor_pointer())
        .when(!enabled, |d| d.text_color(l.text2()))
        .child(checkbox(on))
        .child(label)
}

use crate::buttons::plain as button;

impl Render for VacationPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let l = crate::roles::look(cx);
        if std::mem::take(&mut self.sync_inputs) {
            let f = self.form.clone();
            for (state, value) in [(&self.from, f.from), (&self.to, f.to), (&self.subject, f.subject)] {
                state.update(cx, |s, cx| s.set_value(value, window, cx));
            }
            self.body.update(cx, |s, cx| s.set_value(f.body, window, cx));
        }
        let (enabled, period) = (self.form.enabled, self.form.period);
        let label = |text: String| div().w(px(110.)).flex_none().text_color(l.text2()).child(text);
        let dates = div()
            .flex()
            .items_center()
            .gap_2()
            .pl_6()
            .child(div().text_color(l.text2()).child(tr!("vacation-from")))
            .child(div().w(px(170.)).child(Input::new(&self.from)))
            .child(div().text_color(l.text2()).child(tr!("vacation-to")))
            .child(div().w(px(170.)).child(Input::new(&self.to)));
        let busy = self.busy || self.loading;
        // Раздел вкладки «Настройки» (S5-09): заголовок и пояснение — у вкладки, здесь — поля.
        let card = div()
            .w_full()
            .max_w(px(720.))
            .flex()
            .flex_col()
            .gap_3()
            .when(!self.live, |d| d.child(div().text_color(l.text2()).child(tr!("vacation-demo"))))
            .when(self.loading, |d| d.child(div().text_color(l.text2()).child(tr!("vacation-loading"))))
            .child(
                check("vacation-enabled", enabled, tr!("vacation-enabled"), true)
                    .on_click(cx.listener(|this, _, _, cx| this.toggle(false, cx))),
            )
            .when_some(self.warning.clone(), |d, w| d.child(div().pl_6().text_color(l.warning()).child(w)))
            .child(
                check("vacation-period", period, tr!("vacation-period"), enabled)
                    .on_click(cx.listener(|this, _, _, cx| this.toggle(true, cx))),
            )
            .when(period, |d| d.child(dates))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(label(tr!("vacation-subject")))
                    .child(div().flex_1().child(Input::new(&self.subject))),
            )
            .child(div().text_color(l.text2()).child(tr!("vacation-body")))
            .child(div().h(px(180.)).child(Textarea::new(&self.body).h(relative(1.))))
            .when_some(self.error.clone(), |d, e| d.child(div().text_color(l.error()).child(e)))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(
                        button(
                            "vacation-save",
                            if self.busy { tr!("vacation-saving") } else { tr!("vacation-save") },
                            !busy,
                        )
                        .on_click(cx.listener(|this, _, _, cx| this.save(cx))),
                    )
                    .when_some(self.note.clone(), |d, n| d.child(div().text_color(l.success()).child(n))),
            );
        div()
            .id("vacation-panel")
            .flex()
            .items_start()
            .flex_1()
            .min_w_0()
            .h_full()
            .overflow_y_scroll()
            .px_6()
            .py_4()
            .child(card)
    }
}

/// Полоса «Автоответ включён» под строкой команд.
pub(crate) struct VacationBar {
    pub(crate) text: String,
}

/// «Выключить»; «Настроить» — команда `settings.vacation`, как из строки команд.
pub(crate) enum BarEvent {
    TurnOff,
}

impl EventEmitter<BarEvent> for VacationBar {}

impl Render for VacationBar {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let l = crate::roles::look(cx);
        let link = |id: &'static str, text: String| {
            div()
                .id(id)
                .px_2()
                .rounded_md()
                .cursor_pointer()
                .text_color(l.accent_text())
                .hover(|d| d.bg(l.warning_bg()))
                .child(text)
        };
        div()
            .flex()
            .size_full()
            .items_center()
            .gap_2()
            .px_3()
            .bg(l.warning_bg())
            .border_b_1()
            .border_color(l.c(pets_ui::theme::Role::StrokeCard))
            .child(div().child("⚠"))
            .child(div().flex_1().min_w_0().child(self.text.clone()))
            .child(
                link("vacation-bar-edit", tr!("vacation-bar-edit")).on_click(|_, window, cx| {
                    window.dispatch_action(Box::new(crate::commands::settings::Vacation), cx)
                }),
            )
            .child(
                link("vacation-bar-off", tr!("vacation-bar-off"))
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(BarEvent::TurnOff))),
            )
    }
}

#[cfg(test)]
mod tests {
    // Не `super::*`: из `gpui_kit::*` пришёл бы атрибут `test` gpui вместо встроенного.
    use super::{Form, FormError, TimeZone, format_local, from_response, parse_local, to_response};

    fn moscow() -> TimeZone {
        TimeZone::get("Europe/Moscow").unwrap()
    }

    #[test]
    fn dates_are_local_and_the_end_day_is_included() {
        let tz = moscow();
        let t = |s: &str, end| parse_local(s, end, &tz).unwrap().map(|t| t.to_string());
        assert_eq!(t("", false), None);
        assert_eq!(t("20.10.2026", false).as_deref(), Some("2026-10-19T21:00:00Z"));
        assert_eq!(t("27.10.2026", true).as_deref(), Some("2026-10-27T21:00:00Z"));
        assert_eq!(t(" 27.10.2026 18:30 ", true).as_deref(), Some("2026-10-27T15:30:00Z"));
        assert!(parse_local("27/10/2026", false, &tz).is_err());
        assert!(parse_local("32.10.2026", false, &tz).is_err());
        assert_eq!(format_local("2026-10-19T21:00:00Z", false, &tz), "20.10.2026");
        assert_eq!(format_local("2026-10-27T21:00:00Z", true, &tz), "27.10.2026");
        assert_eq!(format_local("2026-10-27T15:30:00Z", true, &tz), "27.10.2026 18:30");
        assert_eq!(format_local("not a date", true, &tz), "");
    }

    #[test]
    fn form_round_trip() {
        let tz = moscow();
        let form = Form {
            enabled: true,
            period: true,
            from: "20.10.2026".into(),
            to: "27.10.2026 18:30".into(),
            subject: " Нет на месте ".into(),
            body: "В отпуске.\n\n".into(),
        };
        let v = to_response(&form, &tz).unwrap();
        assert_eq!(v.from_date.as_deref(), Some("2026-10-19T21:00:00Z"));
        assert_eq!(v.subject.as_deref(), Some("Нет на месте"));
        assert_eq!(v.text_body.as_deref(), Some("В отпуске."));
        let back = from_response(&v, &tz);
        assert_eq!(back, Form { subject: "Нет на месте".into(), body: "В отпуске.".into(), ..form.clone() });
        // Без периода даты не уходят; пустые тема и текст — `null`.
        let v = to_response(&Form { period: false, subject: " ".into(), body: String::new(), ..form.clone() }, &tz)
            .unwrap();
        assert_eq!((v.from_date, v.to_date, v.subject, v.text_body), (None, None, None, None));
        assert_eq!(to_response(&Form { from: "завтра".into(), ..form.clone() }, &tz), Err(FormError::From));
        assert_eq!(to_response(&Form { to: "1.1".into(), ..form.clone() }, &tz), Err(FormError::To));
        assert_eq!(to_response(&Form { to: "19.10.2026".into(), ..form }, &tz), Err(FormError::Order));
    }
}
