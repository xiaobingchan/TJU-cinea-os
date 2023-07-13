use alloc::alloc::{GlobalAlloc, Layout};
use core::ptr::null_mut;

use x86_64::{
    structures::paging::{
        FrameAllocator, Mapper, mapper::MapToError, Page, PageTableFlags, Size4KiB,
    },
    VirtAddr,
};

use linked_list::LinkedListAllocator;

pub mod bump;
pub mod linked_list;

#[derive(Debug)]
pub struct Locked<A> {
    inner: spin::Mutex<A>,
}

impl<A> Locked<A> {
    pub const fn new(inner: A) -> Self {
        Locked {
            inner: spin::Mutex::new(inner),
        }
    }

    pub fn lock(&self) -> spin::MutexGuard<A> {
        self.inner.lock()
    }
}

fn align_up(addr: usize, align: usize) -> usize {
    (addr + align.clone() - 1) & !(align - 1)
}

pub struct Dummy;

unsafe impl GlobalAlloc for Dummy {
    unsafe fn alloc(&self, _layout: Layout) -> *mut u8 {
        null_mut()
    }

    unsafe fn dealloc(&self, _ptr: *mut u8, _layout: Layout) {
        panic!("dealloc should be never called")
    }
}

#[global_allocator]
static ALLOCATOR: Locked<LinkedListAllocator> = Locked::new(LinkedListAllocator::new());

pub const HEAP_START: usize = 0x_0001_0000_0000;
pub const HEAP_SIZE: usize = 40 * 1024 * 1024; // 10 MiB

pub fn init_heap(
    mapper: &mut impl Mapper<Size4KiB>,
    frame_allocator: &mut impl FrameAllocator<Size4KiB>,
) -> Result<(), MapToError<Size4KiB>> {
    syskrnl::proc::init_process_addr((HEAP_START + HEAP_SIZE) as u64);

    let page_range = {
        let heap_start = VirtAddr::new(HEAP_START as u64);
        let heap_end = heap_start + HEAP_SIZE - 1u64;
        let heap_start_page = Page::containing_address(heap_start);
        let heap_end_page = Page::containing_address(heap_end);
        Page::range_inclusive(heap_start_page, heap_end_page)
    };

    for page in page_range {
        let frame = frame_allocator
            .allocate_frame()
            .ok_or(MapToError::FrameAllocationFailed)?;
        let flags = PageTableFlags::PRESENT | PageTableFlags::WRITABLE;
        unsafe {
            mapper.map_to(page, frame, flags, frame_allocator)?.flush()
        };
    }

    unsafe {
        ALLOCATOR.lock().init(HEAP_START, HEAP_SIZE);
    }

    Ok(())
}

use x86_64::structures::paging::page::PageRangeInclusive;
use crate::{debugln, syskrnl};

// TODO: Replace `free` by `dealloc`
pub fn free_pages(addr: u64, size: usize) {
    let mut mapper = unsafe { syskrnl::memory::mapper(VirtAddr::new(syskrnl::memory::PHYS_MEM_OFFSET)) };
    let pages: PageRangeInclusive<Size4KiB> = {
        let start_page = Page::containing_address(VirtAddr::new(addr));
        let end_page = Page::containing_address(VirtAddr::new(addr + (size as u64) - 1));
        Page::range_inclusive(start_page, end_page)
    };
    for page in pages {
        if let Ok((_frame, mapping)) = mapper.unmap(page) {
            mapping.flush();
        } else {
            //debug!("Could not unmap {:?}", page);
        }
    }
}

pub fn alloc_pages(addr: u64, size: usize) -> Result<(), ()> {
    let mut mapper = unsafe { syskrnl::memory::mapper(VirtAddr::new(syskrnl::memory::PHYS_MEM_OFFSET)) };
    let mut frame_allocator = unsafe { syskrnl::memory::BootInfoFrameAllocator::init(syskrnl::memory::MEMORY_MAP.unwrap()) };
    let flags = PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::USER_ACCESSIBLE;
    let pages = {
        let start_page = Page::containing_address(VirtAddr::new(addr));
        let end_page = Page::containing_address(VirtAddr::new(addr + (size as u64) - 1));
        Page::range_inclusive(start_page, end_page)
    };
    for page in pages {
        //debugln!("Alloc page {:?}", page);
        if let Some(frame) = frame_allocator.allocate_frame() {
            //debugln!("Alloc frame {:?}", frame);
            unsafe {
                if let Ok(mapping) = mapper.map_to(page, frame, flags, &mut frame_allocator) {
                    //debugln!("Mapped {:?} to {:?}", page, frame);
                    mapping.flush();
                } else {
                    debugln!("Could not map {:?} to {:?}", page, frame);
                    return Err(());
                }
            }
        } else {
            debugln!("Could not allocate frame for {:?}", page);
            return Err(());
        }
    }
    Ok(())
}

#[allow(dead_code)]
pub fn test_allocator() {
    use alloc::boxed::Box;
    use crate::println;
    use alloc::vec::Vec;
    use alloc::vec;
    use alloc::rc::Rc;

    let heap_value = Box::new(831);
    println!("heap_value is at {:p}", heap_value);

    let mut vec = Vec::new();
    for i in 0..500 {
        vec.push(i)
    }
    println!("vec at {:p}", vec.as_slice());

    let reference_counted = Rc::new(vec![1, 2, 3]);
    let cloned_reference = reference_counted.clone();
    println!("current reference count is {}", Rc::strong_count(&cloned_reference));
    core::mem::drop(reference_counted);
    println!("reference count is {} now", Rc::strong_count(&cloned_reference));
}
