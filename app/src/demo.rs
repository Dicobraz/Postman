//! Синтетический ящик для замеров памяти/FPS без сервера.

use std::path::Path;

use postman_cache::{Addr, AttachmentRow, BodyRow, Cache, EmailRow, MailboxRow};

const SENDERS: &[(&str, &str)] = &[
    ("Иванова Мария", "m.ivanova@corp.example"),
    ("Петров Алексей", "a.petrov@corp.example"),
    ("Служба поддержки", "support@vendor.example"),
    ("Сидоров Игорь", "i.sidorov@corp.example"),
    ("GitLab", "noreply@gitlab.corp.example"),
    ("Бухгалтерия", "buh@corp.example"),
];

const SUBJECTS: &[&str] = &[
    "Отчёт за квартал",
    "Re: Совещание по проекту",
    "Счёт на оплату №",
    "Пропуск на парковку",
    "Pipeline failed",
    "Fwd: Договор — правки юристов",
    "Обновление регламента",
];

pub fn ensure(path: &Path, n: usize) -> postman_cache::Result<()> {
    let mut cache = Cache::open(path)?;
    if cache.count("inbox").unwrap_or(0) + cache.count("archive").unwrap_or(0) >= n {
        return Ok(());
    }
    let mb = |id: &str, name: &str, order| MailboxRow {
        id: id.into(),
        parent_id: None,
        name: name.into(),
        role: Some(id.into()),
        sort_order: order,
        total: 0,
        unread: 0,
    };
    cache.replace_mailboxes(&[
        mb("inbox", "Входящие", 1),
        mb("drafts", "Черновики", 2),
        mb("sent", "Отправленные", 3),
        mb("trash", "Удалённые", 4),
        mb("archive", "Архив", 5),
    ])?;
    let mut rng = 0x9E37_79B9_7F4A_7C15u64;
    let mut next = move || {
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        rng
    };
    let now = jiff::Timestamp::now().as_second();
    let mut at = now;
    let mut batch = Vec::with_capacity(5000);
    for i in 0..n {
        at -= (next() % 3600) as i64;
        let (name, addr) = SENDERS[next() as usize % SENDERS.len()];
        let subject = SUBJECTS[next() as usize % SUBJECTS.len()];
        let mailbox = match next() % 10 {
            0..=6 => "inbox",
            7 | 8 => "archive",
            _ => "sent",
        };
        batch.push(EmailRow {
            id: format!("demo{i}"),
            thread_id: format!("t{}", i / 3),
            mailbox_ids: vec![mailbox.into()],
            received_at: at,
            subject: format!("{subject} {}", i % 997),
            from_name: name.into(),
            from_addr: addr.into(),
            preview: "Коллеги, добрый день! Направляю материалы к обсуждению, прошу посмотреть до пятницы.".into(),
            seen: i > 40 && next() % 5 != 0,
            flagged: next() % 50 == 0,
            has_attachment: next() % 7 == 0,
            size: 4096 + next() % 100_000,
        });
        if batch.len() == batch.capacity() {
            cache.upsert_emails(&batch)?;
            batch.clear();
        }
    }
    cache.upsert_emails(&batch)?;
    Ok(())
}

/// Тела писем в демо-режиме: типичное письмо Outlook и «враждебное» —
/// чтобы видеть работу белого списка.
pub fn body(id: &str) -> BodyRow {
    let n: usize = id.trim_start_matches("demo").parse().unwrap_or(0);
    let att = |blob: &str, name: &str, mime: &str, size: u64, cid: Option<&str>| AttachmentRow {
        blob_id: blob.into(),
        name: name.into(),
        mime: mime.into(),
        size,
        cid: cid.map(Into::into),
        inline: cid.is_some(),
    };
    let (html, attachments) = if n.is_multiple_of(2) {
        (OUTLOOK, vec![
            att("logo", "logo.png", "image/png", LOGO.len() as u64, Some("logo@corp")),
            att("xlsx", "Итоги Q3.xlsx", "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet", 18_432, None),
        ])
    } else {
        (HOSTILE, vec![
            att("exe", "invoice.pdf.exe", "application/octet-stream", 73_728, None),
            att("rtlo", "счёт\u{202e}fdp.exe", "application/octet-stream", 73_728, None),
        ])
    };
    BodyRow {
        to: vec![Addr { name: "Команда проекта".into(), email: "team@corp.example".into() }],
        message_id: Some(format!("{id}@corp.example")),
        html: Some(html.into()),
        attachments,
        ..Default::default()
    }
}

const LOGO: &[u8] = include_bytes!("../../crates/worker/tests/fixtures/img.png");

/// Содержимое демо-вложений.
pub fn blob(blob_id: &str) -> Option<Vec<u8>> {
    match blob_id {
        "logo" => Some(LOGO.to_vec()),
        "xlsx" | "exe" | "rtlo" => Some(b"demo".to_vec()),
        _ => None,
    }
}

const OUTLOOK: &str = r#"<html><head><style>p.MsoNormal{margin:0}</style></head><body>
<div class=WordSection1><p class=MsoNormal><img src="cid:logo@corp" alt="Логотип"></p><p class=MsoNormal>Коллеги, добрый день!</p>
<p class=MsoNormal>Направляю <b>итоги квартала</b>. Обратите внимание на <span style='color:red;font-weight:bold'>раздел 3</span>
и <i>сроки</i>.</p>
<table border=1><tr><th>Показатель</th><th>План</th><th>Факт</th></tr>
<tr><td>Выручка</td><td>100</td><td>112</td></tr><tr><td>Расходы</td><td>80</td><td>77</td></tr></table>
<ul><li>Сверка до пятницы</li><li>Презентация — <a href="https://wiki.corp.example/q3">в вики</a></li></ul>
<p class=MsoNormal>&nbsp;</p>
<p class=MsoNormal>С уважением,<br>Мария Иванова<br>Финансовый отдел</p>
<div style='border:none;border-top:solid #E1E1E1 1.0pt'><p class=MsoNormal><b>From:</b> Петров Алексей<br><b>Sent:</b> Monday</p></div>
<blockquote><p>Мария, пришлите, пожалуйста, итоги.</p></blockquote></div></body></html>"#;

const HOSTILE: &str = r#"<html><body onload="steal()"><script>alert('xss')</script>
<p>Уважаемый клиент! Ваш аккаунт <span style="display:none">скрытый текст для антиспама</span> заблокирован.</p>
<p>Для разблокировки перейдите: <a href="https://evil.example/login">https://bank.example/secure</a></p>
<p><a href="javascript:steal()">Нажмите здесь</a></p>
<img src="https://tracker.example/pixel.gif?u=123" width=1 height=1>
<img src="https://cdn.example/banner.png" alt="Баннер банка">
<form action="https://evil.example"><input name=password type=password></form>
<iframe src="https://evil.example/frame"></iframe>
<p style="font-size:1px;color:#fff">мелкий белый текст</p></body></html>"#;
