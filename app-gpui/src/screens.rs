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
//! `letter` и `reply-inline` (S3-02, S5-06б) после первого `EmailsChanged` синхронизации, `calendar` и `event` — после недели с
//! сервера, `meeting` — после ответа о занятости участников из `POSTMAN_SCREENS_ATTENDEES` (адреса через `;`, S3-05),
//! `contacts` — после контактов и справочника с сервера (S4-02);
//! `vacation` и `rules` — после автоответа и правил с сервера (S4-05, S4-07), `shared` — «Входящие» общего ящика (S4-06);
//! `thread` (лента беседы, S5-05), `folder-menu`, `thread-stack`, `thread-list` (виды беседы, S5-05б), `lifecycle` и
//! `overdue` (сроки, S5-04), `search`, `search-letter`, `palette` и `palette-query` (поиск и палитра команд, S5-10),
//! `search-next` (F3 прокручивает к совпадению, S5-14) —
//! только демо;
//! вкладка «Настройки» (S5-09): `settings-general`, `settings-categories`, `settings-keys`, `settings-accounts` и первый
//! запуск `firstrun` — демо; `--live` без `POSTMAN_URL` — первый запуск (`firstrun`).

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use gpui_kit::*;
use postman_core::bench;

use crate::settings_tab::Section as SettingsSection;
use crate::{Mailer, Section, sys};

/// Кадров с выполненным условием до снимка: первые кадры после смены состояния ещё могут быть прежними.
const SETTLE_FRAMES: u32 = 3;
/// Снимок одного цвета (окно ещё не показано) — столько попыток; пауза перед второй, дальше — вдвое дольше
/// (50, 100, 200, 400, 800 мс: до ~1,5 с всего).
const CAPTURE_TRIES: u32 = 6;
const CAPTURE_BACKOFF: Duration = Duration::from_millis(50);
/// Сцена не готова за это время (воркер не ответил, страница не прочиталась) — ошибка.
const SCENE_TIMEOUT: Duration = Duration::from_secs(30);
/// Запрос с результатами в демо-ящике (S5-10): последнее слово первой темы демо («Отчёт за квартал») — письма во
/// «Входящих», «Архиве» и «Отправленных», в тексте писем Outlook — «итоги квартала».
fn search_query() -> &'static str {
    postman_demo::SUBJECTS.first().and_then(|s| s.rsplit(' ').next()).unwrap_or("pipeline")
}
/// Палитра с запросом: «от», набранное в английской раскладке, — команды, папки и письма (S5-10).
const PALETTE_QUERY: &str = "jn";

#[derive(Clone, Copy)]
enum Scene {
    /// Входящие, ничего не выбрано.
    List,
    /// Первое письмо выбрано, тело разобрано воркером.
    Letter,
    /// Письмо с вложениями и картинкой `cid:` в тексте (S3-03): лента 40 px с «+N» (S5-07).
    Attachments,
    /// Встроенный просмотр вложений того же письма (S5-07): фото, страница PDF, оглавление архива, книга с
    /// макросами (карточка «Открытие заблокировано»).
    PreviewImage,
    PreviewPdf,
    /// Та же страница PDF, прокрученная до низа (S5-07б): обрезанный прокруткой кадр не сдвинут.
    PreviewPdfEnd,
    PreviewZip,
    AttachMacro,
    /// Письмо с большой картинкой в тексте (S5-07б): кадр 1600 px, уменьшен до ширины области чтения, целиком.
    LetterImage,
    /// Письмо с приглашением: карточка встречи (S3-03), новое — с пересечением с моим днём (S5-11).
    Invite,
    /// Та же карточка: отвечено, изменено организатором, отменено, устарело (S5-11; подмена входов карточки).
    InviteAnswered,
    InviteUpdated,
    InviteCancelled,
    InviteOutdated,
    /// Приглашение боковой панелью справа (S5-24, «Настройки → Чтение»): строка над письмом, панель — рядом.
    InviteSide,
    /// Подписанные письма демо (S5-12 Ф1), карточка подписи раскрыта: подпись RSA верна (корпоративный УЦ), ГОСТ Р
    /// 34.10-2012 (проверено Postman, без СКЗИ), текст изменён после подписи, издатель не доверен.
    Smime,
    SmimeGost,
    SmimeBad,
    SmimeUntrusted,
    /// Шифрованное и подписанное письмо демо (S5-12 Ф2): расшифровано демо-ключом, «Зашифровано» и «Подписано»,
    /// карточка раскрыта (шифр и подпись), вложение — из расшифрованного письма.
    Encrypted,
    /// Шифровано не для меня: строка «не для вас» с издателем и номером сертификата.
    EncryptedOther,
    /// Тег GCM испорчен: красная строка «повреждено».
    EncryptedBad,
    /// Ключ на смарт-карте: строка «Нужен PIN · Ввести» (состояние подставлено — демо без карты).
    EncryptedPin,
    /// Поиск «Везде» (S5-10): результаты группами по папкам, подсветка, чипы «Уточнить».
    Search,
    /// Письмо из результатов поиска: подсветка в теме и тексте, строка «Совпадения».
    SearchLetter,
    /// То же письмо (S5-14): прокручено до низа, затем F3 по кругу до первого совпадения (в теме) — оно прокручено в
    /// вид, «Совпадение 1 из N». Готово, только если переход прокрутил письмо наверх.
    SearchNext,
    /// Палитра команд Ctrl+K с пустым запросом: недавние — первыми.
    Palette,
    /// Палитра с запросом в другой раскладке.
    PaletteQuery,
    /// Раздел «Календарь»: неделя и приглашение без ответа в подробностях (S5-11).
    Calendar,
    /// Вкладка встречи: помощник с занятостью каждого (без «Только общее свободное время»).
    Assistant,
    /// Календарь с подробностями самого людного события (S3-05).
    Event,
    /// Строка входа (нет учётной записи).
    SignIn,
    /// Новое письмо во вкладке (S5-06б): адресаты чипами (один внешний), текст, вложение, панель справа.
    Compose,
    /// То же письмо с открытым меню «Отправить позже» у кнопки «Отправить» (S5-06).
    SendLater,
    /// Второе письмо — вторая вкладка: все замечания «Перед отправкой» (карта, вложение, внешние, сменённый адрес).
    Checks,
    /// Ответ в области чтения над письмом с цитатой (S3-02, S5-06б).
    Reply,
    /// Вкладка новой встречи: поля и подбор времени в одной форме (S5-11); общие окна — розовым бренда (S5-20).
    Meeting,
    /// Та же встреча (S5-23): курсор ←/→ на втором общем окне — рамка плотнее и свечение.
    MeetingSlot,
    /// Та же встреча, цвет свободного времени — синий бренда («Настройки → Календарь», S5-20).
    MeetingBlue,
    /// Раздел «Контакты»: список и карточка первого контакта (S4-02).
    Contacts,
    /// Форма правки первого контакта (демо).
    ContactEdit,
    /// «Часто пишу» (S5-25): адресаты переписки по частоте, первая строка выбрана.
    ContactsFrequent,
    /// Модуль «Задачи» (S6-05): «Сегодня», карточка задачи с подзадачами и заметкой (демо).
    Tasks,
    /// «Сегодня» с выбранной просроченной задачей: «Просрочено» сверху, метка срока в карточке.
    TasksToday,
    /// Проект с прогрессом слева и его задачи.
    TasksProject,
    /// «Выполненные»: сегодня, вчера.
    TasksDone,
    /// Панель «Автоматические ответы» и полоса «Автоответ включён» (S4-05; демо — образец).
    Vacation,
    /// Панель «Правила» (S4-07): демо — образец с редактором первого правила, живой ящик — правила с сервера.
    Rules,
    /// «Входящие» общего ящика в дереве папок, первое письмо открыто (S4-06).
    Shared,
    /// Свои «Входящие» с полосами категорий и метками (S5-02): первому письму без меток клавишами 1 и 3 (как
    /// `mail.categorize`) поставлены «Клиенты» и «Тендеры», оно открыто.
    Labels,
    /// Фильтры (S5-03): панель фильтров на месте папок, «Все папки», «Непрочитанные» — чип, «N из M», группы
    /// по папкам.
    Filters,
    /// Быстрые фильтры (S5-03): закладки «Угрозы» и «Срок» включены (ИЛИ), панель по наведению открыта,
    /// группы по датам.
    Quick,
    /// Две вкладки почты (S5-16, Ctrl+T): главная на «Входящих», вторая — «Все папки» с «Непрочитанными», активна.
    TabMail,
    /// Лента беседы (S5-05): письмо цепочки демо с ответами из разных клиентов, только новый текст.
    Thread,
    /// Меню папки «Входящие» (S5-05б) с открытым подменю «Вид беседы».
    FolderMenu,
    /// Стопка беседы в панели чтения (S5-05б): старые письма свёрнуты строками, последнее и открытое раскрыты.
    ThreadStack,
    /// «Входящие» в виде «как Outlook» (S5-05б): беседа цепочки демо развёрнута, выбрано письмо под головой.
    ThreadList,
    /// Жизненный цикл (S5-04): «Входящие» со всеми метками сроков (срок впереди, сегодня к часу, ждём ответа,
    /// напомнит, исчезнет, просрочено — наверху, «Срочное») и открытая панель «Срок» у первого письма.
    Lifecycle,
    /// Письмо с просроченным сроком открыто: плотная янтарная метка в строке и в шапке.
    Overdue,
    /// Вкладка «Настройки», раздел «Общие» (S5-09).
    SettingsGeneral,
    /// «Настройки → Безопасность» (S5-30).
    SettingsSecurity,
    /// «Настройки → Внешний вид» (S5-30).
    SettingsLook,
    /// «Настройки → Категории» с правкой новой категории: палитра с перечёркнутыми близкими цветами.
    SettingsCategories,
    /// «Настройки → Клавиатура»: своё сочетание у одной команды и конфликт у другой.
    SettingsKeys,
    /// «Настройки → Аккаунты» с карточкой «Добавить аккаунт» на месте.
    SettingsAccounts,
    /// «Настройки → Быстрые фильтры и сроки».
    SettingsQuick,
    /// «Настройки → Чтение».
    SettingsReading,
    /// «Настройки → Календарь» (S5-20).
    SettingsCalendar,
    /// Тот же раздел, прокручен вниз: цвет свободного времени с образцом, календари коллег, приглашения.
    SettingsCalendarFree,
    /// «Настройки → Подписи и шаблоны» с образцом подписи и шаблона (демо — без файла).
    SettingsSign,
    /// Выделение (S5-08): три письма с флажками, панель массовых действий над списком, сводка в области чтения.
    Bulk,
    /// Контекстное меню над выделением с открытым подменю «Категория».
    ContextMenu,
    /// «Сообщить о фишинге»: тост «Отменить» после сообщения и меню у другого письма на пункте с красной маской.
    Phishing,
    /// Нет связи: строка состояния с обратным отсчётом, метка у аккаунта, полоса над списком.
    Offline,
    /// Стопка тостов (три и «+N ещё») рядом с открытой историей уведомлений.
    Toasts,
    /// KeyTips (S5-18): письмо открыто, Ctrl удержан 0,4 с — плашки с клавишами над кнопками (схема «Outlook»).
    KeyTips,
    /// То же в схеме «Одиночные клавиши»: открыто приглашение без ответа — A / T / N на его кнопках.
    KeyTipsSingle,
    /// Кнопки (S5-19): главная, тонированная и тонированная под указателем на каждой поверхности (`buttons::gallery`).
    Buttons,
}

