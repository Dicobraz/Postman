use std::{
    sync::{
        Arc, LazyLock,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use parking_lot::{Condvar, Mutex};

use anyhow::{Context, Result};
use gpui::FrameRequestSource;
use gpui_util::ResultExt;
use windows::Win32::{
    Foundation::HWND,
    Graphics::Dwm::{DWM_TIMING_INFO, DwmFlush, DwmGetCompositionTimingInfo},
    System::Performance::QueryPerformanceFrequency,
};

static QPC_TICKS_PER_SECOND: LazyLock<u64> = LazyLock::new(|| {
    let mut frequency = 0;
    // On systems that run Windows XP or later, the function will always succeed and
    // will thus never return zero.
    unsafe { QueryPerformanceFrequency(&mut frequency).unwrap() };
    frequency as u64
});

const VSYNC_INTERVAL_THRESHOLD: Duration = Duration::from_millis(1);
const DEFAULT_VSYNC_INTERVAL: Duration = Duration::from_micros(16_666); // ~60Hz

/// How often an idle vsync thread wakes up anyway, to notice a stop request or a lost device.
pub(crate) const IDLE_VSYNC_CHECK: Duration = Duration::from_secs(1);

/// Frame demand for the vsync thread (Postman patch): the thread sleeps while no window has asked
/// for a frame, instead of invalidating every window on every vsync. Windows ask through
/// [`FrameRequest`], which GPUI calls via `frame_waker` / `schedule_frame` whenever a window
/// becomes dirty or has next-frame callbacks.
pub(crate) static FRAME_DEMAND: FrameDemand = FrameDemand {
    pending: Mutex::new(false),
    wake: Condvar::new(),
};

pub(crate) struct FrameDemand {
    pending: Mutex<bool>,
    wake: Condvar,
}

impl FrameDemand {
    pub(crate) fn notify(&self) {
        *self.pending.lock() = true;
        self.wake.notify_one();
    }

    /// Blocks until [`Self::notify`] or `timeout`, then clears the request.
    pub(crate) fn wait(&self, timeout: Duration) {
        let mut pending = self.pending.lock();
        if !*pending {
            self.wake.wait_for(&mut pending, timeout);
        }
        *pending = false;
    }
}

/// A window's "draw me on the next vsync" flag.
#[derive(Clone, Default)]
pub(crate) struct FrameRequest(Arc<AtomicBool>);

impl FrameRequest {
    pub(crate) fn new_requested() -> Self {
        Self(Arc::new(AtomicBool::new(true)))
    }

    pub(crate) fn request(&self) {
        // Already requested: the vsync thread has not consumed it yet and will see it.
        if !self.0.swap(true, Ordering::AcqRel) {
            FRAME_DEMAND.notify();
        }
    }

    pub(crate) fn is_requested(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }

    pub(crate) fn take(&self) -> bool {
        self.0.swap(false, Ordering::AcqRel)
    }
}

pub(crate) struct VSyncProvider {
    interval: Duration,
    f: Box<dyn Fn() -> bool>,
}

impl VSyncProvider {
    pub(crate) fn new() -> Self {
        let interval = get_dwm_interval()
            .context("Failed to get DWM interval")
            .log_err()
            .unwrap_or(DEFAULT_VSYNC_INTERVAL);
        let f = Box::new(|| unsafe { DwmFlush().is_ok() });
        Self { interval, f }
    }

    pub(crate) fn wait_for_vsync(&self) -> FrameRequestSource {
        let vsync_start = Instant::now();
        let wait_succeeded = (self.f)();
        let elapsed = vsync_start.elapsed();
        // DwmFlush and DCompositionWaitForCompositorClock returns very early
        // instead of waiting until vblank when the monitor goes to sleep or is
        // unplugged (nothing to present due to desktop occlusion). We use 1ms as
        // a threshold for the duration of the wait functions and fallback to
        // Sleep() if it returns before that. This could happen during normal
        // operation for the first call after the vsync thread becomes non-idle,
        // but it shouldn't happen often.
        if !wait_succeeded || elapsed < VSYNC_INTERVAL_THRESHOLD {
            log::trace!("VSyncProvider::wait_for_vsync() took less time than expected");
            std::thread::sleep(self.interval);
            FrameRequestSource::LocalSchedule
        } else {
            FrameRequestSource::NativeCallback
        }
    }
}

fn get_dwm_interval() -> Result<Duration> {
    let mut timing_info = DWM_TIMING_INFO {
        cbSize: std::mem::size_of::<DWM_TIMING_INFO>() as u32,
        ..Default::default()
    };
    unsafe { DwmGetCompositionTimingInfo(HWND::default(), &mut timing_info) }?;
    let interval = retrieve_duration(timing_info.qpcRefreshPeriod, *QPC_TICKS_PER_SECOND);
    // Check for interval values that are impossibly low. A 29 microsecond
    // interval was seen (from a qpcRefreshPeriod of 60).
    if interval < VSYNC_INTERVAL_THRESHOLD {
        Ok(retrieve_duration(
            timing_info.rateRefresh.uiDenominator as u64,
            timing_info.rateRefresh.uiNumerator as u64,
        ))
    } else {
        Ok(interval)
    }
}

#[inline]
fn retrieve_duration(counts: u64, ticks_per_second: u64) -> Duration {
    let ticks_per_microsecond = ticks_per_second / 1_000_000;
    Duration::from_micros(counts / ticks_per_microsecond)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn successful_compositor_wait_reports_native_callback() {
        let provider = VSyncProvider {
            interval: Duration::ZERO,
            f: Box::new(|| {
                std::thread::sleep(VSYNC_INTERVAL_THRESHOLD * 2);
                true
            }),
        };

        assert_eq!(
            provider.wait_for_vsync(),
            FrameRequestSource::NativeCallback
        );
    }

    #[test]
    fn failed_compositor_wait_reports_local_schedule_even_after_threshold() {
        let provider = VSyncProvider {
            interval: Duration::ZERO,
            f: Box::new(|| {
                std::thread::sleep(VSYNC_INTERVAL_THRESHOLD * 2);
                false
            }),
        };

        assert_eq!(provider.wait_for_vsync(), FrameRequestSource::LocalSchedule);
    }

    #[test]
    fn frame_request_wakes_the_vsync_thread() {
        let request = FrameRequest::default();
        assert!(!request.is_requested());
        request.request();
        request.request();
        assert!(request.is_requested());
        let start = Instant::now();
        FRAME_DEMAND.wait(Duration::from_secs(5));
        assert!(start.elapsed() < Duration::from_secs(1));
        assert!(request.take());
        assert!(!request.take());
    }
}
