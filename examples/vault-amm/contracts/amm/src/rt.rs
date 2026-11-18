//! Allocator and panic handler for `no_std` guest builds, shared by both contracts.
//! Bump allocator over a static heap, reset whenever no allocation is live, and a
//! panic handler that traps. Replaces the allocator and panic machinery of `std`.
use core::{
    alloc::{GlobalAlloc, Layout},
    cell::UnsafeCell,
    sync::atomic::{AtomicUsize, Ordering::Relaxed},
};

const HEAP_BYTES: usize = 128 * 1024;

#[repr(align(16))]
struct Heap(UnsafeCell<[u8; HEAP_BYTES]>);
// SAFETY: a contract instance runs one message at a time on one thread.
unsafe impl Sync for Heap {}

static HEAP: Heap = Heap(UnsafeCell::new([0; HEAP_BYTES]));
static NEXT: AtomicUsize = AtomicUsize::new(0);
static LIVE: AtomicUsize = AtomicUsize::new(0);

struct Bump;

// SAFETY: blocks are carved from a static buffer, never overlap while live, and
// are aligned as requested; `dealloc` only recycles the buffer once nothing is live.
unsafe impl GlobalAlloc for Bump {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let start = (NEXT.load(Relaxed) + layout.align() - 1) & !(layout.align() - 1);
        match start.checked_add(layout.size()) {
            Some(end) if end <= HEAP_BYTES => {
                NEXT.store(end, Relaxed);
                LIVE.fetch_add(1, Relaxed);
                HEAP.0.get().cast::<u8>().add(start)
            }
            _ => core::ptr::null_mut(),
        }
    }
    unsafe fn dealloc(&self, _ptr: *mut u8, _layout: Layout) {
        if LIVE.fetch_sub(1, Relaxed) == 1 {
            NEXT.store(0, Relaxed);
        }
    }
}

#[global_allocator]
static ALLOCATOR: Bump = Bump;

#[panic_handler]
fn panic(_: &core::panic::PanicInfo<'_>) -> ! {
    core::arch::wasm32::unreachable()
}
