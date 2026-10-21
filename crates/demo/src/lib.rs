//! Синтетический ящик для замеров памяти/FPS без сервера.

pub mod smime;

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use postman_cache::{AccountRow, Addr, AttachmentRow, BodyRow, Cache, EmailRow, MailboxRow, Recipients, Rights, scoped};

/// Отправители демо-писем (имя, адрес); их же берёт заполнение ящика стенда (`postman-core`, пример `fill_stand`).
pub const SENDERS: &[(&str, &str)] = &[
    ("Иванова Мария", "m.ivanova@corp.example"),
    ("Петров Алексей", "a.petrov@corp.example"),
    ("Служба поддержки", "support@vendor.example"),
    ("Сидоров Игорь", "i.sidorov@corp.example"),
    ("GitLab", "noreply@gitlab.corp.example"),
    ("Бухгалтерия", "buh@corp.example"),
];

/// Начало текста демо-писем (оно же краткое содержание в списке).
pub const PREVIEW: &str = "Коллеги, добрый день! Направляю материалы к обсуждению, прошу посмотреть до пятницы.";

/// Владелец демо-ящика: участник демо-приглашений и календаря.
pub const ME: &str = "me@corp.example";

/// Начало письма с упоминанием владельца (S5-03, фильтр «Упоминания»).
pub const MENTION: &str = "@Анна, посмотрите, пожалуйста, раздел рисков до отправки заказчику.";

/// Рассылка отдела: письма ей — не «Мне» (S5-03).
const TEAM: &str = "team@corp.example";

/// Темы демо-писем.
pub const SUBJECTS: &[&str] = &[
    "Отчёт за квартал",
    "Re: Совещание по проекту",
    "Счёт на оплату №",
    "Пропуск на парковку",
    "Pipeline failed",
    "Fwd: Договор — правки юристов",
    "Обновление регламента",
];

/// Каталог тестовых данных Postman (H-01): `%LOCALAPPDATA%\Pets\testdata\postman` — общий для приложений Pets,
/// не во временном каталоге; чистится `pets/scripts/stand/clean.ps1 -Testdata`.
pub fn testdata_dir() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .or_else(|| std::env::var_os("XDG_DATA_HOME"))
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Pets")
        .join("testdata")
        .join("postman")
}

/// Свежий ящик для `--bench`: копия шаблона `bench-template-<n>.redb` (создаётся один раз и переиспользуется:
/// сид фиксирован) в `bench.redb`. Бенч помечает письма прочитанными; без свежей копии каждый следующий прогон
/// делал бы меньше работы. Рабочую копию удаляет бенч после прогона (`bench::remove_on_finish`).
pub fn bench_copy(dir: &Path, n: usize) -> postman_cache::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let template = dir.join(format!("bench-template-{n}.redb"));
    ensure(&template, n)?;
    let path = dir.join("bench.redb");
    std::fs::copy(&template, &path)?;
    Ok(path)
}

pub fn ensure(path: &Path, n: usize) -> postman_cache::Result<()> {
    let cache = Cache::open(path)?;
    if cache.count("inbox").unwrap_or(0) + cache.count("archive").unwrap_or(0) >= n {
        // Шаблон старше беседы демо (S5-05) — дописать её.
        if cache.rows_by_ids(&[CHAIN[0].id.to_string()])?.is_empty() {
            chain(&cache, jiff::Timestamp::now().as_second())?;
        }
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
        rights: Rights::ALL,
    };
    cache.replace_mailboxes(None, &[
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
    let mut to = Vec::with_capacity(5000);
    for i in 0..n {
        at -= (next() % 3600) as i64;
        let (mut name, mut addr) = SENDERS[next() as usize % SENDERS.len()];
        let mut subject = SUBJECTS[next() as usize % SUBJECTS.len()];
        // Подписанные образцы (S5-12): отправитель и тема — как в подписанном письме.
        let signed = smime::sample_for(i);
        if let Some(s) = signed {
            (name, addr, subject) = (s.from_name, s.from_addr, s.subject);
        }
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
            subject: if signed.is_some() { subject.to_string() } else { format!("{subject} {}", i % 997) },
            from_name: name.into(),
            from_addr: addr.into(),
            preview: match signed {
                Some(s) => smime::preview(s),
                None if i % 13 == 2 => MENTION.into(),
                None => PREVIEW.into(),
            },
            seen: i > 40 && next() % 5 != 0,
            flagged: next() % 50 == 0,
            has_attachment: next() % 7 == 0 || signed.is_some(),
            size: 4096 + next() % 100_000,
            keywords: demo_keywords(i),
        });
        to.push((format!("demo{i}"), demo_recipients(i)));
        if batch.len() == batch.capacity() {
            cache.upsert_emails(&batch)?;
            cache.set_recipients(&to)?;
            batch.clear();
            to.clear();
        }
    }
    cache.upsert_emails(&batch)?;
    cache.set_recipients(&to)?;
    shared(&cache, now)?;
    chain(&cache, now)
}

