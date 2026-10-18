//! Наполнение поискового индекса. Ошибки индекса не мешают почте:
//! поиск тогда откатывается на простой поиск по кэшу.

use postman_cache::{BodyRow, EmailRow};
use postman_search::{Doc, SearchIndex};

/// Строки списка: тема, отправитель, превью.
pub fn rows(index: &SearchIndex, rows: &[EmailRow]) {
    let people: Vec<String> = rows.iter().map(|r| format!("{} {}", r.from_name, r.from_addr)).collect();
    let docs: Vec<Doc> = rows
        .iter()
        .zip(&people)
        .map(|(r, p)| Doc { id: &r.id, subject: &r.subject, people: p, body: &r.preview, attachments: "", received_at: r.received_at })
        .collect();
    if index.upsert(&docs).is_ok() {
        let _ = index.commit();
    }
}

/// Письмо целиком, когда известен его текст: получатели, тело, имена вложений.
pub fn full(index: &SearchIndex, row: &EmailRow, body: &BodyRow, text: &str) {
    let mut people = format!("{} {}", row.from_name, row.from_addr);
    for a in body.to.iter().chain(&body.cc) {
        people.push(' ');
        people.push_str(&a.name);
        people.push(' ');
        people.push_str(&a.email);
    }
    let attachments: Vec<&str> = body.attachments.iter().map(|a| a.name.as_str()).collect();
    let attachments = attachments.join(" ");
    let doc = Doc { id: &row.id, subject: &row.subject, people: &people, body: text, attachments: &attachments, received_at: row.received_at };
    // Коммит — пакетно, из UI-цикла (см. `App::commit_index`): открытие писем подряд не плодит сегменты.
    let _ = index.upsert(&[doc]);
}