const SCENES: &[Scene] = &[
    Scene::List,
    Scene::Letter,
    Scene::Attachments,
    Scene::PreviewImage,
    Scene::PreviewPdf,
    Scene::PreviewPdfEnd,
    Scene::PreviewZip,
    Scene::AttachMacro,
    Scene::LetterImage,
    Scene::Invite,
    Scene::InviteAnswered,
    Scene::InviteUpdated,
    Scene::InviteCancelled,
    Scene::InviteOutdated,
    Scene::InviteSide,
    Scene::Smime,
    Scene::SmimeGost,
    Scene::SmimeBad,
    Scene::SmimeUntrusted,
    Scene::Encrypted,
    Scene::EncryptedOther,
    Scene::EncryptedBad,
    Scene::EncryptedPin,
    Scene::Search,
    Scene::SearchLetter,
    Scene::SearchNext,
    Scene::Palette,
    Scene::PaletteQuery,
    Scene::Calendar,
    Scene::Event,
    Scene::Meeting,
    Scene::MeetingSlot,
    Scene::MeetingBlue,
    Scene::Assistant,
    Scene::Contacts,
    Scene::ContactEdit,
    Scene::ContactsFrequent,
    Scene::Tasks,
    Scene::TasksToday,
    Scene::TasksProject,
    Scene::TasksDone,
    Scene::Compose,
    Scene::SendLater,
    Scene::Checks,
    Scene::Reply,
    Scene::Vacation,
    Scene::Rules,
    Scene::SettingsGeneral,
    Scene::SettingsSecurity,
    Scene::SettingsLook,
    Scene::SettingsCategories,
    Scene::SettingsKeys,
    Scene::SettingsAccounts,
    Scene::SettingsQuick,
    Scene::SettingsReading,
    Scene::SettingsCalendar,
    Scene::SettingsCalendarFree,
    Scene::SettingsSign,
    Scene::Shared,
    Scene::Labels,
    Scene::Filters,
    Scene::Quick,
    Scene::TabMail,
    Scene::Lifecycle,
    Scene::Overdue,
    Scene::Bulk,
    Scene::ContextMenu,
    Scene::Phishing,
    Scene::Offline,
    Scene::Toasts,
    Scene::KeyTips,
    Scene::KeyTipsSingle,
    // Первый запуск (S5-09): добавление аккаунта на месте экрана почты (сроки прошлых сцен сняты).
    Scene::SignIn,
    // Последней из прежних: лента остаётся включённой, прежние сцены снимаются без неё.
    Scene::Thread,
    // Виды беседы (S5-05б): свой вид «Входящих» — только в этих сценах.
    Scene::FolderMenu,
    Scene::ThreadStack,
    Scene::ThreadList,
    Scene::Buttons,
];

/// Живой ящик: запрос для поиска заранее не известен; ответ — на первое письмо; календарь — неделя с сервера,
/// вкладка встречи — занятость с сервера для адресов из `POSTMAN_SCREENS_ATTENDEES` (S3-05).
const LIVE_SCENES: &[Scene] = &[
    Scene::List,
    Scene::Letter,
    Scene::Reply,
    Scene::Calendar,
    Scene::Event,
    Scene::Meeting,
    Scene::Contacts,
    Scene::Vacation,
    Scene::Rules,
    Scene::Shared,
];
const SIGNIN_SCENES: &[Scene] = &[Scene::SignIn];
/// Первая синхронизация живого ящика дольше сцены демо.
const LIVE_TIMEOUT: Duration = Duration::from_secs(120);

impl Scene {
    fn name(self) -> &'static str {
        match self {
            Scene::List => "list",
            Scene::Letter => "letter",
            Scene::Attachments => "attachments",
            Scene::PreviewImage => "preview-image",
            Scene::PreviewPdf => "preview-pdf",
            Scene::PreviewPdfEnd => "preview-pdf-end",
            Scene::LetterImage => "letter-image",
            Scene::PreviewZip => "preview-zip",
            Scene::AttachMacro => "attach-macro",
            Scene::Invite => "invite",
            Scene::InviteAnswered => "invite-answered",
            Scene::InviteUpdated => "invite-updated",
            Scene::InviteCancelled => "invite-cancelled",
            Scene::InviteOutdated => "invite-outdated",
            Scene::InviteSide => "invite-side",
            Scene::Smime => "smime",
            Scene::SmimeGost => "smime-gost",
            Scene::SmimeBad => "smime-bad",
            Scene::SmimeUntrusted => "smime-untrusted",
            Scene::Encrypted => "encrypted",
            Scene::EncryptedOther => "encrypted-other",
            Scene::EncryptedBad => "encrypted-bad",
            Scene::EncryptedPin => "encrypted-pin",
            Scene::Search => "search",
            Scene::SearchLetter => "search-letter",
            Scene::SearchNext => "search-next",
            Scene::Palette => "palette",
            Scene::PaletteQuery => "palette-query",
            Scene::Calendar => "calendar",
            Scene::Assistant => "assistant",
            Scene::Event => "event",
            Scene::SignIn => "firstrun",
            Scene::Compose => "compose",
            Scene::SendLater => "send-later",
            Scene::Checks => "checks",
            Scene::Reply => "reply-inline",
            Scene::Meeting => "meeting",
            Scene::MeetingSlot => "meeting-slot",
            Scene::MeetingBlue => "meeting-blue",
            Scene::Contacts => "contacts",
            Scene::ContactEdit => "contact-edit",
            Scene::ContactsFrequent => "contacts-frequent",
            Scene::Tasks => "tasks",
            Scene::TasksToday => "tasks-today",
            Scene::TasksProject => "tasks-project",
            Scene::TasksDone => "tasks-done",
            Scene::Vacation => "vacation",
            Scene::Rules => "rules",
            Scene::Shared => "shared",
            Scene::Labels => "labels",
            Scene::Filters => "filters",
            Scene::Quick => "quick",
            Scene::TabMail => "tab-mail",
            Scene::Thread => "thread",
            Scene::FolderMenu => "folder-menu",
            Scene::ThreadStack => "thread-stack",
            Scene::ThreadList => "thread-list",
            Scene::Lifecycle => "lifecycle",
            Scene::Overdue => "overdue",
            Scene::SettingsGeneral => "settings-general",
            Scene::SettingsSecurity => "settings-security",
            Scene::SettingsLook => "settings-look",
            Scene::SettingsCategories => "settings-categories",
            Scene::SettingsKeys => "settings-keys",
            Scene::SettingsAccounts => "settings-accounts",
            Scene::SettingsQuick => "settings-quick",
            Scene::SettingsReading => "settings-reading",
            Scene::SettingsCalendar => "settings-calendar",
            Scene::SettingsCalendarFree => "settings-calendar-free",
            Scene::SettingsSign => "settings-sign",
            Scene::Bulk => "bulk",
            Scene::ContextMenu => "context-menu",
            Scene::Phishing => "phishing",
            Scene::Offline => "offline",
            Scene::Toasts => "toasts",
            Scene::KeyTips => "keytips",
            Scene::KeyTipsSingle => "keytips-single",
            Scene::Buttons => "buttons",
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
    /// Последнее письмо до сцены (`checks`, `reply-inline`): готово, когда открылось следующее.
    composer: Option<u64>,
    /// Окончание имён файлов: `-dark` для `--theme dark` (S5-01), иначе пусто.
    suffix: &'static str,
    /// Сроки, поставленные сценой `lifecycle`: снять до следующих сцен.
    undo: Vec<postman_core::lifecycle::Patch>,
    /// Письмо с просроченным сроком (сцена `overdue`).
    overdue: Option<String>,
    /// Сцена `buttons`: поверх окна — образцы кнопок.
    gallery: bool,
}

impl Driver {
    /// Показать образцы кнопок (сцена `buttons`).
    pub(crate) fn gallery(&self) -> bool {
        self.gallery
    }