/// Ключевые слова демо-письма `i` (S5-02): категории по умолчанию (`postman_core::labels::DEFAULTS`) и метка
/// Thunderbird — список показывает полосы категорий и метки, у части писем — по две.
fn demo_keywords(i: usize) -> Vec<String> {
    let mut out: Vec<&str> = Vec::new();
    match i % 6 {
        0 => out.push("pm.cat.clients"),
        1 => out.push("pm.cat.team"),
        3 => out.push("pm.cat.tenders"),
        4 => out.push("pm.cat.pentest"),
        _ => {}
    }
    if i.is_multiple_of(8) {
        out.push("pm.cat.team");
    }
    if i % 10 == 7 {
        out.push("pm.cat.services");
    }
    if i % 17 == 5 {
        out.push("$label2");
    }
    // Быстрые фильтры (S5-03): угрозы и слова жизненного цикла (их поставит S5-04) — у немногих писем.
    if i % 97 == 13 {
        out.push(words::PHISHING);
    }
    if i % 23 == 4 {
        out.push(words::DUE);
    }
    if i % 29 == 6 {
        out.push(words::WAIT);
    }
    if i % 31 == 9 {
        out.push(words::EXPIRE);
    }
    let mut out: Vec<String> = out.into_iter().map(String::from).collect();
    out.sort();
    out.dedup();
    out
}

/// Получатели демо-письма `i`: чаще мне, иногда — рассылке отдела (а мне копия).
fn demo_recipients(i: usize) -> Recipients {
    if i % 3 == 1 {
        Recipients { to: vec![TEAM.into()], cc: vec![ME.into()] }
    } else {
        Recipients { to: vec![ME.into()], cc: Vec::new() }
    }
}

/// Ключевые слова фильтров (`postman_core::filters`): демо от ядра не зависит (слои), слова — те же.
mod words {
    pub const PHISHING: &str = "$phishing";
    pub const DUE: &str = "pm.due";
    pub const WAIT: &str = "pm.wait";
    pub const EXPIRE: &str = "pm.expire";
}

/// Письмо беседы демо (S5-05): цепочка ответов с цитатами разных клиентов, мои письма — в «Отправленных».
pub struct ChainMessage {
    pub id: &'static str,
    /// Часов назад.
    pub hours_ago: i64,
    pub mine: bool,
    pub subject: &'static str,
    html: Option<&'static str>,
    text: Option<&'static str>,
}

/// Беседа демо: письмо, ответ из Outlook (шапка «От: … Отправлено: …»), ответ из Gmail (`gmail_quote`), ответ
/// текстом с `>`. Снимок `thread` открывает третье.
pub const CHAIN: &[ChainMessage] = &[
    ChainMessage { id: "chain1", hours_ago: 50, mine: false, subject: "Пилот SIEM: сроки", html: Some(CHAIN_1), text: None },
    ChainMessage { id: "chain2", hours_ago: 48, mine: true, subject: "RE: Пилот SIEM: сроки", html: Some(CHAIN_2), text: None },
    ChainMessage { id: "chain3", hours_ago: 26, mine: false, subject: "Re: Пилот SIEM: сроки", html: Some(CHAIN_3), text: None },
    ChainMessage { id: "chain4", hours_ago: 2, mine: true, subject: "Re: Пилот SIEM: сроки", html: None, text: Some(CHAIN_4) },
];

/// Письмо беседы демо, открытое в снимке `thread`.
pub const CHAIN_OPEN: &str = "chain3";

/// Имя владельца демо-ящика (адрес — [`ME`]).
pub const ME_NAME: &str = "Смирнова Анна";

