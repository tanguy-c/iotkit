//! How much RAM the heap and the main stack use (`mem` on the console).

use crate::atomic::{AtomicUsize, Ordering};

/// The heap and main stack use, now and at their fullest.
#[derive(Clone, Copy, Debug)]
pub struct Memory {
    /// The heap: the Wi-Fi and Bluetooth drivers allocate in it.
    pub heap_size: usize,
    /// In use now.
    pub heap_used: usize,
    /// The most in use since boot, sampled every second.
    pub heap_peak: usize,
    /// The main stack: what the link left, where the tasks run and interrupts nest.
    pub stack_size: usize,
    /// The deepest it was used since boot.
    pub stack_peak: usize,
}

/// Fills the unused main stack, so that [`memory`] finds how deep it was used.
const PAINT: u32 = 0xa5a5_a5a5;
static HEAP_PEAK: AtomicUsize = AtomicUsize::new(0);

/// The main stack, from its lowest word (above esp-hal's guard word, which a watchpoint
/// watches) to its top.
fn main_stack() -> (*mut u32, usize) {
    unsafe extern "C" {
        static _stack_start_cpu0: u32;
        static __stack_chk_guard: u32;
    }
    let bottom = (&raw const __stack_chk_guard).wrapping_add(1).cast_mut();
    (bottom, (&raw const _stack_start_cpu0) as usize)
}

/// Paints the main stack below the caller's frame; first thing in [`init!`](crate::init).
#[doc(hidden)]
pub fn paint_stack() {
    let (bottom, _) = main_stack();
    let here = 0u8;
    // Below this frame and what it calls next.
    let end = core::hint::black_box(&raw const here) as usize - 512;
    let mut word = bottom;
    while (word as usize) < end {
        // SAFETY: below the stack pointer, so no live frame: nothing reads these words.
        unsafe { word.write_volatile(PAINT) };
        word = word.wrapping_add(1);
    }
}

/// The heap in use now, kept if the most so far (the watchdog task samples it every second).
pub(super) fn sample_heap() -> usize {
    let used = esp_alloc::HEAP.used();
    HEAP_PEAK.fetch_max(used, Ordering::Relaxed);
    used
}

/// The heap and main stack use now, and at their fullest.
pub fn memory() -> Memory {
    let heap_used = sample_heap();
    let (bottom, top) = main_stack();
    let mut word = bottom;
    // SAFETY: inside the stack; the words below the deepest frame so far are still paint.
    while (word as usize) < top && unsafe { word.read_volatile() } == PAINT {
        word = word.wrapping_add(1);
    }
    Memory {
        heap_size: esp_alloc::HEAP.stats().size,
        heap_used,
        heap_peak: HEAP_PEAK.load(Ordering::Relaxed),
        stack_size: top - bottom as usize,
        stack_peak: top - word as usize,
    }
}
