#![no_std]

use allocator::{AllocError, AllocResult, BaseAllocator, ByteAllocator, PageAllocator};
use core::alloc::Layout;
use core::ptr::NonNull;

/// Early memory allocator
/// Use it before formal bytes-allocator and pages-allocator can work!
/// This is a double-end memory range:
/// - Alloc bytes forward
/// - Alloc pages backward
///
/// [ bytes-used | avail-area | pages-used ]
/// |            | -->    <-- |            |
/// start       b_pos        p_pos       end
///
/// For bytes area, 'count' records number of allocations.
/// When it goes down to ZERO, free bytes-used area.
/// For pages area, it will never be freed!
///
pub struct EarlyAllocator<const SIZE: usize> {
    /// Start of the managed memory range.
    start: usize,
    /// End (exclusive) of the managed memory range.
    end: usize,
    /// Next free byte of the bytes area (grows upward).
    b_pos: usize,
    /// Lowest address used by the pages area (grows downward).
    p_pos: usize,
    /// Number of live byte allocations.
    count: usize,
}

impl<const SIZE: usize> EarlyAllocator<SIZE> {
    pub const fn new() -> Self {
        Self {
            start: 0,
            end: 0,
            b_pos: 0,
            p_pos: 0,
            count: 0,
        }
    }
}

#[inline]
const fn align_up(addr: usize, align: usize) -> usize {
    (addr + align - 1) & !(align - 1)
}

#[inline]
const fn align_down(addr: usize, align: usize) -> usize {
    addr & !(align - 1)
}

impl<const SIZE: usize> BaseAllocator for EarlyAllocator<SIZE> {
    fn init(&mut self, start: usize, size: usize) {
        self.start = start;
        self.end = start + size;
        self.b_pos = start;
        self.p_pos = self.end;
        self.count = 0;
    }

    fn add_memory(&mut self, _start: usize, _size: usize) -> AllocResult {
        // A bump allocator manages exactly one contiguous range.
        Err(AllocError::NoMemory)
    }
}

impl<const SIZE: usize> ByteAllocator for EarlyAllocator<SIZE> {
    fn alloc(&mut self, layout: Layout) -> AllocResult<NonNull<u8>> {
        // Bump `b_pos` forward; fail if it would run into the pages area.
        let addr = align_up(self.b_pos, layout.align());
        let new_pos = addr.checked_add(layout.size()).ok_or(AllocError::NoMemory)?;
        if new_pos > self.p_pos {
            return Err(AllocError::NoMemory);
        }
        self.b_pos = new_pos;
        self.count += 1;
        NonNull::new(addr as *mut u8).ok_or(AllocError::NoMemory)
    }

    fn dealloc(&mut self, _pos: NonNull<u8>, _layout: Layout) {
        // Individual blocks cannot be freed; the whole bytes area is reset
        // once every allocation has been returned.
        self.count = self.count.saturating_sub(1);
        if self.count == 0 {
            self.b_pos = self.start;
        }
    }

    fn total_bytes(&self) -> usize {
        self.end - self.start
    }

    fn used_bytes(&self) -> usize {
        self.b_pos - self.start
    }

    fn available_bytes(&self) -> usize {
        self.p_pos - self.b_pos
    }
}

impl<const SIZE: usize> PageAllocator for EarlyAllocator<SIZE> {
    const PAGE_SIZE: usize = SIZE;

    fn alloc_pages(&mut self, num_pages: usize, align_pow2: usize) -> AllocResult<usize> {
        if align_pow2 % Self::PAGE_SIZE != 0 || !align_pow2.is_power_of_two() {
            return Err(AllocError::InvalidParam);
        }
        // Move `p_pos` backward; fail if it would run into the bytes area.
        let size = num_pages
            .checked_mul(Self::PAGE_SIZE)
            .ok_or(AllocError::NoMemory)?;
        let addr = self
            .p_pos
            .checked_sub(size)
            .map(|a| align_down(a, align_pow2))
            .ok_or(AllocError::NoMemory)?;
        if addr < self.b_pos {
            return Err(AllocError::NoMemory);
        }
        self.p_pos = addr;
        Ok(addr)
    }

    fn dealloc_pages(&mut self, _pos: usize, _num_pages: usize) {
        // Pages are never freed by the early allocator.
    }

    fn total_pages(&self) -> usize {
        (self.end - self.start) / Self::PAGE_SIZE
    }

    fn used_pages(&self) -> usize {
        (self.end - self.p_pos) / Self::PAGE_SIZE
    }

    fn available_pages(&self) -> usize {
        (self.p_pos - self.b_pos) / Self::PAGE_SIZE
    }
}