fn chain(cache: &Cache, now: i64) -> postman_cache::Result<()> {
    let (name, addr) = SENDERS[1];
    let rows: Vec<EmailRow> = CHAIN
        .iter()
        .map(|m| EmailRow {
            id: m.id.into(),
            thread_id: "chain".into(),
            mailbox_ids: vec![if m.mine { "sent" } else { "inbox" }.into()],
            received_at: now - m.hours_ago * 3600,
            subject: m.subject.into(),
            from_name: if m.mine { ME_NAME } else { name }.into(),
            from_addr: if m.mine { ME } else { addr }.into(),
            preview: String::new(),
            seen: true,
            flagged: false,
            has_attachment: false,
            size: 4096,
            keywords: Vec::new(),
        })
        .collect();
    cache.upsert_emails(&rows)?;
    let to = |m: &ChainMessage| if m.mine { addr } else { ME };
    cache.set_recipients(&CHAIN.iter().map(|m| (m.id.to_string(), Recipients { to: vec![to(m).into()], cc: Vec::new() })).collect::<Vec<_>>())
}

fn chain_body(id: &str) -> Option<BodyRow> {
    let m = CHAIN.iter().find(|m| m.id == id)?;
    let (name, addr) = SENDERS[1];
    let to = if m.mine { Addr { name: name.into(), email: addr.into() } } else { Addr { name: ME_NAME.into(), email: ME.into() } };
    Some(BodyRow {
        to: vec![to],
        message_id: Some(format!("{id}@corp.example")),
        html: m.html.map(Into::into),
        text: m.text.map(Into::into),
        ..Default::default()
    })
}

const CHAIN_1: &str = "<p>Анна, добрый день!</p><p>Предлагаю начать пилот SIEM 3 ноября. Нужны от вас:</p>
<ul><li>список источников событий;</li><li>контакт администратора.</li></ul><p>С уважением,<br>Алексей Петров</p>";

const CHAIN_2: &str = r#"<html><body><div class=WordSection1><p class=MsoNormal>Алексей, добрый день!</p>
<p class=MsoNormal>Источники пришлю завтра, администратор — Сидоров Игорь.</p><p class=MsoNormal>&nbsp;</p>
<p class=MsoNormal>С уважением,<br>Анна Смирнова</p>
<div style='border:none;border-top:solid #E1E1E1 1.0pt;padding:3.0pt 0cm 0cm 0cm'><p class=MsoNormal><b>От:</b> Петров Алексей
&lt;a.petrov@corp.example&gt;<br><b>Отправлено:</b> понедельник, 3 ноября 2026 г. 10:15<br><b>Кому:</b> Смирнова Анна<br>
<b>Тема:</b> Пилот SIEM: сроки</p></div><p class=MsoNormal>Анна, добрый день!</p>
<p class=MsoNormal>Предлагаю начать пилот SIEM 3 ноября.</p></div></body></html>"#;

const CHAIN_3: &str = r#"<div dir="ltr">Спасибо! Тогда стартуем 3-го, созвон в 10:00.</div><br>
<div class="gmail_quote"><div dir="ltr" class="gmail_attr">пн, 3 нояб. 2026 г. в 12:10, Смирнова Анна &lt;me@corp.example&gt;:<br></div>
<blockquote class="gmail_quote"><p>Алексей, добрый день!</p><p>Источники пришлю завтра, администратор — Сидоров Игорь.</p>
<p><b>От:</b> Петров Алексей<br><b>Отправлено:</b> понедельник, 3 ноября 2026 г. 10:15</p></blockquote></div>"#;

const CHAIN_4: &str = "Подтверждаю, до встречи.

Анна

04.11.2026 10:05, Петров Алексей пишет:
> Спасибо! Тогда стартуем 3-го, созвон в 10:00.
>
> пн, 3 нояб. 2026 г. в 12:10, Смирнова Анна <me@corp.example>:
>> Алексей, добрый день!
";

/// Общие ящики демо (S4-06): ящик отдела, где можно всё, и ящик руководителя, открытый только для чтения.
pub const SHARED: &[(&str, &str, bool)] = &[("sales", "sales@corp.example", false), ("boss", "director@corp.example", true)];

