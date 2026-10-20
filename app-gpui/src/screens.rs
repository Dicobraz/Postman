//! `--screens <каталог>`: снимки экранов без человека (S2-05) — агенты проверяют интерфейс по PNG, а не
//! управляют окном нажатиями.
//!
//! Сцены идут по очереди, как шаги замера (`drive_bench`): состояние ставится теми же методами, что и у
//! действий; затем ждём условие готовности сцены и ещё несколько показанных кадров; снимок — PrintWindow в
//! фоновом потоке; затем следующая сцена. На каждый снимок — строка `screen <имя> <путь> <ш>x<в>` в stdout,
//! в конце — выход с кодом 0; ошибка (снимок не удался, одного цвета, сцена не готова) — stderr и код 1.
//! Сообщения — по-английски: это диагностика для агентов, не текст интерфейса.
//!
//! `--screens <каталог> --live` (S3-04): живой ящик из окружения (`POSTMAN_URL`…) вместо демо — снимки `list`,
//! `letter` и `reply` (S3-02) после первого `EmailsChanged` синхронизации, `calendar` и `event` — после недели с
//! сервера, `meeting` — после ответа о занятости участников из `POSTMAN_SCREENS_ATTENDEES` (адреса через `;`, S3-05),
//! `contacts` — после контактов и справочника с сервера (S4-02);
//! без `POSTMAN_URL` — строка входа (`signin`).

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use gpui_kit::*;
use postman_core::bench;

use crate::{Mailer, Section, sys};

/// Кадров с выполненным условием до снимка: первые кадры после смены состояния ещё могут быть прежними.
const SETTLE_FRAMES: u32 = 3;
/// Сцена не готова за это время (воркер не ответил, страница не прочиталась) — ошибка.
const SCENE_TIMEOUT: Duration = Duration::from_secs(30);
/// Запрос с результатами в демо-ящике: тема «Pipeline failed» (`crates/demo`).
const SEARCH_QUERY: &str = "pipeline";

#[derive(Clone, Copy)]
enum Scene {
    /// Входящие, ничего не выбрано.
    List,
    /// Первое письмо выбрано, тело разобрано воркером.
    Letter,
    /// Письмо с вложениями и картинкой `cid:` в тексте (S3-03).
    Attachments,
    /// Письмо с приглашением: карточка встречи (S3-03).
    Invite,
    /// Результаты поиска.
    Search,
    /// Раздел «Календарь».
    Calendar,
    /// Календарь с помощником планирования.
    Assistant,
    /// Календарь с подробностями события сбоку (S3-05).
    Event,
    /// Строка входа (нет учётной записи).
    SignIn,
    /// Окно письма с заполненными полями (своё окно: снимается его HWND).
    Compose,
    /// Ответ на письмо: окно письма с цитатой (S3-02).
    Reply,
    /// Окно новой встречи с помощником планирования (своё окно, S3-05).
    Meeting,
    /// Раздел «Контакты»: список и карточка первого контакта (S4-02).
    Contacts,
    /// Форма правки первого контакта (демо).
    ContactEdit,
}

const SCENES: [Scene; 13] = [
    Scene::List,
    Scene::Letter,
    Scene::Attachments,
    Scene::Invite,
    Scene::Search,
    Scene::Calendar,
    Scene::Assistant,
    Scene::Event,
    Scene::Contacts,
    Scene::ContactEdit,
    Scene::Compose,
    Scene::Reply,
    Scene::Meeting,
];

/// Живой ящик: запрос для поиска заранее не известен; ответ — на первое письмо; календарь — неделя с сервера,
/// окно встречи — занятость с сервера для адресов из `POSTMAN_SCREENS_ATTENDEES` (S3-05).
const LIVE_SCENES: [Scene; 7] =
    [Scene::List, Scene::Letter, Scene::Reply, Scene::Calendar, Scene::Event, Scene::Meeting, Scene::Contacts];
const SIGNIN_SCENES: [Scene; 1] = [Scene::SignIn];
/// Первая синхронизация живого ящика дольше сцены демо.
const LIVE_TIMEOUT: Duration = Duration::from_secs(120);

impl Scene {
    fn name(self) -> &'static str {
        match self {
            Scene::List => "list",
            Scene::Letter => "letter",
            Scene::Attachments => "attachments",
            Scene::Invite => "invite",
            Scene::Search => "search",
            Scene::Calendar => "calendar",
            Scene::Assistant => "assistant",
            Scene::Event => "event",
            Scene::SignIn => "signin",
            Scene::Compose => "compose",
            Scene::Reply => "reply",
            Scene::Meeting => "meeting",
            Scene::Contacts => "contacts",
            Scene::ContactEdit => "contact-edit",
        }
    }
}

