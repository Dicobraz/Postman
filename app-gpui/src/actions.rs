//! Ответ, пересылка и удаление (S3-02): команды `mail.reply`, `mail.reply_all`, `mail.forward`, `mail.delete`.
//!
//! Ответ — в области чтения над письмом (S5-06б) с черновиком из `postman_compose::respond`. Цитате нужно тело
//! письма, разобранное воркером:
//! его готовит область чтения (кэш или `Command::Body`). Тело ещё не готово — ждём область чтения (`observe`), поток
//! интерфейса не блокируется. Вложения оригинала при пересылке — по `blob_id`: в демо байты сразу, в живом ящике
//! их скачивает синхронизация перед отправкой.
//!
//! Удаление — в папку с ролью `trash`: сначала кэш (в фоне), затем `Command::Move` серверу; из «Удалённых» —
//! насовсем (`Command::Destroy`) после строки «Удалить навсегда? Да / Нет» над списком.

use std::collections::HashSet;

use gpui_kit::*;
use pets_i18n::tr;
use pets_ui::Level as Toast;
use postman_cache::{Addr, BodyRow, EmailRow, MailboxRow};
use postman_compose::{Address, Attachment, Draft, Mode, Original};
use postman_core::sync::Command;
use postman_ir::Document;

use crate::Mailer;
use crate::commands::mail;
use crate::list::{MailList, Purge};

/// Ответ ждёт тело письма.
struct Waiting {
    mode: Mode,
    id: String,
    _reading: Subscription,
}

/// Состояние команд письма у экрана.
#[derive(Default)]
pub(crate) struct Actions {
    waiting: Option<Waiting>,
}

/// Обработчики команд — на корне экрана почты (`Mailer::render`).
pub(crate) fn handlers(root: Div, cx: &mut Context<Mailer>) -> Div {
    root.on_action(cx.listener(|this, _: &mail::Reply, _, cx| this.respond(Mode::Reply, cx)))
        .on_action(cx.listener(|this, _: &mail::ReplyAll, _, cx| this.respond(Mode::ReplyAll, cx)))
        .on_action(cx.listener(|this, _: &mail::Forward, _, cx| this.respond(Mode::Forward, cx)))
        .on_action(cx.listener(|this, _: &mail::Delete, _, cx| this.delete_selected(cx)))
}

/// «Да» в строке «Удалить навсегда?» — подписка экрана на список.
pub(crate) fn watch_list(list: &Entity<MailList>, cx: &mut Context<Mailer>) -> Subscription {
    cx.subscribe(list, |this, _, p: &Purge, cx| this.remove(p.row.clone(), p.index, Removal::Purge, cx))
}

/// Полная дата для строки цитаты: «17.10.2026 12:00».
fn full_date(ts: i64) -> String {
    jiff::Timestamp::from_second(ts)
        .map(|t| t.to_zoned(jiff::tz::TimeZone::system()).strftime("%d.%m.%Y %H:%M").to_string())
        .unwrap_or_default()
}

fn addresses(list: &[Addr]) -> Vec<Address> {
    list.iter().map(|a| Address::new(&a.name, &a.email)).collect()
}

/// Черновик ответа или пересылки письма `row` с телом `body` (разобранным в `doc`). Пересылка берёт все вложения,
/// ответ — только картинки, на которые ссылается цитата. `local` — байты вложения, если они уже есть (демо);
/// иначе остаётся `blob_id`, и синхронизация скачивает вложение перед отправкой.
pub(crate) fn draft(
    mode: Mode,
    row: &EmailRow,
    body: &BodyRow,
    doc: &Document,
    me: &Address,
    signature: &str,
    local: impl Fn(&str) -> Option<Vec<u8>>,
) -> Draft {
    let original = Original {
        message_id: body.message_id.clone(),
        references: body.references.clone(),
        from: vec![Address::new(&row.from_name, &row.from_addr)],
        reply_to: addresses(&body.reply_to),
        to: addresses(&body.to),
        cc: addresses(&body.cc),
        subject: row.subject.clone(),
        date: full_date(row.received_at),
    };
    let mut d = postman_compose::respond(&original, doc, mode, me, signature);
    let quoted: HashSet<String> = crate::ir_view::cids(doc).into_iter().collect();
    for a in &body.attachments {
        let in_quote = a.cid.as_ref().is_some_and(|c| quoted.contains(c));
        if mode != Mode::Forward && !in_quote {
            continue;
        }
        d.attachments.push(Attachment {
            name: a.name.clone(),
            mime: a.mime.clone(),
            data: local(&a.blob_id).unwrap_or_default(),
            // Картинка в тексте — частью письма; остальное — вложением (чипом в окне письма).
            inline_cid: a.cid.clone().filter(|_| a.inline || in_quote),
            blob_id: Some(a.blob_id.clone()),
        });
    }
    d
}

