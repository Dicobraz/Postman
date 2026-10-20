//! Строка входа в окне (S3-04): адрес сервера, логин, пароль, «Войти». Не модальное окно — панель на месте
//! списка писем; вход проверяется синхронизацией, панель уходит после первого ответа сервера.

use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::*;
use gpui_kit::*;
use pets_i18n::tr;
use postman_core::sync::Account;
use postman_jmap::Auth;

use crate::account::Settings;

pub(crate) struct SignIn {
    url: Entity<InputState>,
    user: Entity<InputState>,
    password: Entity<InputState>,
    /// Вход идёт: ждём сервер.
    pub(crate) busy: bool,
    pub(crate) error: Option<String>,
    _subscriptions: Vec<Subscription>,
}

/// «Войти»: учётная запись из полей.
pub(crate) struct Submit(pub(crate) Account);

impl EventEmitter<Submit> for SignIn {}

impl SignIn {
    pub(crate) fn new(prefill: Option<Settings>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let prefill = prefill.unwrap_or_default();
        let url = cx.new(|cx| {
            InputState::new(window, cx).placeholder(tr!("signin-url-placeholder")).default_value(prefill.url.clone())
        });
        let user = cx.new(|cx| {
            InputState::new(window, cx).placeholder(tr!("signin-user-placeholder")).default_value(prefill.user.clone())
        });
        let password = cx.new(|cx| InputState::new(window, cx).masked(true));
        let enter =
            |this: &mut Self, _: &Entity<InputState>, ev: &InputEvent, _: &mut Window, cx: &mut Context<Self>| {
                if let InputEvent::PressEnter { .. } = ev {
                    this.submit(cx);
                }
            };
        let subscriptions = vec![
            cx.subscribe_in(&url, window, enter),
            cx.subscribe_in(&user, window, enter),
            cx.subscribe_in(&password, window, enter),
        ];
        // Адрес и логин уже есть — сразу пароль.
        let first = if prefill.url.is_empty() { &url } else { &password };
        first.update(cx, |s, cx| s.focus(window, cx));
        Self { url, user, password, busy: false, error: None, _subscriptions: subscriptions }
    }

    fn submit(&mut self, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let url = normalize_url(&self.url.read(cx).value());
        let user = self.user.read(cx).value().trim().to_owned();
        let password = self.password.read(cx).value().to_string();
        if url.is_empty() || user.is_empty() || password.is_empty() {
            self.error = Some(tr!("signin-fill"));
            cx.notify();
            return;
        }
        self.busy = true;
        self.error = None;
        cx.notify();
        cx.emit(Submit(Account { url, auth: Auth::Basic { user, password } }));
    }

    /// Сервер отклонил сохранённый вход: причина под полями, адрес и логин уже подставлены.
    pub(crate) fn rejected(&mut self, text: String, cx: &mut Context<Self>) {
        self.busy = false;
        self.error = Some(text);
        cx.notify();
    }

    /// Вход не удался: показать причину, поля остаются для новой попытки.
    pub(crate) fn failed(&mut self, error: &str, cx: &mut Context<Self>) {
        self.busy = false;
        self.error = Some(tr!("signin-failed", error = error.to_owned()));
        cx.notify();
    }
}

/// Адрес сервера: без пробелов и `/` в конце; без схемы — `https://`.
pub(crate) fn normalize_url(raw: &str) -> String {
    let url = raw.trim().trim_end_matches('/');
    if url.is_empty() || url.starts_with("http://") || url.starts_with("https://") {
        url.to_owned()
    } else {
        format!("https://{url}")
    }
}

impl Render for SignIn {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let row = |label: String, input: &Entity<InputState>| {
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(div().w(px(80.)).text_color(rgb(0x555555)).child(label))
                .child(div().flex_1().child(Input::new(input)))
        };
        let busy = self.busy;
        let card = div()
            .w(px(420.))
            .flex()
            .flex_col()
            .gap_3()
            .p_4()
            .rounded_lg()
            .border_1()
            .border_color(rgb(0xdddddd))
            .bg(rgb(0xffffff))
            .child(div().text_xl().font_weight(FontWeight::SEMIBOLD).child(tr!("signin-title")))
            .child(div().text_color(rgb(0x777777)).child(tr!("signin-hint")))
            .child(row(tr!("signin-url"), &self.url))
            .child(row(tr!("signin-user"), &self.user))
            .child(row(tr!("signin-password"), &self.password))
            .when_some(self.error.clone(), |d, e| d.child(div().text_color(rgb(0xc42b1c)).child(e)))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(
                        div()
                            .id("signin-submit")
                            .px_4()
                            .py_1()
                            .rounded_md()
                            .text_color(rgb(0xffffff))
                            .bg(if busy { rgb(0x8bb8e8) } else { rgb(0x0f6cbd) })
                            .when(!busy, |d| d.cursor_pointer().hover(|d| d.bg(rgb(0x115ea3))))
                            .child(if busy { tr!("signin-busy") } else { tr!("signin-submit") })
                            .on_click(cx.listener(|this, _, _, cx| this.submit(cx))),
                    )
                    .child(div().text_sm().text_color(rgb(0x888888)).child(tr!("signin-demo"))),
            );
        div().flex().flex_1().min_w_0().h_full().items_center().justify_center().child(card)
    }
}

#[cfg(test)]
mod tests {
    use super::normalize_url;

    #[test]
    fn server_url_is_normalized() {
        assert_eq!(normalize_url("  http://localhost:18080/ "), "http://localhost:18080");
        assert_eq!(normalize_url("mail.example.com"), "https://mail.example.com");
        assert_eq!(normalize_url("https://mail.example.com//"), "https://mail.example.com");
        assert_eq!(normalize_url("   "), "");
    }
}