#[derive(Clone, Copy)]
enum Phase {
    /// Поставить состояние сцены.
    Enter,
    /// Ждать готовности; `frames` — сколько кадров подряд условие выполнено.
    Wait { frames: u32, since: Instant },
    /// Снимок в фоне; кадры не запрашиваем.
    Capturing,
}

pub struct Driver {
    dir: PathBuf,
    scene: usize,
    phase: Phase,
    /// `--live`: живой ящик стенда вместо демо.
    live: bool,
    /// `--live` без учётной записи в окружении: снимок строки входа.
    signin: bool,
    /// Окно письма, открытое до сцены `reply`: ответ — следующее окно.
    composer: Option<EntityId>,
}

impl Driver {
    /// Каталог создаётся сразу: ошибка — до открытия окна.
    pub fn new(dir: PathBuf, live: bool) -> std::io::Result<Self> {
        std::fs::create_dir_all(&dir)?;
        let dir = std::path::absolute(&dir)?;
        Ok(Self { dir, scene: 0, phase: Phase::Enter, live, signin: false, composer: None })
    }

    /// Окно открывается со строкой входа: снимается она.
    pub fn sign_in(&mut self) {
        self.signin = true;
    }

    fn scenes(&self) -> &'static [Scene] {
        match (self.live, self.signin) {
            (_, true) => &SIGNIN_SCENES,
            (true, false) => &LIVE_SCENES,
            (false, false) => &SCENES,
        }
    }

    fn timeout(&self) -> Duration {
        if self.live { LIVE_TIMEOUT } else { SCENE_TIMEOUT }
    }
}

/// Шаг сценария; вызывается из `Render for Mailer` на каждом кадре.
pub fn drive(m: &mut Mailer, window: &mut Window, cx: &mut Context<Mailer>) {
    let Some(d) = &m.screens else { return };
    let Some(&scene) = d.scenes().get(d.scene) else { return };
    let timeout = d.timeout();
    match d.phase {
        Phase::Capturing => return,
        Phase::Enter => {
            set_phase(m, Phase::Wait { frames: 0, since: Instant::now() });
            if let Err(e) = enter(m, scene, window, cx) {
                fail(scene, &e);
            }
        }
        Phase::Wait { frames, since } => {
            if !ready(m, scene, cx) {
                if since.elapsed() > timeout {
                    fail(scene, &format!("scene not ready in {} s", timeout.as_secs()));
                }
                set_phase(m, Phase::Wait { frames: 0, since });
            } else if frames + 1 < SETTLE_FRAMES {
                set_phase(m, Phase::Wait { frames: frames + 1, since });
            } else {
                set_phase(m, Phase::Capturing);
                capture(m, scene, window, cx);
                return;
            }
        }
    }
    // Кадры — по запросу (патч gpui): без запроса в простое ничего не рисуется.
    window.request_animation_frame();
    // Следующий кадр — целиком, без кэша частей: `ready` читает часть (`m.reading`) раньше её отрисовки, gpui не
    // засчитывает части её же сущность, и `notify` части теряется — снимок показал бы прошлое письмо.
    window.on_next_frame(|window, _| window.refresh());
}

fn set_phase(m: &mut Mailer, phase: Phase) {
    if let Some(d) = &mut m.screens {
        d.phase = phase;
    }
}