/// Что делает «Удалить» с письмом из папки `from`.
#[derive(Debug, PartialEq)]
pub(crate) enum Removal {
    /// В «Удалённые».
    Move { from: String, to: String },
    /// Насовсем: письмо уже в «Удалённых» (или такой папки нет) — после подтверждения.
    Purge,
}

pub(crate) fn removal(from: &str, mailboxes: &[MailboxRow]) -> Removal {
    // «Удалённые» того же аккаунта: письмо общего ящика (S4-06) остаётся в нём.
    let account = postman_cache::account_of(from);
    let trash = mailboxes.iter().find(|m| m.role.as_deref() == Some("trash") && postman_cache::account_of(&m.id) == account);
    match trash {
        Some(t) if t.id != from => Removal::Move { from: from.to_owned(), to: t.id.clone() },
        _ => Removal::Purge,
    }
}

impl Mailer {
    fn toast(&mut self, level: Toast, text: String, cx: &mut Context<Self>) {
        self.toasts.update(cx, |t, cx| t.push(level, text, cx));
    }

    /// `mail.reply` / `mail.reply_all` / `mail.forward`: окно письма с черновиком; тело ещё не готово — ждём его.
    pub(crate) fn respond(&mut self, mode: Mode, cx: &mut Context<Self>) {
        let Some(row) = self.list.read(cx).selected_row() else {
            return self.toast(Toast::Info, tr!("actions-select-letter"), cx);
        };
        if self.respond_now(mode, &row.id, cx) {
            return;
        }
        let open = self.reading.read(cx).reading.as_ref().is_some_and(|r| r.row.id == row.id);
        let id = row.id.clone();
        if !open {
            self.open_letter(row, cx);
        }
        let watch = cx.observe(&self.reading, |this, _, cx| this.body_arrived(cx));
        self.actions.waiting = Some(Waiting { mode, id, _reading: watch });
    }

    /// Область чтения изменилась: тело ждущего ответа готово — открыть окно; открыто другое письмо — не ждать.
    fn body_arrived(&mut self, cx: &mut Context<Self>) {
        let Some(w) = &self.actions.waiting else { return };
        let (mode, id) = (w.mode, w.id.clone());
        let shown = self.reading.read(cx).reading.as_ref().is_some_and(|r| r.row.id == id);
        if !shown || self.respond_now(mode, &id, cx) {
            self.actions.waiting = None;
        }
    }

    /// Черновик из открытого письма `id`, если его тело уже разобрано. `true` — окно открыто (или не открылось:
    /// об этом уже тост).
    fn respond_now(&mut self, mode: Mode, id: &str, cx: &mut Context<Self>) -> bool {
        let live = self.backend.is_live();
        // Письмо общего ящика (S4-06) — ответ от его имени, если сервер дал его Identity; иначе от своего.
        let identity = self.compose.sender_for(postman_cache::account_of(id)).or_else(|| self.compose.me().cloned());
        let reading = self.reading.read(cx);
        let Some(r) = reading.reading.as_ref().filter(|r| r.row.id == id) else { return false };
        let (Some(body), Some(doc)) = (&r.body, &r.doc) else { return false };
        // Я — Identity сервера; в демо — владелец демо-ящика (его адрес знает область чтения).
        let (me, signature) =
            identity.unwrap_or_else(|| (Address::new("", reading.me.clone().unwrap_or_default()), String::new()));
        let local = |blob: &str| if live { None } else { postman_demo::blob(blob) };
        let d = draft(mode, &r.row, body, doc, &me, &signature, local);
        // Ответ и пересылка — в области чтения над письмом (S5-06б); «Развернуть» — во вкладку.
        self.open_reply(d, id.to_owned(), cx);
        true
    }

    /// `mail.delete`: в «Удалённые»; из «Удалённых» — строка «Удалить навсегда?».
    pub(crate) fn delete_selected(&mut self, cx: &mut Context<Self>) {
        let list = self.list.read(cx);
        let (Some(index), Some(row)) = (list.selected, list.selected_row()) else {
            return self.toast(Toast::Info, tr!("actions-select-letter"), cx);
        };
        // Результаты поиска — из разных папок: письмо уходит из своей.
        let from =
            list.mailbox.clone().filter(|m| row.mailbox_ids.contains(m)).or_else(|| row.mailbox_ids.first().cloned());
        let Some(from) = from else { return };
        // Папка общего ящика, открытая только для чтения (S4-06): удалять из неё нельзя.
        if !crate::list::may(&list.rights, &row, |r| r.remove) {
            return self.toast(Toast::Info, tr!("shared-read-only-delete"), cx);
        }
        match removal(&from, &self.folders.read(cx).mailboxes) {
            Removal::Purge => self.list.update(cx, |l, cx| l.ask_purge(index, row, cx)),
            m @ Removal::Move { .. } => self.remove(row, index, m, cx),
        }
    }

