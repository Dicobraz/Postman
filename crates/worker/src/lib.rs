//! Изолированный воркер разбора писем (HANDOFF §3a).
//!
//! UI держит один процесс `postman-worker` и общается с ним сообщениями
//! «JSON-заголовок + бинарная часть» (каждая — `u32 LE длина + байты`)
//! по stdin/stdout. Бинарная часть — исходная картинка в запросе и RGBA в ответе. Воркер запирает себя до чтения
//! первого запроса: без сети, без файловой системы, без дочерних процессов,
//! на Windows — без win32k. Зависание или падение воркера стоит одного письма:
//! клиент убивает процесс и поднимает новый.

mod sandbox;

use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Duration;

use postman_ir::Document;
use serde::{Deserialize, Serialize};

pub use sandbox::Level;

/// Больше этого не принимаем ни в одну сторону.
const MAX_FRAME: usize = 64 * 1024 * 1024;

#[derive(Debug, Serialize, Deserialize)]
pub enum Request {
    Html(String),
    Text(String),
    /// Приглашение iCalendar (.ics) в бинарной части; ответ — METHOD и JSCalendar (JSON).
    Calendar,
    /// Картинка (PNG/JPEG/GIF/WebP) в бинарной части; ответ — RGBA не больше `max_side` по стороне.
    Image { max_side: u32 },
    /// Проверки песочницы для тестов: попытка открыть файл / сокет / зависнуть.
    #[cfg(debug_assertions)]
    #[doc(hidden)]
    Probe(Probe),
}

#[cfg(debug_assertions)]
#[doc(hidden)]
#[derive(Debug, Serialize, Deserialize)]
pub enum Probe {
    ReadFile(String),
    Connect(String),
    Spawn(String),
    Hang,
}

#[cfg(debug_assertions)]
fn probe(p: Probe) -> Response {
    let r = match p {
        Probe::ReadFile(path) => std::fs::read(path).map(|_| ()),
        Probe::Connect(addr) => std::net::TcpStream::connect(addr).map(drop),
        Probe::Spawn(cmd) => std::process::Command::new(cmd).status().map(drop),
        Probe::Hang => loop {
            std::hint::spin_loop();
        },
    };
    match r {
        Ok(()) => Response::Err("ДОСТУП РАЗРЕШЁН".into()),
        Err(e) => Response::Err(format!("запрещено: {e}")),
    }
}

#[derive(Debug, Serialize, Deserialize)]
enum Response {
    Ok(Document),
    /// RGBA8 в бинарной части.
    Image { width: u32, height: u32 },
    Calendar(serde_json::Value),
    Err(String),
}

/// Декодированная картинка.
pub struct Rgba {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

fn decode_image(bytes: &[u8], max_side: u32) -> Result<Rgba, String> {
    use image::{ImageFormat, ImageReader, Limits};
    let mut reader = ImageReader::new(io::Cursor::new(bytes)).with_guessed_format().map_err(|e| e.to_string())?;
    if !matches!(reader.format(), Some(ImageFormat::Png | ImageFormat::Jpeg | ImageFormat::Gif | ImageFormat::WebP)) {
        return Err("формат картинки не поддерживается".into());
    }
    let mut limits = Limits::default();
    limits.max_image_width = Some(16_384);
    limits.max_image_height = Some(16_384);
    limits.max_alloc = Some(256 * 1024 * 1024);
    reader.limits(limits);
    let mut img = reader.decode().map_err(|e| e.to_string())?;
    let max_side = max_side.clamp(16, 4096);
    if img.width() > max_side || img.height() > max_side {
        img = img.thumbnail(max_side, max_side);
    }
    let rgba = img.into_rgba8();
    Ok(Rgba { width: rgba.width(), height: rgba.height(), pixels: rgba.into_raw() })
}

/// iCalendar → `{ "method": "REQUEST", "data": <JSCalendar> }`. Чужие данные — только в песочнице.
/// Без песочницы — только для тестов и самого воркера.
pub fn parse_calendar(bytes: &[u8]) -> Result<serde_json::Value, String> {
    if bytes.len() > 2 * 1024 * 1024 {
        return Err("приглашение слишком большое".into());
    }
    let text = std::str::from_utf8(bytes).map_err(|_| "приглашение не в UTF-8".to_string())?;
    let method = text
        .lines()
        .find_map(|l| l.trim().strip_prefix("METHOD:"))
        .map(|m| m.trim().to_ascii_uppercase())
        .unwrap_or_default();
    let ical = match calcard::Parser::new(text).entry() {
        calcard::Entry::ICalendar(c) => c,
        _ => return Err("не iCalendar".into()),
    };
    let js = ical.into_jscalendar::<String, String>();
    let data = serde_json::to_value(&js.0).map_err(|e| e.to_string())?;
    Ok(serde_json::json!({ "method": method, "data": data }))
}

fn write_msg(w: &mut impl Write, json: &[u8], bin: &[u8]) -> io::Result<()> {
    write_frame(w, json)?;
    write_frame(w, bin)
}

fn read_msg(r: &mut impl Read) -> io::Result<(Vec<u8>, Vec<u8>)> {
    Ok((read_frame(r)?, read_frame(r)?))
}

fn write_frame(w: &mut impl Write, payload: &[u8]) -> io::Result<()> {
    if payload.len() > MAX_FRAME {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "кадр слишком большой"));
    }
    w.write_all(&(payload.len() as u32).to_le_bytes())?;
    w.write_all(payload)?;
    w.flush()
}

