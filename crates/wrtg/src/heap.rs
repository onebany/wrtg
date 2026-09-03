//! Live-heap accounting for `--stats`.
//!
//! RSS alone cannot tell a leak from allocator retention: musl's malloc keeps
//! freed pages mapped, so a process can sit at 50 MB with 3 MB of live data.
//! Counting what is actually outstanding, by size class, splits the two —
//! and if it is a leak, the class that grows names the suspect (a boxed task
//! future lands in a different bucket than a label `String`).
//!
//! Cost: two relaxed atomics per alloc and per free. Wraps [`System`], so the
//! allocator itself is unchanged.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};

/// Upper bound of each size class, inclusive. The last catches everything.
const CLASS_MAX: [usize; 6] = [64, 256, 1024, 4096, 16384, usize::MAX];
static BYTES: [AtomicUsize; 6] = [const { AtomicUsize::new(0) }; 6];
static ALLOCS: [AtomicUsize; 6] = [const { AtomicUsize::new(0) }; 6];

fn class(size: usize) -> usize {
    CLASS_MAX.iter().position(|&max| size <= max).unwrap_or(5)
}

fn add(size: usize) {
    let c = class(size);
    BYTES[c].fetch_add(size, Relaxed);
    ALLOCS[c].fetch_add(1, Relaxed);
}

fn sub(size: usize) {
    let c = class(size);
    BYTES[c].fetch_sub(size, Relaxed);
    ALLOCS[c].fetch_sub(1, Relaxed);
}

pub struct Counting;

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let p = System.alloc(layout);
        if !p.is_null() {
            add(layout.size());
        }
        p
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let p = System.alloc_zeroed(layout);
        if !p.is_null() {
            add(layout.size());
        }
        p
    }

    unsafe fn dealloc(&self, p: *mut u8, layout: Layout) {
        sub(layout.size());
        System.dealloc(p, layout)
    }

    unsafe fn realloc(&self, p: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let q = System.realloc(p, layout, new_size);
        if !q.is_null() {
            sub(layout.size());
            add(new_size);
        }
        q
    }
}

/// Append the `heap` section of the stats snapshot.
pub fn render(out: &mut String) {
    let (mut bytes, mut allocs) = (0, 0);
    let mut rows = Vec::with_capacity(6);
    for (i, max) in CLASS_MAX.iter().enumerate() {
        let (b, n) = (BYTES[i].load(Relaxed), ALLOCS[i].load(Relaxed));
        bytes += b;
        allocs += n;
        let label = if *max == usize::MAX {
            ">16384".to_string()
        } else {
            format!("<={max}")
        };
        rows.push(format!("  {label} {n} allocs {} kB\n", b / 1024));
    }
    out.push_str(&format!(
        "heap live {} kB in {allocs} allocs\n",
        bytes / 1024
    ));
    for r in rows {
        out.push_str(&r);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classes_cover_every_size() {
        assert_eq!(class(1), 0);
        assert_eq!(class(64), 0);
        assert_eq!(class(65), 1);
        assert_eq!(class(4096), 3);
        assert_eq!(class(1 << 20), 5);
    }

    #[test]
    fn render_reports_totals_and_every_class() {
        let mut out = String::new();
        render(&mut out);
        assert!(out.starts_with("heap live "));
        assert_eq!(out.lines().count(), 7);
        assert!(out.contains("  >16384 "));
    }
}
