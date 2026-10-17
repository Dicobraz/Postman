//! `--bench`: воспроизводимый сценарий для объективного сравнения
//! (бэкенды GPU, UI-фреймворки, машины). Печатает итог одной строкой key=value.

use std::time::{Duration, Instant};

const WARMUP: usize = 30;
/// Листание: прыжок на STEP строк с открытием письма каждый кадр.
const SCROLL_FRAMES: usize = 400;
const STEP: usize = 25;
/// Ресайз окна туда-обратно.
const RESIZE_FRAMES: usize = 120;

pub enum Step {
    Wait,
    Select(usize),
    Resize(f32, f32),
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
}

impl Bench {
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
        }
    }

    /// Вызывается в начале кадра; `rows` — сколько писем в списке.
    pub fn frame(&mut self, rows: usize) -> Step {
        let now = Instant::now();
        self.first_frame.get_or_insert(now - self.start);
        let dt = self.last.map(|l| (now - l).as_secs_f32() * 1000.0);
        self.last = Some(now);
        self.frame += 1;
        if self.frame.is_multiple_of(20) {
            self.rss_peak = self.rss_peak.max(rss());
        }
        let f = self.frame;
        if f <= WARMUP {
            return Step::Wait;
        }
        let f = f - WARMUP;
        if f <= SCROLL_FRAMES {
            if let Some(dt) = dt.filter(|_| f > 1) {
                self.scroll.push(dt);
            }
            return Step::Select((f * STEP) % rows.max(1));
        }
        let f = f - SCROLL_FRAMES;
        if f <= RESIZE_FRAMES {
            if let Some(dt) = dt.filter(|_| f > 1) {
                self.resize.push(dt);
            }
            let w = if (f / 4).is_multiple_of(2) { 1000.0 } else { 1280.0 };
            return Step::Resize(w, 800.0);
        }
        Step::Done(self.report())
    }

    pub fn ui_time(&mut self, d: Duration) {
        if self.frame > WARMUP {
            self.ui_cpu.push(d.as_secs_f32() * 1000.0);
        }
    }

    fn report(&mut self) -> String {
        self.rss_peak = self.rss_peak.max(rss());
        let s = stats(&mut self.scroll);
        let r = stats(&mut self.resize);
        let c = stats(&mut self.ui_cpu);
        format!(
            "first_frame_ms={:.0} scroll_p50={:.2} scroll_p99={:.2} scroll_max={:.2} resize_p50={:.2} resize_p99={:.2} \
             resize_max={:.2} ui_cpu_p50={:.2} ui_cpu_p99={:.2} rss_mb={:.0} rss_peak_mb={:.0}",
            self.first_frame.unwrap_or_default().as_secs_f32() * 1000.0,
            s.0,
            s.1,
            s.2,
            r.0,
            r.1,
            r.2,
            c.0,
            c.1,
            rss() as f32 / 1_048_576.0,
            self.rss_peak as f32 / 1_048_576.0,
        )
    }
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

pub fn rss() -> usize {
    memory_stats::memory_stats().map(|m| m.physical_mem).unwrap_or(0)
}