fn read_frame(r: &mut impl Read) -> io::Result<Vec<u8>> {
    let mut len = [0u8; 4];
    r.read_exact(&mut len)?;
    let len = u32::from_le_bytes(len) as usize;
    if len > MAX_FRAME {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "кадр слишком большой"));
    }
    let mut buf = vec![0; len];
    r.read_exact(&mut buf)?;
    Ok(buf)
}

/// Точка входа процесса-воркера.
pub fn serve() -> ! {
    let mut stdin = io::stdin().lock();
    let mut stdout = io::stdout().lock();
    if let Err(e) = sandbox::lockdown() {
        // Без песочницы не работаем: лучше показать письмо текстом, чем разбирать HTML без защиты.
        let json = serde_json::to_vec(&Response::Err(format!("песочница: {e}"))).unwrap_or_default();
        let _ = write_msg(&mut stdout, &json, &[]);
        std::process::exit(3);
    }
    loop {
        let Ok((frame, bin)) = read_msg(&mut stdin) else { std::process::exit(0) };
        let mut out_bin = Vec::new();
        let resp = match serde_json::from_slice::<Request>(&frame) {
            Ok(Request::Html(h)) => Response::Ok(postman_ir::from_html(&h)),
            Ok(Request::Text(t)) => Response::Ok(postman_ir::from_plain(&t)),
            Ok(Request::Calendar) => match parse_calendar(&bin) {
                Ok(v) => Response::Calendar(v),
                Err(e) => Response::Err(e),
            },
            Ok(Request::Image { max_side }) => match decode_image(&bin, max_side) {
                Ok(img) => {
                    out_bin = img.pixels;
                    Response::Image { width: img.width, height: img.height }
                }
                Err(e) => Response::Err(e),
            },
            #[cfg(debug_assertions)]
            Ok(Request::Probe(p)) => probe(p),
            Err(e) => Response::Err(e.to_string()),
        };
        let out = serde_json::to_vec(&resp).unwrap_or_default();
        if write_msg(&mut stdout, &out, &out_bin).is_err() {
            std::process::exit(0);
        }
    }
}

/// Запущенный процесс-воркер, как его видит клиент.
pub(crate) struct Process {
    pub stdin: Box<dyn Write + Send>,
    pub stdout: Box<dyn Read + Send>,
    pub kill: Box<dyn FnMut() + Send>,
    /// Код завершения, если процесс уже завершился (для текста ошибки).
    pub exit_code: Box<dyn FnMut() -> Option<u32> + Send>,
    pub level: Level,
}

struct Running {
    stdin: Box<dyn Write + Send>,
    responses: mpsc::Receiver<io::Result<(Vec<u8>, Vec<u8>)>>,
    kill: Box<dyn FnMut() + Send>,
    exit_code: Box<dyn FnMut() -> Option<u32> + Send>,
    /// Воркер ответил хотя бы раз — значит, в этой песочнице он работоспособен.
    answered: bool,
}

impl Drop for Running {
    fn drop(&mut self) {
        (self.kill)();
    }
}

/// Клиент воркера. Не потокобезопасен: держите в одном потоке.
pub struct Client {
    exe: PathBuf,
    timeout: Duration,
    running: Option<Running>,
    level: Level,
    /// Строгая песочница не дала воркеру работать — запускаем с ослабленной (Windows: без AppContainer).
    relaxed: bool,
}

impl Client {
    pub fn new(exe: PathBuf, timeout: Duration) -> Self {
        Self { exe, timeout, running: None, level: Level::None, relaxed: false }
    }

