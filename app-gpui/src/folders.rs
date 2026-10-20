//! Папки и переключатель разделов (почта, календарь, контакты).

use gpui_kit::*;
use gpui_kit::prelude::*;
use pets_i18n::tr;
use postman_cache::MailboxRow;

use crate::mailer::*;

// --- папки ---

pub(crate) struct Folders {
    pub(crate) mailboxes: Vec<MailboxRow>,
    pub(crate) current: Option<String>,
    /// Число результатов, пока показан поиск.
    pub(crate) results: Option<usize>,
    pub(crate) section: Section,
}

pub(crate) enum FolderEvent {
    Open(String),
    Section(Section),
}

impl EventEmitter<FolderEvent> for Folders {}

impl Render for Folders {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut col = div().flex().flex_col().size_full().border_r_1().border_color(rgb(0xdddddd)).bg(rgb(0xf7f7f7));
        if let Some(n) = self.results {
            col = col.child(div().px_3().py_1().bg(rgb(0xcce4ff)).child(tr!("folders-results", n = n)));
        }
        let mut list = div().flex().flex_col().flex_1().p_1();
        for m in &self.mailboxes {
            let selected = self.results.is_none() && self.current.as_deref() == Some(&m.id);
            let id = m.id.clone();
            list = list.child(
                div()
                    .id(SharedString::from(format!("mb-{}", m.id)))
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .when(selected, |d| d.bg(rgb(0xcce4ff)))
                    .hover(|d| d.bg(rgb(0xe8f1fb)))
                    .cursor_pointer()
                    .child(m.name.clone())
                    .on_click(cx.listener(move |_, _, _, cx| cx.emit(FolderEvent::Open(id.clone())))),
            );
        }
        col.child(list).child(
            div()
                .flex()
                .flex_wrap()
                .gap_2()
                .p_2()
                .border_t_1()
                .border_color(rgb(0xdddddd))
                .child(nav("nav-mail", tr!("nav-mail"), self.section == Section::Mail, Section::Mail, cx))
                .child(nav("nav-cal", tr!("nav-calendar"), self.section == Section::Calendar, Section::Calendar, cx))
                .child(nav("nav-contacts", tr!("nav-contacts"), self.section == Section::Contacts, Section::Contacts, cx)),
        )
    }
}

pub(crate) fn nav(id: &'static str, label: String, active: bool, section: Section, cx: &mut Context<Folders>) -> impl IntoElement {
    div()
        .id(id)
        .px_1()
        .rounded_md()
        .cursor_pointer()
        .when(active, |d| d.bg(rgb(0xcce4ff)))
        .child(label)
        .on_click(cx.listener(move |_, _, _, cx| cx.emit(FolderEvent::Section(section))))
}
