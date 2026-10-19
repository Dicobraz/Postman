//! `--bench`: один сценарий для обоих интерфейсов (egui и gpui), чтобы сравнение было честным.
//! Прогрев → листание с открытием письма каждый кадр → листание списка без открытия → ресайз окна → простой.
//! Итог — одна строка key=value в stdout, затем процесс завершается.

use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use cpu_time::{ProcessTime, ThreadTime};

const WARMUP: usize = 30;
/// Листание с открытием: прыжок на STEP строк и открытие письма каждый кадр (худший случай).
/// `POSTMAN_BENCH_SCROLL_FRAMES` — другое число кадров, например для профилировщика.
const SCROLL_FRAMES: usize = 400;
const STEP: usize = 25;
/// Листание списка без открытия писем: LIST_STEP строк за кадр, как прокрутка колесом.
/// `POSTMAN_BENCH_LIST_FRAMES` — другое число кадров.
const LIST_FRAMES: usize = 400;
const LIST_STEP: usize = 3;
/// Ресайз окна туда-обратно.
const RESIZE_FRAMES: usize = 120;
/// Простой: окно открыто, ввода нет. Сначала даём улечься фоновым задачам.
const IDLE_SETTLE: Duration = Duration::from_secs(3);
const IDLE: Duration = Duration::from_secs(10);

pub enum Step {
    Wait,
    /// Открыть письмо `i` (список прокручивается к нему).
    Select(usize),
    /// Прокрутить список так, чтобы строка `i` была первой; письмо не открывать.
    Scroll(usize),
    Resize(f32, f32),
    /// Сценарий окончен: остановить перерисовку и вызвать [`finish`].
    Done(String),
}

/// Замеры одной фазы: интервалы кадров, загрузка процесса и потока интерфейса.
#[derive(Default)]
struct Phase {
    frames: Vec<f32>,
    load: Option<Load>,
    ui_load: Option<(Instant, ThreadTime)>,
    cpu: f32,
    ui_cpu: f32,
}

impl Phase {
    /// Кадр `f` фазы (с 1); `dt` — интервал от прошлого кадра.
    fn frame(&mut self, f: usize, dt: Option<f32>) {
        if f == 1 {
            self.load = Some(Load::start());
            self.ui_load = ThreadTime::try_now().ok().map(|t| (Instant::now(), t));
        } else if let Some(dt) = dt {
            self.frames.push(dt);
        }
    }

    fn stop(&mut self) {
        if let Some(l) = self.load.take() {
            self.cpu = l.percent();
        }
        if let Some((wall, cpu)) = self.ui_load.take() {
            self.ui_cpu = percent(cpu.elapsed(), wall.elapsed());
        }
    }

    /// `name_p50 name_p99 name_max name_cpu_pct name_ui_thread_cpu_pct`.
    fn report(&mut self, name: &str) -> String {
        let (p50, p99, max) = stats(&mut self.frames);
        format!(
            "{name}_p50={p50:.2} {name}_p99={p99:.2} {name}_max={max:.2} {name}_cpu_pct={:.0} {name}_ui_thread_cpu_pct={:.0}",
            self.cpu, self.ui_cpu
        )
    }
}

pub struct Bench {
    start: Instant,
    first_frame: Option<Duration>,
    frame: usize,
    last: Option<Instant>,
    scroll: Phase,
    list: Phase,
    resize: Vec<f32>,
    ui_cpu: Vec<f32>,
    rss_peak: usize,
    private_peak: usize,
    scroll_frames: usize,
    list_frames: usize,
}

/// Число кадров фазы из переменной окружения или по умолчанию.
fn frames(var: &str, default: usize) -> usize {
    std::env::var(var).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}

impl Bench {
    /// `start` — момент запуска процесса (до создания окна).
    pub fn new(start: Instant) -> Self {
        Self {
            start,
            first_frame: None,
            frame: 0,
            last: None,
            scroll: Phase::default(),
            list: Phase::default(),
            resize: Vec::new(),
            ui_cpu: Vec::new(),
            rss_peak: 0,
            private_peak: 0,
            scroll_frames: frames("POSTMAN_BENCH_SCROLL_FRAMES", SCROLL_FRAMES),
            list_frames: frames("POSTMAN_BENCH_LIST_FRAMES", LIST_FRAMES),
        }
    }

    /// Вызывается в начале каждого кадра; `rows` — сколько писем в списке.
    pub fn frame(&mut self, rows: usize) -> Step {
        let now = Instant::now();
        self.first_frame.get_or_insert(now - self.start);
        let dt = self.last.map(|l| (now - l).as_secs_f32() * 1000.0);
        self.last = Some(now);
        self.frame += 1;
        if self.frame.is_multiple_of(20) {
            self.sample_memory();
        }
        let rows = rows.max(1);
        let f = self.frame;
        if f <= WARMUP {
            return Step::Wait;
        }
        let f = f - WARMUP;
        if f <= self.scroll_frames {
            self.scroll.frame(f, dt);
            return Step::Select((f * STEP) % rows);
        }
        let f = f - self.scroll_frames;
        if f <= self.list_frames {
            if f == 1 {
                self.scroll.stop();
            }
            self.list.frame(f, dt);
            return Step::Scroll((f * LIST_STEP) % rows);
        }
        let f = f - self.list_frames;
        if f <= RESIZE_FRAMES {
            if f == 1 {
                self.list.stop();
            } else if let Some(dt) = dt {
                self.resize.push(dt);
            }
            let w = if (f / 4).is_multiple_of(2) { 1000.0 } else { 1280.0 };
            return Step::Resize(w, 800.0);
        }
        Step::Done(self.report(rows))
    }