    /// Рядом с основным исполняемым файлом.
    pub fn default_exe() -> PathBuf {
        let name = if cfg!(windows) { "postman-worker.exe" } else { "postman-worker" };
        std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.join(name)))
            .unwrap_or_else(|| PathBuf::from(name))
    }

    /// Насколько изолирован текущий процесс (после первого запроса).
    pub fn level(&self) -> Level {
        self.level
    }

    pub fn render(&mut self, req: &Request) -> Result<Document, String> {
        match self.call(req, &[])?.0 {
            Response::Ok(doc) => Ok(doc),
            Response::Image { .. } | Response::Calendar(_) => Err("неожиданный ответ".into()),
            Response::Err(e) => Err(e),
        }
    }

    /// Декодировать картинку из письма в песочнице.
    pub fn decode_image(&mut self, bytes: &[u8], max_side: u32) -> Result<Rgba, String> {
        match self.call(&Request::Image { max_side }, bytes)? {
            (Response::Image { width, height }, pixels) if pixels.len() == width as usize * height as usize * 4 => {
                Ok(Rgba { width, height, pixels })
            }
            (Response::Err(e), _) => Err(e),
            _ => Err("неожиданный ответ".into()),
        }
    }

    /// Разобрать приглашение (.ics) в песочнице.
    pub fn parse_calendar(&mut self, ics: &[u8]) -> Result<serde_json::Value, String> {
        match self.call(&Request::Calendar, ics)?.0 {
            Response::Calendar(v) => Ok(v),
            Response::Err(e) => Err(e),
            _ => Err("неожиданный ответ".into()),
        }
    }

    fn call(&mut self, req: &Request, bin: &[u8]) -> Result<(Response, Vec<u8>), String> {
        let json = serde_json::to_vec(req).map_err(|e| e.to_string())?;
        let mut result = self.exchange(&json, bin);
        // Воркер не ответил ни разу в полной изоляции (например, AppContainer не может прочитать
        // свой exe в профиле пользователя) — одна попытка с ослабленной, уровень станет «частичная».
        if let Err(strict) = &result
            && !self.relaxed
            && self.level == Level::Full
            && self.running.as_ref().is_some_and(|r| !r.answered)
        {
            self.running = None;
            self.relaxed = true;
            result = self.exchange(&json, bin).map_err(|e| format!("{e}; в полной изоляции: {strict}"));
        }
        if result.is_err() {
            // Таймаут, падение или ошибка песочницы: процесс больше не используем.
            self.running = None;
        }
        let (json, bin) = result?;
        Ok((serde_json::from_slice(&json).map_err(|e| e.to_string())?, bin))
    }

    fn exchange(&mut self, json: &[u8], bin: &[u8]) -> Result<(Vec<u8>, Vec<u8>), String> {
        if self.running.is_none() {
            self.running = Some(self.start()?);
        }
        let r = self.running.as_mut().expect("запущен");
        write_msg(&mut r.stdin, json, bin).map_err(|e| format!("воркер недоступен: {e}"))?;
        match r.responses.recv_timeout(self.timeout) {
            Ok(Ok(msg)) => {
                r.answered = true;
                Ok(msg)
            }
            Ok(Err(_)) | Err(mpsc::RecvTimeoutError::Disconnected) => Err(match (r.exit_code)() {
                Some(code) => format!("воркер завершился, код {}", exit_name(code)),
                None => "воркер завершился".into(),
            }),
            Err(mpsc::RecvTimeoutError::Timeout) => Err(format!("воркер не ответил за {:?}", self.timeout)),
        }
    }

    fn start(&mut self) -> Result<Running, String> {
        let p = sandbox::spawn(&self.exe, self.relaxed).map_err(|e| format!("запуск {}: {e}", self.exe.display()))?;
        self.level = p.level;
        let (tx, rx) = mpsc::channel();
        let mut stdout = p.stdout;
        std::thread::Builder::new()
            .name("worker-reader".into())
            .spawn(move || {
                loop {
                    let r = read_msg(&mut stdout);
                    let stop = r.is_err();
                    if tx.send(r).is_err() || stop {
                        break;
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(Running { stdin: p.stdin, responses: rx, kill: p.kill, exit_code: p.exit_code, answered: false })
    }
}

/// Код завершения с расшифровкой частых NTSTATUS.
fn exit_name(code: u32) -> String {
    let name = match code {
        0xC000_0005 => " (нарушение доступа к памяти)",
        0xC000_0022 => " (нет доступа — вероятно, AppContainer не может прочитать файлы воркера)",
        0xC000_0135 => " (не найдена DLL)",
        0xC000_0139 => " (не найдена точка входа DLL)",
        0xC000_0142 => " (ошибка инициализации DLL)",
        0xC000_0409 => " (аварийное завершение / fail-fast)",
        0xC000_0602 => " (fail-fast)",
        0xC000_0374 => " (повреждение кучи)",
        0xC000_00FD => " (переполнение стека)",
        0xC000_0017 | 0xC000_012D => " (не хватило памяти)",
        _ => "",
    };
    if code > 0xFFFF { format!("0x{code:08X}{name}") } else { format!("{code}{name}") }
}