fn shared(cache: &Cache, now: i64) -> postman_cache::Result<()> {
    let ro = Rights { read: true, add: false, remove: false, seen: false, keywords: false, submit: false };
    let acc = |id: &str, name: &str| AccountRow { id: id.into(), name: name.into(), is_personal: false, read_only: false };
    cache.replace_accounts(&SHARED.iter().map(|(id, name, _)| acc(id, name)).collect::<Vec<_>>())?;
    for (n, &(account, _, read_only)) in SHARED.iter().enumerate() {
        let scope = Some(account);
        let mb = |id: &str, name: &str, order| MailboxRow {
            id: scoped(scope, id),
            parent_id: None,
            name: name.into(),
            role: Some(id.into()),
            sort_order: order,
            total: 0,
            unread: 0,
            rights: if read_only { ro } else { Rights::ALL },
        };
        let boxes = if read_only {
            vec![mb("inbox", "Входящие", 1)]
        } else {
            vec![mb("inbox", "Входящие", 1), mb("drafts", "Черновики", 2), mb("sent", "Отправленные", 3), mb("trash", "Удалённые", 4)]
        };
        cache.replace_mailboxes(scope, &boxes)?;
        let rows: Vec<EmailRow> = (0..12)
            .map(|i| {
                let (name, addr) = SENDERS[(i + n) % SENDERS.len()];
                EmailRow {
                    id: scoped(scope, &format!("demo{}", i * 2 + n)),
                    thread_id: scoped(scope, &format!("t{i}")),
                    mailbox_ids: vec![scoped(scope, "inbox")],
                    received_at: now - (i as i64 + 1) * 5400,
                    subject: format!("{} {}", SHARED_SUBJECTS[(i + n) % SHARED_SUBJECTS.len()], 100 + i),
                    from_name: name.into(),
                    from_addr: addr.into(),
                    preview: PREVIEW.into(),
                    seen: i > 2,
                    flagged: false,
                    has_attachment: i % 4 == 0,
                    size: 8192,
                    keywords: Vec::new(),
                }
            })
            .collect();
        cache.upsert_emails(&rows)?;
    }
    Ok(())
}

/// Темы писем общих ящиков демо.
const SHARED_SUBJECTS: &[&str] = &["Заявка клиента №", "Коммерческое предложение", "Re: Счёт на оплату №", "Вопрос по поставке"];

