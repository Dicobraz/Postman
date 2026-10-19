//! `--bench`: один сценарий для обоих интерфейсов (egui и gpui), чтобы сравнение было честным.
//! Прогрев → листание с открытием письма каждый кадр → ресайз окна → простой.
//! Итог — одна строка key=value в stdout, затем процесс завершается.

use std::time::{Duration, Instant};

use cpu_time::{ProcessTime, ThreadTime};

const WARMUP: usize = 30;
/// Листание: прыжок на STEP строк с открытием письма каждый кадр.
/// `POSTMAN_BENCH_SCROLL_FRAMES` — другое число кадров, например для профилировщика.
const SCROLL_FRAMES: usize = 400;
const STEP: usize = 25;
/// Ресайз окна туда-обратно.
const RESIZE_FRAMES: usize = 120;
/// Простой: окно открыто, ввода нет. Сначала даём улечься фоновым задачам.
const IDLE_SETTLE: Duration = Duration::from_secs(3);
const IDLE: Duration = Duration::from_secs(10);

pub enum Step {
    Wait,
    Select(usize),
    Resize(f32, f32),
    /// Сценарий окончен: остановить перерисовку и вызвать [`finish`].
    Done(String),
}

pub struct Bench {
    start: Instant,
    first_frame: Option<Duration>,
    frame: usize,
    last: Option<Instant>,
    scroll: Vec<f32>,
    resize: Vec<f32>,
    ui_cpu: Vec<f32>,
    rss_peak: usize,
    private_peak: usize,
    /// Загрузка процесса за время листания.
    scroll_load: Option<Load>,
    scroll_cpu: f32,
    /// Загрузка потока интерфейса (того, что вызывает [`Bench::frame`]) за время листания.
    scroll_ui_load: Option<(Instant, ThreadTime)>,
    scroll_ui_cpu: f32,
    scroll_frames: usize,
}

impl Bench {
    /// `start` — момент запуска процесса (до создания окна).
    pub fn new(start: Instant) -> Self {
        Self {
            start,
            first_frame: None,
            frame: 0,
            last: None,
            scroll: Vec::new(),
            resize: Vec::new(),
            ui_cpu: Vec::new(),
            rss_peak: 0,
            private_peak: 0,
            scroll_load: None,
            scroll_cpu: 0.0,
            scroll_ui_load: None,
            scroll_ui_cpu: 0.0,
            scroll_frames: std::env::var("POSTMAN_BENCH_SCROLL_FRAMES")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(SCROLL_FRAMES),
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
        let f = self.frame;
        if f <= WARMUP {
            return Step::Wait;
        }
        let f = f - WARMUP;
        if f <= self.scroll_frames {
            if f == 1 {
                self.scroll_load = Some(Load::start());
                self.scroll_ui_load = ThreadTime::try_now().ok().map(|t| (Instant::now(), t));
            }
            if let Some(dt) = dt.filter(|_| f > 1) {
                self.scroll.push(dt);
            }
            return Step::Select((f * STEP) % rows.max(1));
        }
        let f = f - self.scroll_frames;
        if f == 1
            && let Some(l) = self.scroll_load.take()
        {
            self.scroll_cpu = l.percent();
            if let Some((wall, cpu)) = self.scroll_ui_load.take() {
                self.scroll_ui_cpu = percent(cpu.elapsed(), wall.elapsed());
            }
        }
        if f <= RESIZE_FRAMES {
            if let Some(dt) = dt.filter(|_| f > 1) {
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
        let s = stats(&mut self.scroll);
        let r = stats(&mut self.resize);
        let c = stats(&mut self.ui_cpu);
        format!(
            "rows={rows} exe_mb={:.1} first_frame_ms={:.0} scroll_p50={:.2} scroll_p99={:.2} scroll_max={:.2} \
             scroll_cpu_pct={:.0} scroll_ui_thread_cpu_pct={:.0} resize_p50={:.2} resize_p99={:.2} resize_max={:.2} \
             ui_cpu_p50={:.2} ui_cpu_p99={:.2} rss_peak_mb={:.0} private_peak_mb={:.0}",
            exe_mb(),
            self.first_frame.unwrap_or_default().as_secs_f32() * 1000.0,
            s.0,
            s.1,
            s.2,
            self.scroll_cpu,
            self.scroll_ui_cpu,
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
