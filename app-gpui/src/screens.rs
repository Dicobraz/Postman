//! `--screens <каталог>`: снимки экранов без человека (S2-05) — агенты проверяют интерфейс по PNG, а не
//! управляют окном нажатиями.
//!
//! Сцены идут по очереди, как шаги замера (`drive_bench`): состояние ставится теми же методами, что и у
//! действий; затем ждём условие готовности сцены и ещё несколько показанных кадров; снимок — PrintWindow в
//! фоновом потоке; затем следующая сцена. На каждый снимок — строка `screen <имя> <путь> <ш>x<в>` в stdout,
//! в конце — выход с кодом 0; ошибка (снимок не удался, одного цвета, сцена не готова) — stderr и код 1.
//! Сообщения — по-английски: это диагностика для агентов, не текст интерфейса.
//!
//! `--screens <каталог> --live` (S3-04): живой ящик из окружения (`POSTMAN_URL`…) вместо демо — снимки `list` и
//! `letter` после первого `EmailsChanged` синхронизации; без `POSTMAN_URL` — строка входа (`signin`).

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
    /// Результаты поиска.
    Search,
    /// Раздел «Календарь».
    Calendar,
    /// Календарь с помощником планирования.
    Assistant,
    /// Строка входа (нет учётной записи).
    SignIn,
}

const SCENES: [Scene; 5] = [Scene::List, Scene::Letter, Scene::Search, Scene::Calendar, Scene::Assistant];
/// Живой ящик: запрос для поиска заранее не известен, календарь — пока демо (S3-05).
const LIVE_SCENES: [Scene; 2] = [Scene::List, Scene::Letter];
const SIGNIN_SCENES: [Scene; 1] = [Scene::SignIn];
/// Первая синхронизация живого ящика дольше сцены демо.
const LIVE_TIMEOUT: Duration = Duration::from_secs(120);

impl Scene {
    fn name(self) -> &'static str {
        match self {
            Scene::List => "list",
            Scene::Letter => "letter",
            Scene::Search => "search",
            Scene::Calendar => "calendar",
            Scene::Assistant => "assistant",
            Scene::SignIn => "signin",
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
}

impl Driver {
    /// Каталог создаётся сразу: ошибка — до открытия окна.
    pub fn new(dir: PathBuf, live: bool) -> std::io::Result<Self> {
        std::fs::create_dir_all(&dir)?;
        let dir = std::path::absolute(&dir)?;
        Ok(Self { dir, scene: 0, phase: Phase::Enter, live, signin: false })
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
            m.cal.assistant = false;
            m.set_section(Section::Calendar, cx);
        }
        Scene::Assistant => {
            m.cal.assistant = true;
            m.set_section(Section::Calendar, cx);
        }
        Scene::SignIn => m.set_section(Section::Mail, cx),
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
        Scene::Search => list.search_ids.is_some() && rows_loaded,
        Scene::Calendar => m.section == Section::Calendar && !m.cal.assistant,
        Scene::Assistant => m.section == Section::Calendar && m.cal.assistant,
        Scene::SignIn => m.live.signin.is_some(),
    }
}

/// PrintWindow — в фоновом потоке: он шлёт сообщения окну, и в потоке окна это повторный вход в оконную
/// процедуру gpui посреди отрисовки.
fn capture(m: &Mailer, scene: Scene, window: &Window, cx: &mut Context<Mailer>) {
    let Some(d) = &m.screens else { return };
    let path = d.dir.join(format!("{}.png", scene.name()));
    let hwnd = sys::window_id(window).unwrap_or_else(|e| fail(scene, &e));
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