    /// Esc: снять строку «Удалить навсегда?». `true` — она была.
    pub(crate) fn cancel_purge(&mut self, cx: &mut Context<Self>) -> bool {
        if self.list.read(cx).confirm.is_none() {
            return false;
        }
        self.list.update(cx, |l, cx| l.answer_purge(false, cx));
        true
    }

    /// Убрать письмо: кэш — в фоне, затем команда серверу (живой ящик), список — на следующее письмо, тост.
    pub(crate) fn remove(&mut self, row: EmailRow, index: usize, how: Removal, cx: &mut Context<Self>) {
        let (cache, id) = (self.cache.clone(), row.id);
        let (key, op) = (
            id.clone(),
            match &how {
                Removal::Move { from, to } => Some((from.clone(), to.clone())),
                Removal::Purge => None,
            },
        );
        let task = cx.background_spawn(async move {
            match op {
                Some((from, to)) => cache.move_email(&key, &from, &to).map(|_| ()),
                None => cache.delete_emails(std::slice::from_ref(&key)),
            }
        });
        cx.spawn(async move |this, cx| {
            let stored = task.await;
            let _ = this.update(cx, |m, cx| {
                if let Err(e) = stored {
                    tracing::warn!("delete {id}: {e}");
                    return m.toast(Toast::Error, tr!("actions-delete-failed", error = e.to_string()), cx);
                }
                let text = match how {
                    Removal::Move { .. } => tr!("actions-moved-to-trash"),
                    Removal::Purge => tr!("actions-deleted"),
                };
                // Кэш уже поменян; сервер — после него: сбой на сервере вернёт письмо в кэш (синхронизация).
                if m.backend.is_live() {
                    let command = match how {
                        Removal::Move { from, to } => Command::Move { id: id.clone(), from, to },
                        Removal::Purge => Command::Destroy(id.clone()),
                    };
                    if !m.backend.send(command) {
                        m.sync_stopped(cx);
                    }
                }
                if m.reading.read(cx).reading.as_ref().is_some_and(|r| r.row.id == id) {
                    m.reading.update(cx, |r, cx| r.clear(cx));
                }
                // Следующее письмо встаёт на место удалённого: выделить его и открыть.
                m.list.update(cx, |l, cx| l.removed(&id, index, cx));
                m.toast(Toast::Success, text, cx);
            });
        })
        .detach();
    }
}

#[cfg(test)]
mod tests {
    // Не `super::*`: из `gpui_kit::*` пришёл бы атрибут `test` gpui вместо встроенного.
    use super::{Addr, Address, BodyRow, EmailRow, MailboxRow, Mode, Removal, draft, removal};
    use postman_cache::AttachmentRow;

    fn me() -> Address {
        Address::new("Я", "me@corp.example")
    }

    fn row() -> EmailRow {
        EmailRow {
            id: "m1".into(),
            thread_id: "t1".into(),
            mailbox_ids: vec!["inbox".into()],
            received_at: 1_792_240_200,
            subject: "Отчёт".into(),
            from_name: "Иван".into(),
            from_addr: "ivan@corp.example".into(),
            preview: String::new(),
            seen: true,
            flagged: false,
            has_attachment: true,
            size: 1,
            keywords: Vec::new(),
        }
    }

    fn addr(name: &str, email: &str) -> Addr {
        Addr { name: name.into(), email: email.into() }
    }

    fn attachment(blob: &str, name: &str, cid: Option<&str>) -> AttachmentRow {
        AttachmentRow {
            blob_id: blob.into(),
            name: name.into(),
            mime: "application/octet-stream".into(),
            size: 4,
            cid: cid.map(Into::into),
            inline: cid.is_some(),
        }
    }

    fn body() -> BodyRow {
        BodyRow {
            // Я — и в «Кому», и в «Копии» (другим регистром).
            to: vec![addr("Я", "me@corp.example"), addr("Пётр", "petr@corp.example")],
            cc: vec![addr("", "ME@corp.example"), addr("Анна", "anna@corp.example")],
            message_id: Some("orig@corp.example".into()),
            references: vec!["root@corp.example".into()],
            text: Some("текст".into()),
            attachments: vec![
                attachment("B-logo", "logo.png", Some("logo@corp")),
                attachment("B-xlsx", "Итоги.xlsx", None),
            ],
            ..Default::default()
        }
    }

