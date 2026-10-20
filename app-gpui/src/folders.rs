//! Папки и переключатель разделов (почта, календарь, контакты). Общие ящики (S4-06) — свёртываемые ветки
//! с именем аккаунта под своими папками.

use std::collections::HashSet;

use gpui_kit::*;
use gpui_kit::prelude::*;
use pets_i18n::tr;
use postman_cache::{AccountRow, MailboxRow, account_of};

use crate::mailer::*;

// --- папки ---

pub(crate) struct Folders {
    pub(crate) mailboxes: Vec<MailboxRow>,
    /// Общие ящики (S4-06): ветка на аккаунт.
    pub(crate) accounts: Vec<AccountRow>,
    /// Свёрнутые ветки общих ящиков (id аккаунта).
    pub(crate) collapsed: HashSet<String>,
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

impl Folders {
    pub(crate) fn new(mailboxes: Vec<MailboxRow>, accounts: Vec<AccountRow>, current: Option<String>) -> Self {
        Self { mailboxes, accounts, collapsed: HashSet::new(), current, results: None, section: Section::Mail }
    }

    /// Папки аккаунта (`None` — своего) в порядке сервера.
    pub(crate) fn of(&self, account: Option<&str>) -> impl Iterator<Item = &MailboxRow> {
        self.mailboxes.iter().filter(move |m| account_of(&m.id) == account)
    }

    /// Аккаунт только для чтения: так сказала сессия или ни в одной его папке ничего нельзя изменить
    /// (Stalwart ставит `isReadOnly: false` и при папке, открытой только на чтение, — права в `myRights`).
    pub(crate) fn read_only(&self, account: &AccountRow) -> bool {
        let mut folders = self.of(Some(&account.id)).peekable();
        account.read_only || (folders.peek().is_some() && folders.all(|m| m.rights.read_only()))
    }

    fn toggle(&mut self, account: &str, cx: &mut Context<Self>) {
        if !self.collapsed.remove(account) {
            self.collapsed.insert(account.to_owned());
        }
        cx.notify();
    }

    fn folder(&self, m: &MailboxRow, indent: bool, cx: &mut Context<Self>) -> AnyElement {
        let selected = self.results.is_none() && self.current.as_deref() == Some(&m.id);
        let id = m.id.clone();
        div()
            .id(SharedString::from(format!("mb-{}", m.id)))
            .px_2()
            .when(indent, |d| d.pl_5())
            .py_1()
            .rounded_md()
            .when(selected, |d| d.bg(rgb(0xcce4ff)))
            .hover(|d| d.bg(rgb(0xe8f1fb)))
            .cursor_pointer()
            .flex()
            .gap_2()
            .child(div().flex_1().truncate().child(m.name.clone()))
            // Непрочитанные — с сервера (`Mailbox.unreadEmails`), а не по загруженной части папки.
            .when(m.unread > 0, |d| unread(d, m.unread))
            .on_click(cx.listener(move |_, _, _, cx| cx.emit(FolderEvent::Open(id.clone()))))
            .into_any_element()
    }

    /// Заголовок ветки общего ящика: стрелка, имя; свёрнута — непрочитанные всего ящика.
    fn branch(&self, a: &AccountRow, cx: &mut Context<Self>) -> AnyElement {
        let open = !self.collapsed.contains(&a.id);
        let total: u32 = if open { 0 } else { self.of(Some(&a.id)).map(|m| m.unread).sum() };
        let id = a.id.clone();
        div()
            .id(SharedString::from(format!("acc-{}", a.id)))
            .mt_2()
            .px_1()
            .py_1()
            .rounded_md()
            .hover(|d| d.bg(rgb(0xe8f1fb)))
            .cursor_pointer()
            .flex()
            .items_center()
            .gap_1()
            .child(div().w(px(12.)).text_color(rgb(0x666666)).child(if open { "▾" } else { "▸" }))
            .child(div().flex_1().min_w_0().truncate().font_weight(FontWeight::SEMIBOLD).child(a.name.clone()))
            .when(self.read_only(a), |d| {
                d.child(div().flex_none().text_size(px(11.)).text_color(rgb(0x888888)).child(tr!("shared-read-only")))
            })
            .when(total > 0, |d| unread(d, total))
            .on_click(cx.listener(move |this, _, _, cx| this.toggle(&id, cx)))
            .into_any_element()
    }
}

fn unread(d: Stateful<Div>, n: u32) -> Stateful<Div> {
    d.child(div().font_weight(FontWeight::SEMIBOLD).text_color(rgb(0x0f6cbd)).child(n.to_string()))
}

impl Render for Folders {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut col = div().flex().flex_col().size_full().border_r_1().border_color(rgb(0xdddddd)).bg(rgb(0xf7f7f7));
        if let Some(n) = self.results {
            col = col.child(div().px_3().py_1().bg(rgb(0xcce4ff)).child(tr!("folders-results", n = n)));
        }
        let mut list = div().id("folders").flex().flex_col().flex_1().min_h_0().overflow_y_scroll().p_1();
        let own: Vec<AnyElement> = self.of(None).map(|m| self.folder(m, false, cx)).collect();
        list = list.children(own);
        for a in &self.accounts {
            list = list.child(self.branch(a, cx));
            if !self.collapsed.contains(&a.id) {
                let folders: Vec<AnyElement> = self.of(Some(&a.id)).map(|m| self.folder(m, true, cx)).collect();
                list = list.children(folders);
            }
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

#[cfg(test)]
mod tests {
    // Не `super::*`: из `gpui_kit::*` пришёл бы атрибут `test` gpui вместо встроенного.
    use super::Folders;
    use postman_cache::{AccountRow, MailboxRow, Rights};

    fn mb(id: &str, rights: Rights) -> MailboxRow {
        MailboxRow { id: id.into(), parent_id: None, name: id.into(), role: None, sort_order: 0, total: 0, unread: 0, rights }
    }

    fn acc(id: &str, read_only: bool) -> AccountRow {
        AccountRow { id: id.into(), name: id.into(), is_personal: false, read_only }
    }

    /// Ветки дерева (S4-06): папки по аккаунтам; «только чтение» — по сессии или по правам всех папок.
    #[test]
    fn folders_are_grouped_by_account() {
        let ro = Rights { read: true, add: false, remove: false, seen: false, keywords: false, submit: false };
        let f = Folders::new(
            vec![mb("in", Rights::ALL), mb("i/in", Rights::ALL), mb("j/in", ro), mb("sent", Rights::ALL)],
            vec![acc("i", false), acc("j", false), acc("k", true)],
            None,
        );
        let ids = |a: Option<&str>| f.of(a).map(|m| m.id.as_str()).collect::<Vec<_>>();
        assert_eq!(ids(None), ["in", "sent"]);
        assert_eq!(ids(Some("i")), ["i/in"]);
        assert!(!f.read_only(&acc("i", false)));
        assert!(f.read_only(&acc("j", false)), "все папки только для чтения");
        assert!(f.read_only(&acc("k", true)), "так сказала сессия");
        assert!(!f.read_only(&acc("k", false)), "папок ещё нет — не «только чтение»");
    }
}