/// Состояние сцены — через те же методы, что вызывают действия пользователя.
fn enter(m: &mut Mailer, scene: Scene, window: &mut Window, cx: &mut Context<Mailer>) -> Result<(), String> {
    match scene {
        Scene::List => m.set_section(Section::Mail, cx),
        Scene::Letter => m.list.update(cx, |l, cx| l.select(0, cx)),
        Scene::Attachments | Scene::Invite => {
            let Some(i) = demo_row(m.list.read(cx), scene) else {
                return Err("no such message in the first rows of the demo inbox".into());
            };
            m.list.update(cx, |l, cx| l.select(i, cx));
        }
        Scene::Search => {
            m.search.update(cx, |s, cx| s.set_value(SEARCH_QUERY, window, cx));
            m.run_search(SEARCH_QUERY, cx);
            if m.list.read(cx).count == 0 {
                return Err(format!("search \"{SEARCH_QUERY}\" found nothing"));
            }
        }
        Scene::Calendar => {
            // Как `search::Clear`: календарь снимается без следов поиска в папках.
            m.search.update(cx, |s, cx| s.set_value("", window, cx));
            m.run_search("", cx);
            m.cal.update(cx, |c, cx| {
                c.assistant = false;
                cx.notify();
            });
            m.set_section(Section::Calendar, cx);
        }
        Scene::Assistant => {
            m.cal.update(cx, |c, cx| {
                c.assistant = true;
                cx.notify();
            });
            m.set_section(Section::Calendar, cx);
        }
        Scene::Event => {
            m.cal.update(cx, |c, cx| {
                c.assistant = false;
                c.select_busiest(cx);
            });
            m.set_section(Section::Calendar, cx);
        }
        Scene::SignIn => m.set_section(Section::Mail, cx),
        Scene::Compose => {
            m.cal.update(cx, |c, cx| {
                c.assistant = false;
                c.close_details(cx);
            });
            m.set_section(Section::Mail, cx);
            // Окно — после кадра основного: не открывать его посреди отрисовки.
            let mailer = cx.entity();
            cx.defer(move |cx| mailer.update(cx, |m, cx| m.open_compose_screen(cx)));
        }
        Scene::Reply => {
            // Демо — письмо с картинкой в тексте и цитатой Outlook; живой ящик — первое письмо.
            let live = m.screens.as_ref().is_some_and(|d| d.live);
            let i = if live { Some(0) } else { demo_row(m.list.read(cx), Scene::Attachments) };
            let Some(i) = i else { return Err("no message to reply to".into()) };
            let composer = m.compose.last().map(|c| c.entity_id());
            if let Some(d) = &mut m.screens {
                d.composer = composer;
            }
            m.list.update(cx, |l, cx| l.select(i, cx));
            // Как `mail.reply`: окно — когда тело разобрано (ответ ждёт его сам), и не посреди отрисовки.
            let mailer = cx.entity();
            cx.defer(move |cx| mailer.update(cx, |m, cx| m.respond(postman_compose::Mode::Reply, cx)));
        }
        Scene::Contacts => {
            m.cal.update(cx, |c, cx| c.close_details(cx));
            m.set_section(Section::Contacts, cx);
            m.contacts.update(cx, |c, cx| c.select_first(cx));
        }
        Scene::ContactEdit => m.contacts.update(cx, |c, cx| c.edit_selected(window, cx)),
        Scene::Meeting => {
            // Окно письма прошлой сцены остаётся: снимается своё окно.
            m.set_section(Section::Calendar, cx);
            let mailer = cx.entity();
            cx.defer(move |cx| mailer.update(cx, |m, cx| m.open_meeting_screen(cx)));
        }
    }
    Ok(())
}

/// Условие готовности сцены: данные, которые она показывает, уже в состоянии view.
fn ready(m: &Mailer, scene: Scene, cx: &App) -> bool {
    let list = m.list.read(cx);
    // Окно строк прочитано (первое — сразу при открытии папки, следующие — в фоне).
    let rows_loaded = list.count > 0 && !list.rows.1.is_empty() && list.page_pending.is_none();
    let reading = m.reading.read(cx).reading.as_ref();
    match scene {
        Scene::List => {
            // Живой ящик: снимок — после первой синхронизации, а не по старому кэшу.
            let synced = m.live.synced || m.screens.as_ref().is_some_and(|d| !d.live);
            synced
                && m.section == Section::Mail
                && list.search_ids.is_none()
                && list.selected.is_none()
                && rows_loaded
                && reading.is_none()
        }
        Scene::Letter => {
            let first = list.row(0).map(|r| r.id.as_str());
            list.selected == Some(0)
                && first.is_some()
                && reading.is_some_and(|r| r.doc.is_some() && Some(r.row.id.as_str()) == first)
        }
        Scene::Attachments | Scene::Invite => {
            let target = demo_row(list, scene).and_then(|i| list.row(i)).map(|r| r.id.as_str());
            let shown = |r: &crate::reading::Reading| match scene {
                Scene::Attachments => r.doc.is_some() && !r.images.is_empty(),
                _ => r.doc.is_some() && r.invite.as_ref().is_some_and(|c| c.invitation().is_some()),
            };
            target.is_some() && reading.is_some_and(|r| Some(r.row.id.as_str()) == target && shown(r))
        }
        Scene::Search => list.search_ids.is_some() && rows_loaded,
        Scene::Calendar => {
            let c = m.cal.read(cx);
            // Живой ящик: неделя пришла с сервера.
            let loaded = c.loaded || m.screens.as_ref().is_some_and(|d| !d.live);
            m.section == Section::Calendar && !c.assistant && loaded
        }
        Scene::Assistant => m.section == Section::Calendar && m.cal.read(cx).assistant,
        Scene::Event => {
            // Живая неделя может быть пустой: тогда снимок — без подробностей.
            let c = m.cal.read(cx);
            m.section == Section::Calendar && (c.selected() || c.is_empty())
        }
        Scene::SignIn => m.live.signin.is_some(),
        Scene::Compose => compose_hwnd(m, cx).is_some(),
        Scene::Reply => {
            // Новое окно письма — ответ (окно сцены `compose` ещё открыто).
            let before = m.screens.as_ref().and_then(|d| d.composer);
            let reply = m.compose.last().is_some_and(|c| Some(c.entity_id()) != before);
            reply && compose_hwnd(m, cx).is_some()
        }
        Scene::Meeting => meeting_hwnd(m, cx).is_some(),
        Scene::Contacts => {
            // Живой ящик: контакты с сервера пришли — выделить первый (до ответа выделять нечего).
            let c = m.contacts.read(cx);
            m.section == Section::Contacts && c.ready()
        }
        Scene::ContactEdit => m.section == Section::Contacts && m.contacts.read(cx).editing(),
    }
}