    fn emails(v: &[Address]) -> Vec<String> {
        v.iter().map(|a| a.email.clone()).collect()
    }

    #[test]
    fn reply_all_goes_to_everyone_but_me() {
        let doc = postman_ir::from_plain("текст");
        let d = draft(Mode::ReplyAll, &row(), &body(), &doc, &me(), "", |_| None);
        assert_eq!(emails(&d.to), ["ivan@corp.example"]);
        assert_eq!(emails(&d.cc), ["petr@corp.example", "anna@corp.example"]);
        assert!(d.recipients().all(|a| !a.email.eq_ignore_ascii_case("me@corp.example")), "я не в адресатах");
        assert_eq!(d.subject, "RE: Отчёт");
        assert_eq!(d.in_reply_to.as_deref(), Some("orig@corp.example"));
        assert_eq!(d.references, ["root@corp.example", "orig@corp.example"]);
        assert!(d.markdown.contains("> текст"), "цитата: {}", d.markdown);
        // «Ответить» — только отправителю; картинок в цитате нет — вложений нет.
        let d = draft(Mode::Reply, &row(), &body(), &doc, &me(), "", |_| None);
        assert_eq!(emails(&d.to), ["ivan@corp.example"]);
        assert!(d.cc.is_empty() && d.attachments.is_empty());
    }

    #[test]
    fn forward_carries_the_attachments() {
        let doc = postman_ir::from_plain("текст");
        // Живой ящик: байтов нет — только blob_id, вложения скачает синхронизация перед отправкой.
        let d = draft(Mode::Forward, &row(), &body(), &doc, &me(), "", |_| None);
        assert_eq!(d.subject, "FW: Отчёт");
        assert!(d.to.is_empty() && d.in_reply_to.is_none());
        let got: Vec<_> = d
            .attachments
            .iter()
            .map(|a| (a.name.as_str(), a.blob_id.as_deref(), a.inline_cid.as_deref(), a.data.len()))
            .collect();
        assert_eq!(got, [("logo.png", Some("B-logo"), Some("logo@corp"), 0), ("Итоги.xlsx", Some("B-xlsx"), None, 0)]);
        // Демо: байты сразу.
        let d = draft(Mode::Forward, &row(), &body(), &doc, &me(), "", |b| Some(b.as_bytes().to_vec()));
        assert!(d.attachments.iter().all(|a| Some(a.data.as_slice()) == a.blob_id.as_deref().map(str::as_bytes)));
    }

    #[test]
    fn delete_moves_to_trash_and_purges_from_it() {
        let mb = |id: &str, role: Option<&str>| MailboxRow {
            id: id.into(),
            parent_id: None,
            name: id.into(),
            role: role.map(Into::into),
            sort_order: 0,
            total: 0,
            unread: 0,
            rights: postman_cache::Rights::ALL,
        };
        let boxes = [mb("in", Some("inbox")), mb("i/del", Some("trash")), mb("del", Some("trash")), mb("arch", None)];
        assert_eq!(removal("in", &boxes), Removal::Move { from: "in".into(), to: "del".into() });
        assert_eq!(removal("arch", &boxes), Removal::Move { from: "arch".into(), to: "del".into() });
        // Из «Удалённых» — насовсем (после подтверждения); нет такой папки — тоже.
        assert_eq!(removal("del", &boxes), Removal::Purge);
        assert_eq!(removal("in", &boxes[..2]), Removal::Purge);
        // Общий ящик (S4-06) — в свои «Удалённые», не в чужие.
        assert_eq!(removal("i/in", &boxes), Removal::Move { from: "i/in".into(), to: "i/del".into() });
        assert_eq!(removal("j/in", &boxes), Removal::Purge);

        // Кэш: в «Удалённые» — из папки ушло, следующее письмо на его месте; насовсем — и из «Удалённых».
        let cache = postman_cache::Cache::open_in_memory().unwrap();
        let (mut a, mut b) = (row(), row());
        a.received_at += 1;
        b.id = "m2".into();
        cache.upsert_emails(&[a, b]).unwrap();
        assert!(cache.move_email("m1", "inbox", "trash").unwrap());
        assert_eq!(cache.page("inbox", 0, 10).unwrap()[0].id, "m2");
        assert_eq!(cache.count("trash").unwrap(), 1);
        cache.delete_emails(&["m1".into()]).unwrap();
        assert_eq!((cache.count("trash").unwrap(), cache.count("inbox").unwrap()), (0, 1));
    }
}