/// Тела писем в демо-режиме: типичное письмо Outlook и «враждебное» —
/// чтобы видеть работу белого списка.
pub fn body(id: &str) -> BodyRow {
    // Письма общих ящиков демо — с префиксом аккаунта, тела — те же.
    let (account, id) = postman_cache::unscoped(id);
    if let Some(b) = chain_body(id) {
        return b;
    }
    let n: usize = id.trim_start_matches("demo").parse().unwrap_or(0);
    // Подписанное письмо (S5-12): тело — как его разобрал бы сервер, `smime.p7s` во вложениях, письмо целиком —
    // блоб `smime-<вид>`.
    if let Some(s) = smime::sample_for(n).filter(|_| account.is_none()) {
        return BodyRow {
            to: vec![Addr { name: ME_NAME.into(), email: ME.into() }],
            message_id: Some(format!("{id}@corp.example")),
            html: Some(s.html.into()),
            attachments: vec![AttachmentRow {
                blob_id: format!("p7s-{}", s.kind),
                name: "smime.p7s".into(),
                mime: "application/pkcs7-signature".into(),
                size: smime::P7S_SIZE,
                cid: None,
                inline: false,
            }],
            blob_id: Some(format!("smime-{}", s.kind)),
            ..Default::default()
        };
    }
    let att = |blob: &str, name: &str, mime: &str, size: u64, cid: Option<&str>| AttachmentRow {
        blob_id: blob.into(),
        name: name.into(),
        mime: mime.into(),
        size,
        cid: cid.map(Into::into),
        inline: cid.is_some(),
    };
    let (html, attachments) = if n % 100 == CHART_EVERY {
        // Большая картинка в тексте (S5-07б): кадр больше 1024 px по стороне.
        (CHART_HTML, vec![att("chart", "chart.png", "image/png", chart().len() as u64, Some("chart@corp"))])
    } else if n.is_multiple_of(10) {
        (INVITE_HTML, vec![att("ics", "invite.ics", "text/calendar", INVITE.len() as u64, None)])
    } else if n.is_multiple_of(2) {
        (OUTLOOK, vec![
            att("logo", "logo.png", "image/png", LOGO.len() as u64, Some("logo@corp")),
            // Встроенный просмотр (S5-07): PDF, архив с опасными файлами, книга с макросами, фото.
            att("pdf", "Отчёт за квартал.pdf", "application/pdf", REPORT.len() as u64, None),
            att("zip", "Снимки экрана.zip", "application/zip", SCREENS.len() as u64, None),
            att("xlsm", "Бюджет.xlsm", "application/vnd.ms-excel.sheet.macroEnabled.12", BUDGET.len() as u64, None),
            att("xlsx", "Итоги Q3.xlsx", "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet", 18_432, None),
            att("photo", "Фото стенда.jpg", "image/jpeg", PHOTO.len() as u64, None),
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

const LOGO: &[u8] = include_bytes!("../../worker/tests/fixtures/img.png");
/// Вложения для встроенного просмотра (S5-07): синтетические, из тестов воркера.
const REPORT: &[u8] = include_bytes!("../../worker/tests/fixtures/report.pdf");
const SCREENS: &[u8] = include_bytes!("../../worker/tests/fixtures/screens.zip");
const BUDGET: &[u8] = include_bytes!("../../worker/tests/fixtures/budget.xlsm");
const PHOTO: &[u8] = include_bytes!("../../worker/tests/fixtures/img.jpg");
/// Приглашение Outlook (iMIP REQUEST) — каждое десятое демо-письмо.
const INVITE: &[u8] = include_bytes!("../../worker/tests/fixtures/invite-outlook.ics");
/// Приглашение демо — на завтра (дата файла заменяется): в любой час запуска оно впереди и ждёт ответа, а не
/// «Встреча уже прошла». Длина файла та же — размер вложения в письме верен.
pub fn invite() -> Vec<u8> {
    let tomorrow = jiff::Zoned::now().date().tomorrow().unwrap_or(jiff::civil::date(2026, 10, 22));
    let day = format!("{:04}{:02}{:02}T", tomorrow.year(), tomorrow.month(), tomorrow.day());
    String::from_utf8_lossy(INVITE).replace(INVITE_DAY, &day).into_bytes()
}

/// День встречи в файле приглашения (`DTSTART`, `DTEND`).
const INVITE_DAY: &str = "20261021T";
const INVITE_HTML: &str = "<p>Иван Петров приглашает вас на встречу «Обсуждение пилота SIEM».</p>";

/// Письмо с большой картинкой в тексте — `demo4`, `demo104`… (одно на сотню: бенч открытия писем их почти не видит).
const CHART_EVERY: usize = 4;
const CHART_HTML: &str = r#"<p>Коллеги, график загрузки стенда за неделю — ниже.</p>
<p><img src="cid:chart@corp" alt="График загрузки"></p>
<p>Пики — по вторникам, подробности — в отчёте.</p>"#;
/// Сторона большой картинки: шире предела кадра в тексте письма (1600) — воркер её уменьшает.
pub const CHART_SIZE: (u32, u32) = (2400, 1500);

/// Большая картинка (PNG): рамка и цветные углы — на снимке видно, что кадр целиком и не сдвинут. Собирается один
/// раз при первом запросе; ошибка кодировщика — пустой файл (просмотр покажет ошибку разбора).
fn chart() -> &'static [u8] {
    static CHART: OnceLock<Vec<u8>> = OnceLock::new();
    CHART.get_or_init(|| chart_png(CHART_SIZE.0, CHART_SIZE.1).unwrap_or_default())
}

fn chart_png(w: u32, h: u32) -> Option<Vec<u8>> {
    const BORDER: u32 = 24;
    const CORNER: u32 = 220;
    let mut rgb = Vec::with_capacity((w * h * 3) as usize);
    for y in 0..h {
        for x in 0..w {
            let left = x < CORNER;
            let top = y < CORNER;
            let right = x >= w - CORNER;
            let bottom = y >= h - CORNER;
            let px: [u8; 3] = if x < BORDER || y < BORDER || x >= w - BORDER || y >= h - BORDER {
                [0x33, 0x33, 0x44]
            } else if left && top {
                [0xD1, 0x34, 0x38]
            } else if right && top {
                [0x10, 0x7C, 0x10]
            } else if left && bottom {
                [0x00, 0x78, 0xD4]
            } else if right && bottom {
                [0xFF, 0x8C, 0x00]
            } else if x.abs_diff(w / 2) < 8 || y.abs_diff(h / 2) < 8 {
                [0x33, 0x33, 0x44]
            } else if x % 150 < 3 || y % 150 < 3 {
                [0xB0, 0xC4, 0xDE]
            } else {
                // Сверху вниз — от голубого к белому.
                let t = (y * 255 / h) as u8;
                [0xC8u8.saturating_add(t / 5), 0xDCu8.saturating_add(t / 8), 0xF5]
            };
            rgb.extend_from_slice(&px);
        }
    }
    let mut out = Vec::new();
    let mut enc = png::Encoder::new(&mut out, w, h);
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    let mut writer = enc.write_header().ok()?;
    writer.write_image_data(&rgb).ok()?;
    writer.finish().ok()?;
    Some(out)
}

/// Содержимое демо-вложений.
pub fn blob(blob_id: &str) -> Option<Vec<u8>> {
    match blob_id {
        "logo" => Some(LOGO.to_vec()),
        "chart" => Some(chart().to_vec()),
        "ics" => Some(invite()),
        "pdf" => Some(REPORT.to_vec()),
        "zip" => Some(SCREENS.to_vec()),
        "xlsm" => Some(BUDGET.to_vec()),
        "photo" => Some(PHOTO.to_vec()),
        "xlsx" | "exe" | "rtlo" => Some(b"demo".to_vec()),
        other => other.strip_prefix("smime-").and_then(smime::raw).map(<[u8]>::to_vec),
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

/// Демо-календарь на текущую рабочую неделю: мои события и занятость коллег.
pub mod calendar {
    use postman_calendar::{Busy, BusyKind, Event, EventStatus, FreeBusy, Participant, Partstat, Span};

    /// Понедельник текущей недели, 00:00 в системном часовом поясе (Unix-секунды).
    pub fn week_start() -> i64 {
        let now = jiff::Zoned::now();
        let monday = now.date().checked_sub(jiff::Span::new().days(i64::from(now.weekday().to_monday_zero_offset()))).unwrap_or(now.date());
        monday.to_zoned(now.time_zone().clone()).map(|z| z.timestamp().as_second()).unwrap_or(0)
    }

    fn at(day: i64, h: i64, m: i64) -> i64 {
        week_start() + day * 86_400 + h * 3600 + m * 60
    }

    /// Кто я во встрече демо: организатор, приглашённый (с моим ответом) — организатор тогда коллега.
    #[derive(Clone, Copy)]
    enum Me {
        Owner,
        Invited(Partstat),
    }

    #[allow(clippy::too_many_arguments)]
    fn event(id: &str, title: &str, day: i64, (h, m): (i64, i64), mins: i64, location: &str, tentative: bool, me: Me, category: &str) -> Event {
        let mine = |status| Participant { name: "Я".into(), email: crate::ME.into(), owner: false, attendee: true, optional: false, status };
        let participants = match me {
            Me::Owner => vec![Participant { owner: true, ..mine(Partstat::Accepted) }],
            Me::Invited(status) => vec![
                Participant { name: "Петров Алексей".into(), email: "a.petrov@corp.example".into(), owner: true, attendee: true, optional: false, status: Partstat::Accepted },
                mine(status),
                Participant { name: "Иванова Мария".into(), email: "m.ivanova@corp.example".into(), owner: false, attendee: true, optional: false, status: Partstat::Accepted },
                Participant { name: "Сидоров Игорь".into(), email: "i.sidorov@corp.example".into(), owner: false, attendee: true, optional: true, status: Partstat::NeedsAction },
            ],
        };
        Event {
            id: id.into(),
            uid: format!("{id}@corp.example"),
            calendar_ids: vec!["work".into()],
            title: title.into(),
            description: String::new(),
            location: location.into(),
            span: Span::new(at(day, h, m), at(day, h, m) + mins * 60),
            all_day: false,
            status: if tentative { EventStatus::Tentative } else { EventStatus::Confirmed },
            free_busy: FreeBusy::Busy,
            participants,
            recurring: false,
            keywords: if category.is_empty() { Vec::new() } else { vec![format!("pm.cat.{category}")] },
            sequence: 0,
        }
    }

    pub fn my_events() -> Vec<Event> {
        use Me::{Invited, Owner};
        use Partstat::{Accepted, Declined, NeedsAction, Tentative};
        let weekly = |id: &str, day| {
            let mut e = event(id, "Планёрка отдела", day, (9, 30), 30, "Переговорная 1", false, Invited(Accepted), "team");
            e.recurring = true;
            e
        };
        let mut v = vec![
            weekly("e1", 0),
            event("e2", "Созвон с заказчиком", 0, (11, 0), 90, "Teams", false, Owner, "clients"),
            event("e3", "Обед с партнёром", 0, (13, 0), 60, "Кафе", true, Owner, ""),
            event("e4", "Разбор инцидента", 0, (11, 30), 60, "Переговорная 2", false, Invited(Accepted), "pentest"),
            weekly("e5", 1),
            event("e6", "Пресейл: SIEM", 1, (14, 0), 120, "Переговорная 3", false, Invited(NeedsAction), "tenders"),
            event("e7", "1:1 с руководителем", 2, (10, 0), 60, "", false, Owner, ""),
            event("e8", "Демо продукта", 2, (15, 0), 60, "Переговорная 1", false, Invited(Accepted), "clients"),
            event("e9", "Ревью архитектуры", 2, (15, 30), 90, "Переговорная 2", true, Invited(Tentative), "services"),
            event("e13", "Утренний стендап", 2, (11, 30), 30, "Teams", false, Invited(Accepted), "team"),
            event("e14", "Вебинар вендора", 2, (12, 30), 60, "", false, Invited(Declined), ""),
            weekly("e10", 3),
            event("e11", "Обучение: Stalwart", 3, (12, 0), 180, "Учебный класс", false, Invited(Accepted), "team"),
            event("e12", "Итоги недели", 4, (16, 0), 60, "Переговорная 1", false, Owner, ""),
        ];
        // На весь день, два дня: четверг и пятница.
        let mut conference = event("e15", "Выездной семинар", 3, (0, 0), 2 * 24 * 60, "Конгресс-центр", false, Invited(Accepted), "clients");
        conference.all_day = true;
        v.push(conference);
        v.sort_by_key(|e| e.span.start);
        v
    }

    /// События соседних недель и выходных (S5-21: виды «Неделя» и «Месяц» не пустые вокруг текущей недели): в
    /// занятость помощника и ленту приглашения не попадают.
    pub fn other_events() -> Vec<Event> {
        use Me::{Invited, Owner};
        use Partstat::Accepted;
        let mut v = vec![
            event("o1", "Ретро квартала", -4, (15, 0), 60, "Переговорная 1", false, Invited(Accepted), "team"),
            event("o2", "Аудит доступа", -3, (10, 0), 120, "Переговорная 2", false, Owner, "pentest"),
            event("o3", "Велопрогулка", 5, (10, 0), 180, "Парк", false, Owner, ""),
            event("o4", "Тендер: подача заявки", 8, (12, 0), 60, "", false, Owner, "tenders"),
            event("o5", "Созвон с заказчиком", 9, (11, 0), 90, "Teams", false, Owner, "clients"),
            event("o6", "Обучение: SOC", 15, (10, 0), 240, "Учебный класс", false, Invited(Accepted), "team"),
            event("o7", "Демо продукта", -11, (14, 0), 60, "Переговорная 1", false, Invited(Accepted), "clients"),
        ];
        // Командировка: с субботы через выходные до вторника — полосой через границу недели.
        let mut trip = event("o8", "Командировка", 12, (0, 0), 4 * 24 * 60, "Екатеринбург", false, Owner, "services");
        trip.all_day = true;
        v.push(trip);
        v.sort_by_key(|e| e.span.start);
        v
    }

    /// Коллеги для «Помощника планирования»: (имя, адрес, занятость).
    pub fn colleagues() -> Vec<(&'static str, &'static str, Vec<Busy>)> {
        let b = |day, (h, m): (i64, i64), mins: i64, kind| Busy { span: Span::new(at(day, h, m), at(day, h, m) + mins * 60), kind };
        use BusyKind::{Busy as B, Tentative as T, Unavailable as U};
        vec![
            ("Иванова Мария", "m.ivanova@corp.example", vec![b(0, (10, 0), 120, B), b(1, (9, 0), 240, B), b(2, (14, 0), 60, T), b(3, (9, 0), 540, U)]),
            ("Петров Алексей", "a.petrov@corp.example", vec![b(0, (9, 0), 60, B), b(0, (15, 0), 120, B), b(1, (13, 0), 120, B), b(2, (10, 0), 90, B), b(4, (11, 0), 60, T)]),
            ("Сидоров Игорь", "i.sidorov@corp.example", vec![b(0, (12, 0), 60, B), b(1, (10, 0), 60, B), b(2, (9, 0), 180, B), b(4, (9, 0), 120, B)]),
            ("Переговорная 3", "room3@corp.example", vec![b(1, (14, 0), 120, B), b(2, (11, 0), 60, B), b(3, (15, 0), 60, B)]),
        ]
    }

    /// Моя занятость из событий — для сетки помощника (отклонённые не занимают время).
    pub fn my_busy() -> Vec<Busy> {
        let declined = |e: &Event| e.participants.iter().any(|p| p.email == crate::ME && p.status == Partstat::Declined);
        my_events()
            .iter()
            .filter(|e| !declined(e))
            .map(|e| Busy { span: e.span, kind: if e.status == EventStatus::Tentative { BusyKind::Tentative } else { BusyKind::Busy } })
            .collect()
    }
}

/// Демо-контакты (S4-02): личная адресная книга и справочник сотрудников; одни и те же от запуска к запуску.
pub mod contacts {
    /// Личный контакт.
    pub struct Card {
        pub name: &'static str,
        pub emails: &'static [&'static str],
        pub phones: &'static [&'static str],
        pub organization: &'static str,
        pub title: &'static str,
    }

    const fn card(
        name: &'static str,
        emails: &'static [&'static str],
        phones: &'static [&'static str],
        organization: &'static str,
        title: &'static str,
    ) -> Card {
        Card { name, emails, phones, organization, title }
    }

    const MINE: &[Card] = &[
        card("Белова Ольга", &["o.belova@partner.example", "olga.belova@mail.example"], &["+7 495 123-45-67", "+7 916 555-01-02"], "ООО «Партнёр»", "Руководитель отдела закупок"),
        card("Гусев Дмитрий", &["d.gusev@integrator.example"], &["+7 903 222-33-44"], "АО «Интегратор»", "Архитектор решений"),
        card("Курьерская служба", &["orders@courier.example"], &["+7 495 777-00-11"], "", ""),
        card("Лебедева Анна", &["anna.lebedeva@law.example"], &[], "Юридическая фирма «Лебедева и партнёры»", "Партнёр"),
        card("Морозов Кирилл", &["k.morozov@bank.example"], &["+7 495 600-60-60"], "Банк", "Персональный менеджер"),
        card("Никитина Светлана", &["svetlana@design.example"], &["+7 921 000-12-34"], "Студия дизайна", "Арт-директор"),
        card("Служба поддержки", &["support@vendor.example"], &["8 800 100-00-00"], "Вендор", ""),
        card("Тарасов Олег", &["oleg.tarasov@mail.example"], &["+7 905 111-22-33"], "", ""),
    ];

    /// Справочник: (имя, адрес, тип Principal) — `individual`, `location` (переговорная), `resource`.
    const DIRECTORY: &[(&str, &str, &str)] = &[
        ("Бухгалтерия", "buh@corp.example", "individual"),
        ("Волков Сергей", "s.volkov@corp.example", "individual"),
        ("Иванова Мария", "m.ivanova@corp.example", "individual"),
        ("Кузнецова Елена", "e.kuznetsova@corp.example", "individual"),
        ("Переговорная 1", "room1@corp.example", "location"),
        ("Переговорная 2", "room2@corp.example", "location"),
        ("Переговорная 3", "room3@corp.example", "location"),
        ("Петров Алексей", "a.petrov@corp.example", "individual"),
        ("Петров Иван", "ivan.petrov@corp.example", "individual"),
        ("Проектор в учебном классе", "projector@corp.example", "resource"),
        ("Сидоров Игорь", "i.sidorov@corp.example", "individual"),
        ("Смирнова Анна", "anna.smirnova@corp.example", "individual"),
        ("Фёдорова Наталья", "n.fedorova@corp.example", "individual"),
        ("Щукин Алексей", "a.shchukin@corp.example", "individual"),
    ];

    pub fn mine() -> &'static [Card] {
        MINE
    }

    pub fn directory() -> &'static [(&'static str, &'static str, &'static str)] {
        DIRECTORY
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chart_letter_has_a_large_inline_png() {
        let b = body("demo104");
        assert!(b.html.as_deref().is_some_and(|h| h.contains("cid:chart@corp")));
        assert_eq!(b.attachments.len(), 1);
        let data = blob("chart").unwrap();
        assert_eq!(b.attachments[0].size, data.len() as u64);
        let info = png::Decoder::new(std::io::Cursor::new(data)).read_info().unwrap();
        assert_eq!((info.info().width, info.info().height), CHART_SIZE);
    }

    /// Приглашение демо — на завтра, тот же размер файла; фиксированной даты в нём нет.
    #[test]
    fn demo_invite_is_tomorrow() {
        let ics = String::from_utf8(invite()).unwrap();
        assert_eq!(ics.len(), INVITE.len());
        assert!(!ics.contains(INVITE_DAY));
        let t = jiff::Zoned::now().date().tomorrow().unwrap();
        assert!(ics.contains(&format!("DTSTART;TZID=Russian Standard Time:{:04}{:02}{:02}T110000", t.year(), t.month(), t.day())));
    }
}