/// Первое прочитанное письмо демо-ящика для сцены: с картинкой `cid:` (и без приглашения) или с приглашением.
fn demo_row(list: &crate::list::MailList, scene: Scene) -> Option<usize> {
    let (offset, rows) = &list.rows;
    let wanted = |a: &[postman_cache::AttachmentRow]| {
        let invite = a.iter().any(crate::attachments::is_invite);
        match scene {
            Scene::Invite => invite,
            _ => !invite && a.iter().any(|a| a.cid.is_some()),
        }
    };
    rows.iter().position(|r| wanted(&postman_demo::body(&r.id).attachments)).map(|p| p + offset)
}

/// PrintWindow — в фоновом потоке: он шлёт сообщения окну, и в потоке окна это повторный вход в оконную
/// процедуру gpui посреди отрисовки.
fn capture(m: &Mailer, scene: Scene, window: &Window, cx: &mut Context<Mailer>) {
    let Some(d) = &m.screens else { return };
    let path = d.dir.join(format!("{}.png", scene.name()));
    let hwnd = match scene {
        Scene::Compose | Scene::Reply => compose_hwnd(m, cx).unwrap_or_else(|| fail(scene, "no compose window")),
        Scene::Meeting => meeting_hwnd(m, cx).unwrap_or_else(|| fail(scene, "no meeting window")),
        _ => sys::window_id(window).unwrap_or_else(|e| fail(scene, &e)),
    };
    let task = cx.background_spawn(async move { shoot(hwnd, &path).map(|size| (path, size)) });
    cx.spawn(async move |this, cx| {
        let result = task.await;
        let _ = this.update(cx, |m, cx| {
            match result {
                Ok((path, (w, h))) => println!("screen {} {} {w}x{h}", scene.name(), path.display()),
                Err(e) => fail(scene, &e),
            }
            let Some(d) = &mut m.screens else { return };
            d.scene += 1;
            d.phase = Phase::Enter;
            if d.scene >= d.scenes().len() {
                finish(0);
            }
            cx.notify();
        });
    })
    .detach();
}

/// Окно письма открыто и нарисовано: его дескриптор.
fn compose_hwnd(m: &Mailer, cx: &App) -> Option<isize> {
    m.compose.last().and_then(|c| c.read(cx).screen_hwnd())
}

/// Окно встречи открыто, нарисовано, занятость получена: его дескриптор.
fn meeting_hwnd(m: &Mailer, cx: &App) -> Option<isize> {
    m.meetings.last().and_then(|w| w.read(cx).screen_hwnd())
}

fn shoot(hwnd: isize, path: &Path) -> Result<(u32, u32), String> {
    let img = sys::capture(hwnd)?;
    if uniform(&img.rgba) {
        return Err(format!("capture {}x{} is a single color: the window is not drawn", img.width, img.height));
    }
    write_png(path, &img).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok((img.width, img.height))
}

/// Все пиксели одного цвета (чёрный кадр, пустое окно).
fn uniform(rgba: &[u8]) -> bool {
    let (px, _) = rgba.as_chunks::<4>();
    px.iter().all(|p| Some(p) == px.first())
}

fn write_png(path: &Path, img: &sys::Image) -> Result<(), png::EncodingError> {
    let file = std::io::BufWriter::new(std::fs::File::create(path)?);
    let mut enc = png::Encoder::new(file, img.width, img.height);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut w = enc.write_header()?;
    w.write_image_data(&img.rgba)?;
    w.finish()
}

fn fail(scene: Scene, e: &str) -> ! {
    eprintln!("screens: {}: {e}", scene.name());
    finish(1)
}

/// Выход: рабочая копия ящика удаляется, как после замера (без `--keep`).
fn finish(code: i32) -> ! {
    bench::remove_work_copy();
    std::process::exit(code)
}

#[cfg(test)]
mod tests {
    use super::uniform;

    #[test]
    fn one_color_capture_is_rejected() {
        assert!(uniform(&[0, 0, 0, 255].repeat(16)));
        assert!(uniform(&[]));
        let mut img = [250, 250, 250, 255].repeat(16);
        img[20] = 0;
        assert!(!uniform(&img));
    }
}