    /// Каталог создаётся сразу: ошибка — до открытия окна.
    pub fn new(dir: PathBuf, live: bool, suffix: &'static str) -> std::io::Result<Self> {
        std::fs::create_dir_all(&dir)?;
        let dir = std::path::absolute(&dir)?;
        Ok(Self { dir, scene: 0, phase: Phase::Enter, live, signin: false, composer: None, suffix, undo: Vec::new(), overdue: None, gallery: false })
    }

    /// Окно открывается со строкой входа: снимается она.
    pub fn sign_in(&mut self) {
        self.signin = true;
    }

    /// Сцены прогона; `POSTMAN_SCREENS_ONLY=bulk,offline` — только эти (отладка сцен; сцена, которой нужны
    /// прежние, без них может не собраться).
    fn scenes(&self) -> Vec<Scene> {
        let all: &[Scene] = match (self.live, self.signin) {
            (_, true) => SIGNIN_SCENES,
            (true, false) => LIVE_SCENES,
            (false, false) => SCENES,
        };
        match std::env::var("POSTMAN_SCREENS_ONLY") {
            Ok(only) if !only.trim().is_empty() => {
                all.iter().copied().filter(|s| only.split(',').any(|o| o.trim() == s.name())).collect()
            }
            _ => all.to_vec(),
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
            wait_step(m, scene, cx);
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
    // Вкладка почты сцены `tab-mail` — только в ней: дальше — главная со своим состоянием (как Ctrl+W).
    let mail_tab = m.tabs.iter().find(|t| t.kind == crate::tabs::TabKind::Mail).map(|t| t.id);
    if let Some(id) = mail_tab.filter(|_| !matches!(scene, Scene::TabMail)) {
        m.close_tab(id, window, cx);
    }
    // Образцы кнопок — только в своей сцене (S5-19).
    if let Some(d) = &mut m.screens {
        d.gallery = matches!(scene, Scene::Buttons);
    }
    // Боковая панель приглашения — только в своей сцене (S5-24).
    let side = matches!(scene, Scene::InviteSide);
    if crate::prefs::get(cx).reading.invite_side != side {
        crate::prefs::update(cx, |p| p.reading.invite_side = side);
    }
    // Плашки KeyTips — только в своих сценах: дальше Ctrl отпущен, схема — «Outlook».
    if crate::keytips::shown(cx) && !matches!(scene, Scene::KeyTips | Scene::KeyTipsSingle) {
        send_input(window, cx, PlatformInput::ModifiersChanged(ctrl(false)));
        crate::prefs::update(cx, |p| p.key_scheme = postman_core::prefs::KeyScheme::Outlook);
    }
    match scene {
        Scene::List => m.set_section(Section::Mail, cx),
        Scene::Letter => m.list.update(cx, |l, cx| l.select(0, cx)),
        Scene::Attachments
        | Scene::Invite
        | Scene::LetterImage
        | Scene::Smime
        | Scene::SmimeGost
        | Scene::SmimeBad
        | Scene::SmimeUntrusted
        | Scene::Encrypted
        | Scene::EncryptedOther
        | Scene::EncryptedBad
        | Scene::EncryptedPin => {
            let Some(i) = demo_row(m.list.read(cx), scene) else {
                return Err("no such message in the first rows of the demo inbox".into());
            };
            m.list.update(cx, |l, cx| l.select(i, cx));
        }
        Scene::PreviewImage | Scene::PreviewPdf | Scene::PreviewZip | Scene::AttachMacro => {
            // Как щелчок по карточке ленты.
            let blob = preview_blob(scene).to_string();
            m.reading.update(cx, |r, cx| r.toggle_preview(Some(blob), cx));
        }
        Scene::PreviewPdfEnd => m.reading.update(cx, |r, cx| {
            // Как прокрутка колесом до конца: смещение за край gpui прижимает к низу при раскладке.
            if let Some(p) = r.reading.as_ref().and_then(|r| r.preview.as_ref()) {
                p.scroll.set_offset(point(px(0.), px(-100_000.)));
            }
            cx.notify();
        }),
        Scene::InviteAnswered | Scene::InviteUpdated | Scene::InviteCancelled | Scene::InviteOutdated | Scene::InviteSide => {
            let which = scene.name().trim_start_matches("invite-");
            m.reading.update(cx, |r, cx| {
                if let Some(c) = r.reading.as_mut().and_then(|r| r.invite.as_mut()) {
                    c.screen_state(which);
                }
                cx.notify();
            });
        }
        Scene::Search => {
            // Как «Везде» и ввод запроса с Enter.
            m.bar.update(cx, |b, _| b.scope = crate::bar::Scope::All);
            m.search.update(cx, |s, cx| s.set_value(search_query(), window, cx));
            m.run_search(search_query(), cx);
        }
        Scene::SearchLetter => {
            let Some(i) = demo_row(m.list.read(cx), Scene::Attachments) else {
                return Err(format!("no Outlook letter among the results of \"{}\"", search_query()));
            };
            m.list.update(cx, |l, cx| l.select(i, cx));
        }
        // Письмо прокручено вниз, затем F3 (`search.next_match`) по кругу — к первому совпадению.
        Scene::SearchNext => m.reading.update(cx, |r, cx| {
            let bottom = r.scroll.max_offset().y;
            if bottom <= px(0.) {
                return Err("the letter fits the reading pane: nothing to scroll".to_string());
            }
            r.scroll.set_offset(point(px(0.), -bottom));
            for _ in 0..r.hits.get() {
                r.step_hit(1, cx);
            }
            Ok(())
        })?,
        Scene::Palette => {
            // Недавние — как после трёх запусков из палитры.
            m.recent = vec!["cmd:mail.due".into(), "folder:archive".into(), "cmd:mail.reply_all".into()];
            m.toggle_palette(window, cx);
        }
        Scene::PaletteQuery => {
            let Some(p) = m.palette.as_ref().map(|p| p.view.clone()) else { return Err("palette is not open".into()) };
            let input = p.read(cx).input.clone();
            input.update(cx, |s, cx| s.set_value(PALETTE_QUERY, window, cx));
            // Как набор: `set_value` не шлёт `Change`.
            p.update(cx, |p, cx| p.typed(cx));
        }
        Scene::Calendar => {
            m.close_palette(window, cx);
            // Как `search::Clear`: календарь снимается без следов поиска в папках.
            m.search.update(cx, |s, cx| s.set_value("", window, cx));
            m.run_search("", cx);
            m.set_section(Section::Calendar, cx);
            // «Сейчас» — 14:20 сегодня: линия времени видна при любом часе съёмки.
            let today = jiff::Zoned::now().date().at(14, 20, 0, 0);
            let clock = today.to_zoned(jiff::tz::TimeZone::system()).ok().map(|z| z.timestamp().as_second());
            m.cal.update(cx, |c, cx| {
                c.clock = clock;
                c.select_pending(cx);
            });
        }
        Scene::MeetingSlot => {
            // Как →, → с клавиатуры: курсор — на втором общем окне.
            let Some(v) = m.meetings.last() else { return Err("no meeting tab".into()) };
            v.update(cx, |v, cx| {
                v.step_cursor(true, cx);
                v.step_cursor(true, cx);
            });
        }
        Scene::MeetingBlue => crate::prefs::update(cx, |p| p.calendar.free_color = postman_core::prefs::FreeColor::Blue),
        Scene::Assistant => {
            // Цвет свободного времени — снова по умолчанию (после `meeting-blue`), курсора окон нет — как на макете.
            crate::prefs::update(cx, |p| p.calendar.free_color = postman_core::prefs::FreeColor::default());
            let Some(v) = m.meetings.last() else { return Err("no meeting tab".into()) };
            v.update(cx, |v, cx| {
                v.only_free = false;
                v.slot_cursor = None;
                cx.notify();
            });
        }
        Scene::Event => {
            m.cal.update(cx, |c, cx| c.select_busiest(cx));
            m.set_section(Section::Calendar, cx);
        }
        Scene::Lifecycle => lifecycle_scene(m, cx)?,
        Scene::Overdue => {
            m.close_picker(cx);
            let id = m.screens.as_ref().and_then(|d| d.overdue.clone());
            let row = id.and_then(|id| m.cache.rows_by_ids(&[id]).ok()).and_then(|mut r| r.pop());
            let Some(row) = row else { return Err("no overdue letter".into()) };
            m.list.update(cx, |l, cx| l.select(0, cx));
            m.open_letter(row, cx);
        }
        Scene::Bulk => {
            m.set_section(Section::Mail, cx);
            m.reset_filters(cx);
            m.filters.update(cx, |f, cx| f.set(cx, |f| f.quick_open = false));
            m.close_picker(cx);
            m.reading.update(cx, |r, cx| r.clear(cx));
            let inbox = crate::live::inbox(&m.folders.read(cx).mailboxes).ok_or("no inbox")?;
            m.open_mailbox(inbox, cx);
            // Как Ctrl+щелчки по второй, третьей и пятой строкам.
            m.list.update(cx, |l, cx| {
                for i in [1, 2, 4] {
                    l.pick(i, false, cx);
                }
            });
        }
        Scene::ContextMenu => {
            // Как правая кнопка по третьей строке (она в выделении), затем наведение на «Категория».
            m.open_menu_at_row(2, None, cx);
            m.screen_menu_sub(crate::menu::Sub::Category);
        }
        Scene::Phishing => {
            m.close_menu(cx);
            m.list.update(cx, |l, cx| {
                l.clear_selection(cx);
            });
            // Как «Сообщить о фишинге» у первого письма (план — тот же, что у действия; «Отменить» — после снимков).
            let rows: Vec<postman_cache::EmailRow> = m.list.read(cx).rows.1.iter().take(2).cloned().collect();
            let Some(first) = rows.first().cloned() else { return Err("no letters in the inbox".into()) };
            let boxes = m.folders.read(cx).mailboxes.clone();
            let act = postman_core::bulk::Act::Phishing;
            let plan = postman_core::bulk::plan(std::slice::from_ref(&first), &act, &boxes, |_, _| true);
            if let Some(d) = &mut m.screens {
                d.undo.extend(plan.undo.clone());
            }
            m.bulk_planned(act, plan, 1, cx);
            // …и меню у второго письма на пункте «Сообщить о фишинге».
            m.open_menu_at_row(1, None, cx);
            m.screen_menu_last();
        }
        Scene::Offline => {
            m.close_menu(cx);
            m.toasts.update(cx, |t, cx| t.hide_all(cx));
            let own = postman_demo::ME.to_owned();
            m.folders.update(cx, |f, cx| {
                f.own = Some(own);
                cx.notify();
            });
            // Как событие синхронизации: связи нет, повтор через 25 с, две правки в очереди.
            m.on_connection(postman_core::link::Connection::Offline { retry_in: 25, queued: 2 }, cx);
        }
        Scene::KeyTips => {
            if m.toasts.read(cx).is_history_open() {
                m.toasts.update(cx, |t, cx| t.toggle_history(cx));
            }
            m.set_section(Section::Mail, cx);
            m.open_mailbox("inbox".into(), cx);
            m.list.update(cx, |l, cx| l.select(0, cx));
            // Как удержание Ctrl: событие окна, таймер экрана почты (0,4 с).
            send_input(window, cx, PlatformInput::ModifiersChanged(ctrl(true)));
        }
        Scene::Buttons => m.set_section(Section::Mail, cx),
        Scene::KeyTipsSingle => {
            crate::prefs::update(cx, |p| p.key_scheme = postman_core::prefs::KeyScheme::Single);
            let Some(i) = demo_row(m.list.read(cx), Scene::Invite) else { return Err("no invitation in the demo inbox".into()) };
            m.list.update(cx, |l, cx| l.select(i, cx));
        }
        Scene::Toasts => {
            // Связь вернулась (запись в истории), затем пять уведомлений: видно три и «+2 ещё»; история открыта.
            m.on_connection(postman_core::link::Connection::Online, cx);
            m.folders.update(cx, |f, cx| {
                f.own = None;
                cx.notify();
            });
            use pets_ui::{Level, ToastAction};
            let undo = || ToastAction::new(pets_i18n::tr!("bulk-undo"), |_, _| {});
            let weak = crate::reading::sandbox_label(postman_worker::Level::Partial);
            m.toasts.update(cx, |t, cx| {
                t.push_action(Level::Success, pets_i18n::tr!("bulk-done-archive", n = 3), undo(), cx);
                t.push(Level::Info, pets_i18n::tr!("bulk-done-read", n = 12), cx);
                t.push(Level::Error, pets_i18n::tr!("bulk-phishing-failed", error = String::from("552")), cx);
                t.push(Level::Warning, pets_i18n::tr!("sandbox-weak", level = weak), cx);
                t.push_action(Level::Success, pets_i18n::tr!("bulk-done-archive", n = 14), undo(), cx);
                if !t.is_history_open() {
                    t.toggle_history(cx);
                }
            });
        }
        Scene::SignIn => {
            // Уведомления сцены `toasts` — убрать: первый запуск без них.
            m.toasts.update(cx, |t, cx| {
                t.hide_all(cx);
                if t.is_history_open() {
                    t.toggle_history(cx);
                }
            });
            // Сроки сцены `lifecycle` — снять: дальше снимки без них.
            let undo = m.screens.as_mut().map(|d| std::mem::take(&mut d.undo)).unwrap_or_default();
            m.apply_patches(undo, cx);
            m.close_settings(window, cx);
            m.set_section(Section::Mail, cx);
            // Демо: первый запуск без учётной записи — так же, как без `POSTMAN_URL` и сохранённого входа.
            if m.live.signin.is_none() {
                m.show_signin(None, window, cx);
            }
        }
        Scene::Compose => {
            m.cal.update(cx, |c, cx| c.close_details(cx));
            m.set_section(Section::Mail, cx);
            // Как `mail.new`: письмо — вкладкой (поля ввода создаются после кадра).
            let mut draft = crate::composer::scenes::compose_draft();
            draft.attachments.push(crate::composer::scenes::sample_attachment());
            m.open_compose(draft, cx);
        }
        Scene::SendLater => {
            let Some(c) = m.compose.last().cloned() else { return Err("no compose tab".into()) };
            // «Ждать ответа» включено — письмо изменено: у вкладки точка несохранённого.
            c.update(cx, |c, cx| {
                c.model.wait = Some(postman_core::marks::WAIT_DEFAULT);
                c.toggle_later_menu(cx);
            });
        }
        Scene::Checks => {
            remember_composer(m, cx);
            m.load_address_book(cx);
            crate::composer::scenes::seed_book(&m.compose.book);
            m.open_compose(crate::composer::scenes::checks_draft(), cx);
        }
        Scene::Reply => {
            // Демо — письмо с картинкой в тексте и цитатой Outlook; живой ящик — первое письмо.
            let live = m.screens.as_ref().is_some_and(|d| d.live);
            let i = if live { Some(0) } else { demo_row(m.list.read(cx), Scene::Attachments) };
            let Some(i) = i else { return Err("no message to reply to".into()) };
            remember_composer(m, cx);
            m.tabs.activate_kind(crate::tabs::TabKind::Main);
            m.set_section(Section::Mail, cx);
            m.list.update(cx, |l, cx| l.select(i, cx));
            // Как `mail.reply`: ответ — когда тело разобрано (ответ ждёт его сам), и не посреди отрисовки.
            let mailer = cx.entity();
            cx.defer(move |cx| mailer.update(cx, |m, cx| m.respond(postman_compose::Mode::Reply, cx)));
        }
        Scene::Contacts => {
            m.cal.update(cx, |c, cx| c.close_details(cx));
            m.close_all_meetings(window, cx);
            m.set_section(Section::Contacts, cx);
            let live = m.screens.as_ref().is_some_and(|d| d.live);
            m.contacts.update(cx, |c, cx| {
                c.select_first(cx);
                // Демо: коллега, чья занятость есть в демо-неделе, — видна полоса «Сегодня».
                if !live {
                    c.select_email("m.ivanova@corp.example", cx);
                }
            });
        }
        Scene::ContactEdit => m.contacts.update(cx, |c, cx| c.edit_selected(window, cx)),
        Scene::ContactsFrequent => {
            // Из правки — назад к карточке; «Часто пишу» — из адресной книги переписки (собирается в фоне).
            m.contacts.update(cx, |c, cx| c.escape(window, cx));
            m.load_address_book(cx);
            m.contacts_frequent(cx);
            m.contacts.update(cx, |c, cx| c.show_frequent(cx));
        }
        Scene::Tasks | Scene::TasksToday | Scene::TasksProject | Scene::TasksDone => {
            use postman_core::tasks::List;
            m.set_section(Section::Tasks, cx);
            m.tasks.update(cx, |t, cx| match scene {
                Scene::TasksProject => t.set_nav(crate::tasks::Nav::Project(postman_demo::tasks::PROJECT.into()), cx),
                Scene::TasksDone => t.set_nav(crate::tasks::Nav::List(List::Done), cx),
                _ => {
                    t.set_nav(crate::tasks::Nav::List(List::Today), cx);
                    let title = if matches!(scene, Scene::Tasks) { postman_demo::tasks::MAIN } else { postman_demo::tasks::OVERDUE };
                    t.select_title(title, cx);
                }
            });
        }
        Scene::Shared => {
            m.set_section(Section::Mail, cx);
            let Some(id) = shared_inbox(m, cx) else { return Err("no shared mailbox in the folder tree".into()) };
            m.open_mailbox(id, cx);
            m.list.update(cx, |l, cx| {
                if l.count > 0 {
                    l.select(0, cx);
                }
            });
        }
        Scene::Labels => {
            m.set_section(Section::Mail, cx);
            let inbox = crate::live::inbox(&m.folders.read(cx).mailboxes);
            let Some(id) = inbox else { return Err("no inbox".into()) };
            m.open_mailbox(id, cx);
            let list = m.list.read(cx);
            let (offset, rows) = &list.rows;
            let plain = rows.iter().position(|r| crate::labels::row_labels(&list.labels, r).chips.is_empty());
            let Some(i) = plain.map(|p| p + offset) else {
                return Err("no message without labels in the first rows of the demo inbox".into());
            };
            m.list.update(cx, |l, cx| {
                l.select(i, cx);
                l.toggle_category(1, cx);
                l.toggle_category(3, cx);
            });
        }
        Scene::Filters => {
            use postman_core::filters::Check;
            m.set_section(Section::Mail, cx);
            let inbox = crate::live::inbox(&m.folders.read(cx).mailboxes);
            let Some(id) = inbox else { return Err("no inbox".into()) };
            m.open_mailbox(id, cx);
            m.filters.update(cx, |f, cx| {
                f.set(cx, |f| f.panel = true);
                f.change(cx, |q| {
                    q.scope = crate::filters::Where::All;
                    q.criteria.toggle_check(Check::Unread);
                });
            });
        }
        Scene::Quick => {
            use postman_core::filters::Quick;
            m.filters.update(cx, |f, cx| {
                f.set(cx, |f| f.panel = false);
                f.change(cx, |q| {
                    q.scope = crate::filters::Where::Folder;
                    q.criteria.clear_panel();
                    q.criteria.toggle_quick(Quick::Threats);
                    q.criteria.toggle_quick(Quick::Due);
                });
            });
            // Панель закладок — как по наведению.
            m.filters.update(cx, |f, cx| f.set(cx, |f| f.quick_open = true));
        }
        Scene::TabMail => {
            use postman_core::filters::Check;
            m.set_section(Section::Mail, cx);
            m.reset_filters(cx);
            m.filters.update(cx, |f, cx| f.set(cx, |f| f.quick_open = false));
            let inbox = crate::live::inbox(&m.folders.read(cx).mailboxes);
            let Some(id) = inbox else { return Err("no inbox".into()) };
            m.open_mailbox(id.clone(), cx);
            // Как Ctrl+T: вторая вкладка на той же папке; в ней — «Все папки» и «Непрочитанные».
            m.new_mail_tab(window, cx);
            m.open_mailbox(id, cx);
            m.filters.update(cx, |f, cx| {
                f.change(cx, |q| {
                    q.scope = crate::filters::Where::All;
                    q.criteria.toggle_check(Check::Unread);
                })
            });
        }
        Scene::Thread => {
            m.live.signin = None;
            m.set_section(Section::Mail, cx);
            // Без фильтров прошлых сцен.
            m.reset_filters(cx);
            m.filters.update(cx, |f, cx| f.set(cx, |f| f.quick_open = false));
            m.open_mailbox("inbox".into(), cx);
            let row = m.cache.rows_by_ids(&[postman_demo::CHAIN_OPEN.into()]).map_err(|e| e.to_string())?.pop();
            let Some(row) = row else { return Err("no demo conversation in the cache".into()) };
            m.reading.update(cx, |r, _| r.feed_on = true);
            m.open_letter(row, cx);
        }
        Scene::FolderMenu => {
            // Как правая кнопка по «Входящим» и наведение на «Вид беседы».
            m.reading.update(cx, |r, _| r.feed_on = false);
            m.screen_folder_menu("inbox".into(), point(px(150.), px(140.)), cx);
        }
        Scene::ThreadStack | Scene::ThreadList => {
            use postman_core::prefs::Conversation;
            m.close_folder_menu(cx);
            let view = if matches!(scene, Scene::ThreadStack) { Conversation::Stack } else { Conversation::Outlook };
            // Как выбор в меню папки.
            crate::prefs::update(cx, |p| p.reading.set_folder("inbox", Some(view)));
            m.open_mailbox("inbox".into(), cx);
            if matches!(scene, Scene::ThreadStack) {
                let row = m.cache.rows_by_ids(&[postman_demo::CHAIN_OPEN.into()]).map_err(|e| e.to_string())?.pop();
                let Some(row) = row else { return Err("no demo conversation in the cache".into()) };
                m.open_letter(row, cx);
            } else {
                // Как щелчок по стрелке беседы; письмо под головой выбирается, когда беседы посчитаны (`wait_step`).
                m.list.update(cx, |l, _| l.threads_open.insert("chain".into()));
            }
        }
        Scene::Meeting => {
            m.set_section(Section::Calendar, cx);
            m.open_meeting_screen(cx);
        }
        Scene::Vacation => {
            m.set_section(Section::Mail, cx);
            if !m.backend.is_live() {
                m.set_vacation_state(Some(crate::settings::screen_sample()), cx);
                // Строка состояния — с заполненностью ящика: 12 % из 2 ГБ.
                let limit: u64 = 2 << 30;
                m.set_quota(Some(postman_core::settings::Usage { used: limit / 8, limit }), cx);
            }
            m.open_vacation(window, cx);
        }
        Scene::Rules => {
            m.open_rules(window, cx);
            // Демо: редактор первого правила открыт — снимок показывает и список, и редактор.
            if let (false, Some(p)) = (m.backend.is_live(), m.settings.rules_panel.clone()) {
                p.update(cx, |p, cx| {
                    let first = p.state.as_ref().and_then(|s| s.rules.first().cloned());
                    if let Some(r) = first {
                        p.edit(crate::rules::Draft::of(0, &r), cx);
                    }
                });
            }
        }
        Scene::SettingsGeneral => m.open_settings(Some(SettingsSection::General), window, cx),
        Scene::SettingsSecurity => m.open_settings(Some(SettingsSection::Security), window, cx),
        Scene::SettingsLook => m.open_settings(Some(SettingsSection::Look), window, cx),
        Scene::SettingsCategories => {
            m.open_settings(Some(SettingsSection::Categories), window, cx);
            if let Some(v) = m.settings.view.clone() {
                v.update(cx, |v, cx| v.edit_category(None, window, cx));
            }
        }
        Scene::SettingsKeys => {
            // Своё сочетание у «Флаг» и конфликт: «Сегодня в календаре» на Ctrl+2, как «Календарь»; схема «Одиночные
            // клавиши» (S5-17) со своей буквой у «Архив».
            crate::prefs::update(cx, |p| {
                p.keys.insert("mail.toggle_flag".into(), vec!["ctrl-g".into()]);
                p.keys.insert("calendar.today".into(), vec!["ctrl-2".into()]);
                p.key_scheme = postman_core::prefs::KeyScheme::Single;
                p.keys_single.insert("mail.archive".into(), vec!["y".into()]);
            });
            m.open_settings(Some(SettingsSection::Keys), window, cx);
        }
        Scene::SettingsAccounts => {
            crate::prefs::update(cx, |p| {
                p.keys.clear();
                p.keys_single.clear();
                p.key_scheme = postman_core::prefs::KeyScheme::Outlook;
            });
            m.open_settings(Some(SettingsSection::Accounts), window, cx);
            m.on_settings(&crate::settings_tab::SettingsEvent::AddAccount, window, cx);
        }
        Scene::SettingsQuick => m.open_settings(Some(SettingsSection::Quick), window, cx),
        Scene::SettingsReading => m.open_settings(Some(SettingsSection::Reading), window, cx),
        Scene::SettingsCalendar => m.open_settings(Some(SettingsSection::Calendar), window, cx),
        Scene::SettingsCalendarFree => {
            m.open_settings(Some(SettingsSection::Calendar), window, cx);
            if let Some(v) = m.settings.view.clone() {
                v.update(cx, |v, cx| {
                    v.scroll.scroll_to_bottom();
                    cx.notify();
                });
            }
        }
        Scene::SettingsSign => {
            m.open_settings(Some(SettingsSection::Signatures), window, cx);
            if let Some(v) = m.settings.view.clone() {
                v.update(cx, |v, cx| {
                    v.sign.prefs = crate::settings_sign::screen_sample(&v.info.identities);
                    cx.notify();
                });
            }
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
        Scene::Attachments | Scene::Invite | Scene::LetterImage => {
            let target = demo_row(list, scene).and_then(|i| list.row(i)).map(|r| r.id.as_str());
            let shown = |r: &crate::reading::Reading| match scene {
                Scene::Attachments | Scene::LetterImage => r.doc.is_some() && !r.images.is_empty(),
                _ => r.doc.is_some() && r.invite.as_ref().is_some_and(|c| c.invitation().is_some() && c.day.is_some() && c.clock.is_some()),
            };
            target.is_some() && reading.is_some_and(|r| Some(r.row.id.as_str()) == target && shown(r))
        }
        Scene::InviteAnswered | Scene::InviteUpdated | Scene::InviteCancelled | Scene::InviteOutdated => {
            reading.and_then(|r| r.invite.as_ref()).is_some_and(|c| c.clock.is_some())
        }
        Scene::InviteSide => {
            crate::prefs::get(cx).reading.invite_side && reading.and_then(|r| r.invite.as_ref()).is_some_and(|c| c.clock.is_some())
        }
        Scene::Encrypted | Scene::EncryptedOther | Scene::EncryptedBad | Scene::EncryptedPin => {
            use crate::smime_enc::{Lock, Problem};
            let target = demo_row(list, scene).and_then(|i| list.row(i)).map(|r| r.id.as_str());
            reading.is_some_and(|r| {
                let s = r.smime.as_ref();
                let lock = s.and_then(|s| s.lock.as_ref());
                let ok = match scene {
                    Scene::Encrypted => {
                        matches!(lock, Some(Lock::Open(_))) && s.is_some_and(|s| s.open && matches!(s.seal, crate::smime::Seal::Done(_)))
                    }
                    Scene::EncryptedOther => matches!(lock, Some(Lock::Failed(Problem::NotForYou(_)))),
                    Scene::EncryptedBad => matches!(lock, Some(Lock::Failed(Problem::Damaged))),
                    _ => matches!(lock, Some(Lock::Failed(Problem::NeedPin))),
                };
                Some(r.row.id.as_str()) == target && ok
            })
        }
        Scene::Smime | Scene::SmimeGost | Scene::SmimeBad | Scene::SmimeUntrusted => {
            // Подпись проверена воркером, карточка раскрыта, тело разобрано.
            let target = demo_row(list, scene).and_then(|i| list.row(i)).map(|r| r.id.as_str());
            reading.is_some_and(|r| {
                let done = r.smime.as_ref().is_some_and(|s| s.open && matches!(s.seal, crate::smime::Seal::Done(_)));
                Some(r.row.id.as_str()) == target && r.doc.is_some() && done
            })
        }
        Scene::PreviewImage | Scene::PreviewPdf | Scene::PreviewZip | Scene::AttachMacro => {
            // Просмотр разобран воркером; у книги с макросами — и проверка для щитка в ленте.
            let blob = preview_blob(scene);
            reading.is_some_and(|r| {
                let ready = r.preview.as_ref().is_some_and(|p| p.row.blob_id == blob && p.ready());
                ready && (!matches!(scene, Scene::AttachMacro) || r.scans.contains_key(blob))
            })
        }
        Scene::PreviewPdfEnd => reading.and_then(|r| r.preview.as_ref()).is_some_and(|p| {
            // Прокручено до низа и страница не перерисовывается.
            let (offset, max) = (p.scroll.offset().y, p.scroll.max_offset().y);
            p.ready() && max > px(0.) && (offset + max).abs() < px(1.)
        }),
        Scene::Search => list.found.as_ref().is_some_and(|f| !f.chips.is_empty()) && rows_loaded,
        Scene::SearchLetter => {
            let selected = list.selected.and_then(|i| list.row(i)).map(|r| r.id.as_str());
            let marked = m.reading.read(cx).marks.is_some();
            marked && selected.is_some() && reading.is_some_and(|r| r.doc.is_some() && Some(r.row.id.as_str()) == selected)
        }
        Scene::SearchNext => {
            let r = m.reading.read(cx);
            r.hits.get() > 0 && r.hit == 0 && r.scroll.offset().y.abs() < px(1.)
        }
        Scene::Palette => m.palette.as_ref().is_some_and(|p| p.view.read(cx).shown() > 0),
        Scene::PaletteQuery => m.palette.as_ref().is_some_and(|p| p.view.read(cx).has_letters()),
        Scene::Calendar => {
            let c = m.cal.read(cx);
            // Живой ящик: неделя пришла с сервера.
            let loaded = c.loaded || m.screens.as_ref().is_some_and(|d| !d.live);
            m.section == Section::Calendar && c.clock.is_some() && loaded
        }
        Scene::Meeting | Scene::MeetingSlot | Scene::MeetingBlue | Scene::Assistant => {
            let tab = m.tabs.is_active(crate::tabs::TabKind::Meeting);
            let blue = crate::prefs::get(cx).calendar.free_color == postman_core::prefs::FreeColor::Blue;
            tab && blue == matches!(scene, Scene::MeetingBlue)
                && m.meetings.last().is_some_and(|v| {
                    let v = v.read(cx);
                    v.ready()
                        && (v.only_free == matches!(scene, Scene::Meeting | Scene::MeetingSlot | Scene::MeetingBlue))
                        && (!matches!(scene, Scene::MeetingSlot) || v.slot_cursor.is_some())
                })
        }
        Scene::Event => {
            // Живая неделя может быть пустой: тогда снимок — без подробностей.
            let c = m.cal.read(cx);
            m.section == Section::Calendar && (c.selected() || c.is_empty())
        }
        Scene::SignIn => m.live.signin.is_some() && !m.tabs.is_active(crate::tabs::TabKind::Settings),
        Scene::SettingsGeneral
        | Scene::SettingsSecurity
        | Scene::SettingsLook
        | Scene::SettingsCategories
        | Scene::SettingsKeys
        | Scene::SettingsAccounts
        | Scene::SettingsQuick
        | Scene::SettingsReading
        | Scene::SettingsCalendar
        | Scene::SettingsCalendarFree
        | Scene::SettingsSign => {
            let want = match scene {
                Scene::SettingsGeneral => SettingsSection::General,
                Scene::SettingsSecurity => SettingsSection::Security,
                Scene::SettingsLook => SettingsSection::Look,
                Scene::SettingsCategories => SettingsSection::Categories,
                Scene::SettingsKeys => SettingsSection::Keys,
                Scene::SettingsQuick => SettingsSection::Quick,
                Scene::SettingsReading => SettingsSection::Reading,
                Scene::SettingsCalendar | Scene::SettingsCalendarFree => SettingsSection::Calendar,
                Scene::SettingsSign => SettingsSection::Signatures,
                _ => SettingsSection::Accounts,
            };
            settings_on(m, want, cx)
                && m.settings.view.as_ref().is_some_and(|v| {
                    let v = v.read(cx);
                    match scene {
                        Scene::SettingsCategories => v.cat_edit.is_some(),
                        Scene::SettingsAccounts => v.add.is_some(),
                        Scene::SettingsSign => !v.sign.prefs.signatures.is_empty(),
                        _ => true,
                    }
                })
        }
        Scene::Compose => compose_tab(m, cx).is_some_and(|c| c.read(cx).drawn()),
        Scene::SendLater => compose_tab(m, cx).is_some_and(|c| c.read(cx).drawn() && c.read(cx).later_menu_open()),
        Scene::Checks => {
            // Новое письмо — вторая вкладка; замечания по адресатам считаются при отрисовке.
            let before = m.screens.as_ref().and_then(|d| d.composer);
            compose_tab(m, cx).is_some_and(|c| {
                let c = c.read(cx);
                Some(c.id) != before && c.drawn() && c.model.checks().count() >= 4
            })
        }
        Scene::Reply => {
            // Ответ — в области чтения над письмом (вкладки прошлых сцен ещё открыты).
            let before = m.screens.as_ref().and_then(|d| d.composer);
            let reply = m.compose.inline_view().is_some_and(|c| Some(c.read(cx).id) != before && c.read(cx).drawn());
            reply && m.tabs.active().kind.shows_mail()
        }
        Scene::Contacts => {
            // Живой ящик: контакты с сервера пришли — выделить первый (до ответа выделять нечего).
            let c = m.contacts.read(cx);
            m.section == Section::Contacts && c.ready()
        }
        Scene::ContactEdit => m.section == Section::Contacts && m.contacts.read(cx).editing(),
        Scene::ContactsFrequent => m.contacts.read(cx).frequent_shown() && m.contacts.read(cx).ready(),
        Scene::Tasks | Scene::TasksToday | Scene::TasksProject | Scene::TasksDone => {
            m.section == Section::Tasks && m.tasks.read(cx).ready()
        }
        Scene::Vacation => {
            // Живой ящик: автоответ прочитан с сервера (полоса — только если он включён).
            let live = m.screens.as_ref().is_some_and(|d| d.live);
            let state = if live { m.settings.vacation.is_some() } else { m.settings.bar.is_some() };
            settings_on(m, SettingsSection::Vacation, cx) && m.settings.vacation_panel.is_some() && state
        }
        Scene::Rules => {
            let live = m.screens.as_ref().is_some_and(|d| d.live);
            match &m.settings.rules_panel {
                Some(p) if settings_on(m, SettingsSection::Rules, cx) => {
                    let p = p.read(cx);
                    if live { p.state.is_some() } else { p.editor.is_some() }
                }
                _ => false,
            }
        }
        Scene::Labels => {
            let row = list.selected.and_then(|i| list.row(i));
            let labeled = row.is_some_and(|r| crate::labels::row_labels(&list.labels, r).chips.len() == 2);
            let id = row.map(|r| r.id.as_str());
            labeled
                && list.mailbox == crate::live::inbox(&m.folders.read(cx).mailboxes)
                && reading.is_some_and(|r| r.doc.is_some() && Some(r.row.id.as_str()) == id)
        }
        Scene::Filters => {
            let f = m.filters.read(cx);
            f.panel
                && list.query.scope == crate::filters::Where::All
                && !list.query.criteria.is_empty()
                && list.view.is_some()
                && !list.computing
                && f.counts.total > 0
                && rows_loaded
        }
        Scene::TabMail => {
            m.tabs.is_active(crate::tabs::TabKind::Mail)
                && m.tabs.iter().filter(|t| t.kind == crate::tabs::TabKind::Mail).count() == 1
                && m.section == Section::Mail
                && list.query.scope == crate::filters::Where::All
                && !list.query.criteria.is_empty()
                && list.view.is_some()
                && !list.computing
                && rows_loaded
        }
        Scene::Quick => {
            let f = m.filters.read(cx);
            !f.panel
                && list.query.scope == crate::filters::Where::Folder
                && list.query.criteria.quick.len() == 2
                && list.view.is_some()
                && !list.computing
                && f.quick_open
                && (rows_loaded || list.count == 0)
        }
        Scene::Lifecycle | Scene::Overdue => {
            let now = jiff::Zoned::now();
            let (_, rows) = &list.rows;
            let marked = rows.iter().filter(|r| !crate::lifecycle::row_badges(r, &now).is_empty()).count();
            let want = m.screens.as_ref().and_then(|d| d.overdue.as_deref());
            let top = list.row(0).map(|r| r.id.as_str()) == want;
            let open = reading.is_some_and(|r| r.doc.is_some() && (matches!(scene, Scene::Lifecycle) || Some(r.row.id.as_str()) == want));
            let picker = m.lifecycle.picker.is_some() == matches!(scene, Scene::Lifecycle);
            marked >= 6 && top && list.view.is_some() && !list.computing && rows_loaded && open && picker
        }
        Scene::Bulk => {
            let s = list.summary.as_ref().map(|s| s.summary.count);
            m.section == Section::Mail && list.selection.len() == 3 && s == Some(3) && rows_loaded
        }
        Scene::ContextMenu | Scene::Phishing => m.menu.is_some() && rows_loaded,
        Scene::Offline => m.offline.is_down() && list.offline.is_some(),
        Scene::Toasts => m.toasts.read(cx).is_history_open() && !m.offline.is_down(),
        Scene::KeyTips => crate::keytips::shown(cx) && reading.is_some_and(|r| r.doc.is_some()),
        Scene::Buttons => m.screens.as_ref().is_some_and(|d| d.gallery),
        Scene::KeyTipsSingle => {
            let single = crate::prefs::get(cx).key_scheme == postman_core::prefs::KeyScheme::Single;
            crate::keytips::shown(cx) && single && reading.is_some_and(|r| r.doc.is_some() && r.invite.is_some())
        }
        Scene::Thread => {
            let r = m.reading.read(cx);
            let open = r.reading.as_ref().is_some_and(|r| r.row.id == postman_demo::CHAIN_OPEN);
            open && r.feed_on && r.feed.as_ref().is_some_and(|f| f.conv.len() == postman_demo::CHAIN.len() && f.ready())
        }
        Scene::FolderMenu => m.folder_menu.is_some(),
        Scene::ThreadStack => {
            let r = m.reading.read(cx);
            let open = r.reading.as_ref().is_some_and(|r| r.row.id == postman_demo::CHAIN_OPEN && r.doc.is_some());
            open && r.stacked(cx) && r.feed.as_ref().is_some_and(|f| f.conv.len() == postman_demo::CHAIN.len() && f.ready())
        }
        Scene::ThreadList => {
            let open = reading.is_some_and(|r| r.row.id == THREAD_CHILD && r.doc.is_some());
            let picked = list.selected.and_then(|i| list.row(i)).is_some_and(|r| r.id == THREAD_CHILD);
            list.outline.is_some() && !list.computing && rows_loaded && picked && open
        }
        Scene::Shared => {
            // Живой общий ящик может быть пуст: тогда снимок — без письма.
            let want = shared_inbox(m, cx);
            let opened = want.is_some() && list.mailbox == want && list.search_ids.is_none();
            let first = list.row(0).map(|r| r.id.as_str());
            let letter = rows_loaded && reading.is_some_and(|r| r.doc.is_some() && Some(r.row.id.as_str()) == first);
            m.section == Section::Mail && opened && (list.count == 0 || letter)
        }
    }
}

/// Письмо под головой беседы демо во «Входящих» (сцена `thread-list`): старшее из двух писем цепочки там.
const THREAD_CHILD: &str = "chain1";

/// Шаг ожидания сцены (перед проверкой готовности): то, что можно сделать только после фоновой работы.
fn wait_step(m: &mut Mailer, scene: Scene, cx: &mut Context<Mailer>) {
    if let Scene::ContactsFrequent = scene {
        // Книга собирается в фоне: пока «Часто пишу» пуст — открыть снова, когда придёт.
        if !m.contacts.read(cx).frequent_shown() {
            m.contacts.update(cx, |c, cx| c.show_frequent(cx));
        }
    }
    if let Scene::EncryptedPin = scene {
        // Демо без смарт-карты: «не для вас» подменяется строкой PIN — так её видит владелец карты.
        m.reading.update(cx, |r, cx| {
            use crate::smime_enc::{Lock, Problem};
            if let Some(s) = r.reading.as_mut().and_then(|r| r.smime.as_mut()).filter(|s| matches!(s.lock, Some(Lock::Failed(Problem::NotForYou(_))))) {
                s.lock = Some(Lock::Failed(Problem::NeedPin));
                cx.notify();
            }
        });
    }
    if let Scene::Smime | Scene::SmimeGost | Scene::SmimeBad | Scene::SmimeUntrusted | Scene::Encrypted = scene {
        // Подпись проверена — раскрыть карточку, как щелчок по значку.
        m.reading.update(cx, |r, cx| {
            let closed = r.reading.as_ref().and_then(|r| r.smime.as_ref()).is_some_and(|s| !s.open && matches!(s.seal, crate::smime::Seal::Done(_)));
            if closed {
                r.toggle_smime(cx);
            }
        });
    }
    if let Scene::Invite = scene {
        // Встреча демо — в прошлом (файл `.ics` с датой): «сейчас» — за сутки до неё, карточка — новое приглашение.
        m.reading.update(cx, |r, cx| {
            if let Some(c) = r.reading.as_mut().and_then(|r| r.invite.as_mut()).filter(|c| c.invitation().is_some() && c.clock.is_none()) {
                c.screen_state("new");
                cx.notify();
            }
        });
    }
    if let Scene::ThreadList = scene {
        // Беседы посчитаны — выбрать письмо под головой, как щелчок по нему.
        m.list.update(cx, |l, cx| {
            let at = l.outline.as_ref().filter(|_| !l.computing).and_then(|o| o.position(THREAD_CHILD));
            let picked = l.selected.and_then(|i| l.row(i)).is_some_and(|r| r.id == THREAD_CHILD);
            if let (Some(i), false) = (at, picked) {
                l.select(i, cx);
            }
        });
    }
}

/// Сцена `lifecycle`: первым шести письмам «Входящих» — сроки всех видов (от «сейчас»), как поставили бы действия
/// «Срок…», «Напомнить…», «Ждать ответа…», «Исчезнет…»; у первого письма открыта панель «Срок».
fn lifecycle_scene(m: &mut Mailer, cx: &mut Context<Mailer>) -> Result<(), String> {
    use postman_core::lifecycle;
    use postman_core::marks::{Kind, When};
    m.set_section(Section::Mail, cx);
    m.reset_filters(cx);
    m.filters.update(cx, |f, cx| f.set(cx, |f| f.quick_open = false));
    let inbox = crate::live::inbox(&m.folders.read(cx).mailboxes).ok_or("no inbox")?;
    m.open_mailbox(inbox, cx);
    let rows: Vec<postman_cache::EmailRow> = m.list.read(cx).rows.1.iter().take(6).cloned().collect();
    if rows.len() < 6 {
        return Err("fewer than 6 letters in the demo inbox".into());
    }
    let now = jiff::Zoned::now();
    let tz = now.time_zone().clone();
    let ts = now.timestamp().as_second();
    let at = |secs: i64| lifecycle::when_at(ts + secs, &tz).ok_or("time out of range");
    let day = |days: i64| now.date().checked_add(jiff::Span::new().days(days)).map_err(|e| e.to_string());
    let remind_day = day(3)?;
    let marks: [(Kind, When); 6] = [
        (Kind::Due, When::day(day(2)?)),
        (Kind::Due, at(3 * 3600)?),
        (Kind::Wait, When::day(day(2)?)),
        (Kind::Remind, When::at(remind_day, jiff::civil::Time::new(9, 0, 0, 0).map_err(|e| e.to_string())?)),
        (Kind::Expire, at(3 * 86_400)?),
        (Kind::Due, When::day(day(-1)?)),
    ];
    let mut patches = Vec::new();
    let mut undo = Vec::new();
    for (row, (kind, when)) in rows.iter().zip(marks) {
        let p = lifecycle::set(row, kind, Some(when));
        let added: Vec<(String, bool)> = p.keywords.iter().filter(|(_, on)| *on).map(|(k, _)| (k.clone(), false)).collect();
        undo.push(lifecycle::Patch { id: row.id.clone(), keywords: added, mailboxes: Vec::new() });
        patches.push(p);
    }
    // «Напомнить» — словом без переноса в «Архив»: письмо остаётся в списке со своей меткой.
    if let Some(d) = &mut m.screens {
        d.undo = undo;
        d.overdue = Some(rows[5].id.clone());
    }
    m.apply_patches(patches, cx);
    m.open_letter(rows[0].clone(), cx);
    m.lifecycle.picker = Some(crate::lifecycle::Picker::new(Kind::Due, rows[0].clone()));
    Ok(())
}

/// Вкладка «Настройки» открыта на разделе `section`.
fn settings_on(m: &Mailer, section: SettingsSection, cx: &App) -> bool {
    m.tabs.is_active(crate::tabs::TabKind::Settings) && m.settings.view.as_ref().is_some_and(|v| v.read(cx).section == section)
}

/// «Входящие» первого общего ящика, где можно писать (иначе первого любого), — сцена `shared`.
fn shared_inbox(m: &Mailer, cx: &App) -> Option<String> {
    let f = m.folders.read(cx);
    let account = f.accounts.iter().find(|a| !f.read_only(a)).or(f.accounts.first())?;
    let mut boxes = f.of(Some(&account.id));
    let first = f.of(Some(&account.id)).next().map(|b| b.id.clone());
    boxes.find(|b| b.role.as_deref() == Some("inbox")).map(|b| b.id.clone()).or(first)
}

/// Вложение демо-письма для сцены просмотра (`postman_demo::blob`).
fn preview_blob(scene: Scene) -> &'static str {
    match scene {
        Scene::PreviewImage => "photo",
        Scene::PreviewPdf => "pdf",
        Scene::PreviewZip => "zip",
        _ => "xlsm",
    }
}

/// Первое прочитанное письмо демо-ящика для сцены: с приглашением, с большой картинкой в тексте или с вложениями
/// для просмотра и картинкой `cid:`.
fn demo_row(list: &crate::list::MailList, scene: Scene) -> Option<usize> {
    let (offset, rows) = &list.rows;
    let wanted = |body: &postman_cache::BodyRow| {
        let a = &body.attachments;
        let has = |blob: &str| a.iter().any(|a| a.blob_id == blob);
        let signed = |kind: &str| body.blob_id.as_deref() == Some(format!("smime-{kind}").as_str());
        match scene {
            Scene::Smime => signed("rsa"),
            Scene::SmimeGost => signed("gost"),
            Scene::SmimeBad => signed("bad"),
            Scene::SmimeUntrusted => signed("untrusted"),
            Scene::Encrypted => signed("enc"),
            Scene::EncryptedOther | Scene::EncryptedPin => signed("enc-other"),
            Scene::EncryptedBad => signed("enc-bad"),
            Scene::Invite => a.iter().any(crate::attachments::is_invite),
            Scene::LetterImage => has("chart"),
            _ => has("pdf") && a.iter().any(|a| a.cid.is_some()),
        }
    };
    rows.iter().position(|r| wanted(&postman_demo::body(&r.id))).map(|p| p + offset)
}

/// PrintWindow — в фоновом потоке: он шлёт сообщения окну, и в потоке окна это повторный вход в оконную
/// процедуру gpui посреди отрисовки.
fn capture(m: &Mailer, scene: Scene, window: &Window, cx: &mut Context<Mailer>) {
    let Some(d) = &m.screens else { return };
    let name = format!("{}{}", scene.name(), d.suffix);
    let path = d.dir.join(format!("{name}.png"));
    let hwnd = sys::window_id(window).unwrap_or_else(|e| fail(scene, &e));
    let handle = window.window_handle();
    cx.spawn(async move |this, cx| {
        // Снимок одного цвета — окно ещё не нарисовано (первый прогон после сборки: шейдеры, первый показ окна):
        // попросить кадр и снять снова, с паузой вдвое дольше каждый раз. Обычно хватает первой попытки.
        let mut result = Err(Shot::Failed(String::new()));
        for attempt in 0..CAPTURE_TRIES {
            if attempt > 0 {
                let _ = cx.update_window(handle, |_, window, _| window.refresh());
                cx.background_executor().timer(CAPTURE_BACKOFF * (1 << (attempt - 1))).await;
            }
            let path = path.clone();
            result = cx.background_spawn(async move { shoot(hwnd, &path).map(|size| (path, size)) }).await;
            if !matches!(result, Err(Shot::Blank(_))) {
                break;
            }
        }
        let result = result.map_err(Shot::message);
        let _ = this.update(cx, |m, cx| {
            match result {
                Ok((path, (w, h))) => println!("screen {name} {} {w}x{h}", path.display()),
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

/// Письмо в активной вкладке.
fn compose_tab<'a>(m: &'a Mailer, _cx: &App) -> Option<&'a Entity<crate::composer::Composer>> {
    match m.tabs.active().kind {
        crate::tabs::TabKind::Compose(id) => m.compose.get(id),
        _ => None,
    }
}

/// Запомнить последнее письмо: сцена готова, когда откроется следующее.
fn remember_composer(m: &mut Mailer, cx: &App) {
    let last = m.compose.last().map(|c| c.read(cx).id);
    if let Some(d) = &mut m.screens {
        d.composer = last;
    }
}

/// Неудачный снимок: окно ещё не нарисовано (повторить) или ошибка.
enum Shot {
    Blank(String),
    Failed(String),
}

impl Shot {
    fn message(self) -> String {
        match self {
            Shot::Blank(e) | Shot::Failed(e) => e,
        }
    }
}

fn shoot(hwnd: isize, path: &Path) -> Result<(u32, u32), Shot> {
    let img = sys::capture(hwnd).map_err(Shot::Failed)?;
    if uniform(&img.rgba) {
        return Err(Shot::Blank(format!(
            "capture {}x{} is a single color after {CAPTURE_TRIES} tries: the window is not drawn",
            img.width, img.height
        )));
    }
    write_png(path, &img).map_err(|e| Shot::Failed(format!("{}: {e}", path.display())))?;
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

/// Событие окна для сцены (KeyTips — удержание Ctrl, подсказка — наведение): после кадра, когда экран почты не занят
/// отрисовкой; уходит в окно тем же путём, что ввод ОС.
fn send_input(window: &Window, cx: &mut Context<Mailer>, event: PlatformInput) {
    let handle = window.window_handle();
    cx.defer(move |cx| {
        let _ = handle.update(cx, |_, window, cx| window.dispatch_event(event, cx));
    });
}

/// Нажатые модификаторы: только Ctrl или ничего.
fn ctrl(on: bool) -> ModifiersChangedEvent {
    ModifiersChangedEvent { modifiers: Modifiers { control: on, ..Modifiers::default() }, capslock: Capslock::default() }
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