    /// Время построения кадра интерфейсом (без ожидания vsync).
    pub fn ui_time(&mut self, d: Duration) {
        if self.frame > WARMUP {
            self.ui_cpu.push(d.as_secs_f32() * 1000.0);
        }
    }

    fn sample_memory(&mut self) {
        let m = memory();
        self.rss_peak = self.rss_peak.max(m.rss);
        self.private_peak = self.private_peak.max(m.private);
    }

    fn report(&mut self, rows: usize) -> String {
        self.sample_memory();
        let r = stats(&mut self.resize);
        let c = stats(&mut self.ui_cpu);
        format!(
            "rows={rows} exe_mb={:.1} first_frame_ms={:.0} {} {} resize_p50={:.2} resize_p99={:.2} resize_max={:.2} \
             ui_cpu_p50={:.2} ui_cpu_p99={:.2} rss_peak_mb={:.0} private_peak_mb={:.0}",
            exe_mb(),
            self.first_frame.unwrap_or_default().as_secs_f32() * 1000.0,
            self.scroll.report("scroll"),
            self.list.report("list"),
            r.0,
            r.1,
            r.2,
            c.0,
            c.1,
            mb(self.rss_peak),
            mb(self.private_peak),
        )
    }
}

/// Рабочая копия данных бенча — удалить при завершении (H-01).
static WORK_COPY: OnceLock<PathBuf> = OnceLock::new();

/// Удалить файл данных бенча, когда замер закончится (флаг `--keep` у приложения — не вызывать).
pub fn remove_on_finish(path: PathBuf) {
    let _ = WORK_COPY.set(path);
}

/// Замер простоя в отдельном потоке: печатает `prefix report idle_…` и завершает процесс.
/// Интерфейс к этому моменту должен перестать запрашивать кадры.
pub fn finish(line: String) {
    std::thread::spawn(move || {
        std::thread::sleep(IDLE_SETTLE);
        let load = Load::start();
        std::thread::sleep(IDLE);
        let m = memory();
        println!(
            "{line} idle_cpu_pct={:.1} idle_rss_mb={:.0} idle_private_mb={:.0}",
            load.percent(),
            mb(m.rss),
            mb(m.private)
        );
        // Хранилище ещё открыто, но std открывает файлы с FILE_SHARE_DELETE: файл исчезнет, когда процесс
        // закроет его при выходе.
        if let Some(p) = WORK_COPY.get() {
            let _ = std::fs::remove_file(p);
        }
        std::process::exit(0);
    });
}

/// Загрузка CPU процессом: процессорное время / прошедшее, в % от одного ядра.
struct Load {
    wall: Instant,
    cpu: Option<ProcessTime>,
}

impl Load {
    fn start() -> Self {
        Self { wall: Instant::now(), cpu: ProcessTime::try_now().ok() }
    }

    fn percent(&self) -> f32 {
        self.cpu.as_ref().map_or(0.0, |cpu| percent(cpu.elapsed(), self.wall.elapsed()))
    }
}

/// Процессорное время / прошедшее, в % от одного ядра.
fn percent(cpu: Duration, wall: Duration) -> f32 {
    let wall = wall.as_secs_f32();
    if wall > 0.0 { cpu.as_secs_f32() / wall * 100.0 } else { 0.0 }
}

/// (p50, p99, max) в мс.
fn stats(v: &mut [f32]) -> (f32, f32, f32) {
    if v.is_empty() {
        return (0.0, 0.0, 0.0);
    }
    v.sort_by(f32::total_cmp);
    let p = |q: f32| v[((v.len() - 1) as f32 * q) as usize];
    (p(0.5), p(0.99), v[v.len() - 1])
}

/// Рабочий набор процесса (RSS / Working Set), байты.
pub fn rss() -> usize {
    memory().rss
}

struct Memory {
    /// Рабочий набор (Working Set): столбец «Память» в диспетчере задач.
    rss: usize,
    /// Windows — private bytes (PagefileUsage): столбец «Выделенная память» в диспетчере задач.
    /// На других ОС — виртуальный размер, с Windows не сравнивать.
    private: usize,
}

fn memory() -> Memory {
    memory_stats::memory_stats()
        .map(|m| Memory { rss: m.physical_mem, private: m.virtual_mem })
        .unwrap_or(Memory { rss: 0, private: 0 })
}

fn mb(bytes: usize) -> f32 {
    bytes as f32 / 1_048_576.0
}

/// Размер исполняемого файла, МиБ.
fn exe_mb() -> f32 {
    std::env::current_exe().and_then(std::fs::metadata).map_or(0.0, |m| m.len() as f32 / 1_048_576.0)
}
