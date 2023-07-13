use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::arch::asm;
use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use lazy_static::lazy_static;
use object::{Object, ObjectSegment};
use spin::RwLock;
use x86_64::structures::idt::InterruptStackFrameValue;
use x86_64::VirtAddr;

use crate::{debugln, syskrnl};
use crate::sysapi::proc::ExitCode;
use crate::syskrnl::allocator::{alloc_pages, Locked};
use crate::syskrnl::allocator::linked_list::LinkedListAllocator;

// const MAX_FILE_HANDLES: usize = 64;
/// 最大进程数，先写2个，后面再改
const MAX_PROCS: usize = 4;
const MAX_PROC_SIZE: usize = 10 << 20;

pub static PID: AtomicUsize = AtomicUsize::new(0);
pub static MAX_PID: AtomicUsize = AtomicUsize::new(1);

pub static PROC_HEAP_ADDR: AtomicUsize = AtomicUsize::new(0x0002_0000_0000);
const DEFAULT_HEAP_SIZE: usize = 0x4000; // 默认堆内存大小

static mut RSP: usize = 0;
static mut RFLAGS: usize = 0;

lazy_static! {
    pub static ref PROCESS_TABLE: RwLock<[Box<Process>; MAX_PROCS]> = {
        let table: [Box<Process>; MAX_PROCS] = [(); MAX_PROCS].map(|_| Box::new(Process::new(0)));
        RwLock::new(table)
    };
}

#[derive(Clone, Debug)]
pub struct ProcessData {
    env: BTreeMap<String, String>,
    dir: String,
    user: Option<String>,
    //file_handles: [Option<Box<Resource>>; MAX_FILE_HANDLES],
}

#[repr(align(8), C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct Registers {
    pub r11: usize,
    pub r10: usize,
    pub r9: usize,
    pub r8: usize,
    pub rdi: usize,
    pub rsi: usize,
    pub rdx: usize,
    pub rcx: usize,
    pub rax: usize,
}

const ELF_MAGIC: [u8; 4] = [0x7F, b'E', b'L', b'F'];
const BIN_MAGIC: [u8; 4] = [0x7F, b'B', b'I', b'N'];

#[derive(Clone, Debug)]
pub struct Process {
    id: usize,
    code_addr: u64,
    stack_addr: u64,
    entry_point: u64,
    stack_frame: InterruptStackFrameValue,
    registers: Registers,
    data: ProcessData,
    parent: usize,
    allocator: Arc<Locked<LinkedListAllocator>>
}

impl ProcessData {
    pub fn new(dir: &str, user: Option<&str>) -> Self {
        let env = BTreeMap::new();
        let dir = dir.to_string();
        let user = user.map(String::from);
        // let mut file_handles = [(); MAX_FILE_HANDLES].map(|_| None);
        // file_handles[0] = Some(Box::new(Resource::Device(Device::Console(Console::new())))); // stdin
        // file_handles[1] = Some(Box::new(Resource::Device(Device::Console(Console::new())))); // stdout
        // file_handles[2] = Some(Box::new(Resource::Device(Device::Console(Console::new())))); // stderr
        // file_handles[3] = Some(Box::new(Resource::Device(Device::Null))); // stdnull
        Self { env, dir, user /*, file_handles*/ }
    }
}

impl Process {
    pub fn new(id: usize) -> Self {
        let isf = InterruptStackFrameValue {
            instruction_pointer: VirtAddr::new(0),
            code_segment: 0,
            cpu_flags: 0,
            stack_pointer: VirtAddr::new(0),
            stack_segment: 0,
        };
        Self {
            id,
            code_addr: 0,
            stack_addr: 0,
            entry_point: 0,
            stack_frame: isf,
            registers: Registers::default(),
            data: ProcessData::new("/", None),
            parent: 0,
            allocator: Arc::new(Locked::new(LinkedListAllocator::new()))
        }
    }
}

/// 获取当前进程PID
pub fn id() -> usize {
    PID.load(Ordering::SeqCst)
}

/// 设置当前进程PID
pub fn set_id(id: usize) {
    PID.store(id, Ordering::SeqCst);
}

/// 获取当前进程的环境变量
pub fn env(key: &str) -> Option<String> {
    let table = PROCESS_TABLE.read();
    let process = &table[id()];
    process.data.env.get(key).cloned()
}

/// 获取当前进程的环境变量
pub fn envs() -> BTreeMap<String, String> {
    let table = PROCESS_TABLE.read();
    let process = &table[id()];
    process.data.env.clone()
}

/// 获取当前进程的工作目录
pub fn dir() -> String {
    let table = PROCESS_TABLE.read();
    let process = &table[id()];
    process.data.dir.clone()
}

/// 获取当前进程的用户名
pub fn user() -> Option<String> {
    let table = PROCESS_TABLE.read();
    let process = &table[id()];
    process.data.user.clone()
}

/// 设置当前进程的环境变量
pub fn set_env(key: &str, val: &str) {
    let mut table = PROCESS_TABLE.write();
    let proc = &mut table[id()];
    proc.data.env.insert(key.into(), val.into());
}

/// 设置当前进程的工作目录
pub fn set_dir(dir: &str) {
    let mut table = PROCESS_TABLE.write();
    let proc = &mut table[id()];
    proc.data.dir = dir.into();
}

/// 设置当前进程的用户名
pub fn set_user(user: &str) {
    let mut table = PROCESS_TABLE.write();
    let proc = &mut table[id()];
    proc.data.user = Some(user.into())
}

/// 获取当前进程的代码地址
pub fn code_addr() -> u64 {
    let table = PROCESS_TABLE.read();
    let process = &table[id()];
    process.code_addr
}

/// 设置当前进程的代码地址
pub fn set_code_addr(addr: u64) {
    let mut table = PROCESS_TABLE.write();
    let proc = &mut table[id()];
    proc.code_addr = addr;
}

/// 偏移地址转换实际地址
pub fn ptr_from_addr(addr: u64) -> *mut u8 {
    let base = code_addr();
    if addr < base {
        (base + addr) as *mut u8
    } else {
        addr as *mut u8
    }
}

/// 获取当前进程的寄存器
pub fn registers() -> Registers {
    let table = PROCESS_TABLE.read();
    let process = &table[id()];
    process.registers
}

/// 设置当前进程的寄存器
pub fn set_registers(regs: Registers) {
    let mut table = PROCESS_TABLE.write();
    let proc = &mut table[id()];
    proc.registers = regs
}

/// 获取当前进程的栈帧
pub fn stack_frame() -> InterruptStackFrameValue {
    let table = PROCESS_TABLE.read();
    let proc = &table[id()];
    proc.stack_frame
}

/// 设置当前进程的栈帧
pub fn set_stack_frame(stack_frame: InterruptStackFrameValue) {
    let mut table = PROCESS_TABLE.write();
    let proc = &mut table[id()];
    proc.stack_frame = stack_frame;
}

/// 获取当前进程的堆分配器
pub fn heap_allocator() -> Arc<Locked<LinkedListAllocator>> {
    let table = PROCESS_TABLE.read();
    let proc = &table[id()];
    proc.allocator.clone()
}

/// 生长当前进程的堆
pub fn allocator_grow(size: usize) {
    let table = PROCESS_TABLE.write();
    let allocator = table[id()].allocator.clone();
    let addr = PROC_HEAP_ADDR.fetch_add(size, Ordering::SeqCst);
    alloc_pages(addr as u64, size).expect("proc mem grow fail 1545");
    unsafe { allocator.lock().grow(addr, size); };
}

/// 进程退出
pub fn exit() {
    let table = PROCESS_TABLE.read();
    let proc = &table[id()];
    syskrnl::allocator::free_pages(proc.code_addr, MAX_PROC_SIZE);
    MAX_PID.fetch_sub(1, Ordering::SeqCst);
    set_id(proc.parent); // FIXME: 因为目前还不存在调度，所以直接设置为父进程
}

/***************************
 *  用户空间相关。祝我们好运！ *
 ***************************/

static CODE_ADDR: AtomicU64 = AtomicU64::new(0);

/// 初始化进程代码地址，在内核初始化的时候调用
pub fn init_process_addr(addr: u64) {
    CODE_ADDR.store(addr, Ordering::SeqCst);
}

impl Process {
    /// 创建进程
    pub fn spawn(bin: &[u8], args_ptr: usize, args_len: usize) -> Result<(), ExitCode> {
        if let Ok(id) = Self::create(bin) {
            let mut proc = {
                let table = PROCESS_TABLE.read();
                table[id].clone()
            };
            proc.exec(args_ptr, args_len);
            Ok(())
        } else {
            Err(ExitCode::ExecError)
        }
    }

    fn create(bin: &[u8]) -> Result<usize, ()> {
        let proc_size = MAX_PROC_SIZE as u64;
        let code_addr = CODE_ADDR.fetch_add(proc_size, Ordering::SeqCst);
        let stack_addr = code_addr + proc_size - 4096;
        // 紧跟在程序段后面
        debugln!("code_addr:  {:#x}", code_addr);
        debugln!("stack_addr: {:#x}", stack_addr);

        let mut entry_point = 0;
        let code_ptr = code_addr as *mut u8;
        if bin[0..4] == ELF_MAGIC { // 进程代码是ELF格式的
            if let Ok(obj) = object::File::parse(bin) {
                syskrnl::allocator::alloc_pages(code_addr, proc_size as usize).expect("proc mem alloc");
                entry_point = obj.entry();
                debugln!("entry_point:{:#x}",entry_point);
                for segment in obj.segments() {
                    let addr = segment.address() as usize;
                    if let Ok(data) = segment.data() {
                        debugln!("before flight? codeaddr,addr,datalen is {:#x},{:#x},{}", code_addr, addr, data.len());
                        for (i, b) in data.iter().enumerate() {
                            unsafe {
                                //debugln!("code:       {:#x}", code_ptr.add(addr + i) as usize);
                                //debugln!("WRITE: from {:p} to {:p}", b, code_ptr.add(addr + i));
                                core::ptr::write(code_ptr.add(addr + i), *b)
                            }
                        }
                    }
                }
            }
        } else if bin[0..4] == BIN_MAGIC {
            for (i, b) in bin.iter().enumerate() {
                unsafe {
                    core::ptr::write(code_ptr.add(i), *b);
                }
            }
        } else { // 文件头错误
            return Err(());
        }

        // 父进程
        let parent = {
            let table = PROCESS_TABLE.read();
            table[id()].clone()
        };

        let data = parent.data.clone();
        let registers = parent.registers;
        let stack_frame = parent.stack_frame;
        let parent = parent.id;

        // 初始化进程的堆分配器
        let mut allocator = LinkedListAllocator::new();
        let heap_addr = PROC_HEAP_ADDR.fetch_add(DEFAULT_HEAP_SIZE, Ordering::SeqCst);
        syskrnl::allocator::alloc_pages(heap_addr as u64, DEFAULT_HEAP_SIZE).expect("proc heap mem alloc failed");
        unsafe { allocator.init(heap_addr, DEFAULT_HEAP_SIZE) };
        let allocator = Arc::new(Locked::new(allocator));

        let id = MAX_PID.fetch_add(1, Ordering::SeqCst);
        let proc = Process {
            id,
            code_addr,
            stack_addr,
            data,
            registers,
            stack_frame,
            entry_point,
            parent,
            allocator
        };

        let mut table = PROCESS_TABLE.write();
        table[id] = Box::new(proc);

        Ok(id)
    }

    // 切换到用户空间并执行程序
    fn exec(&mut self, args_ptr: usize, args_len: usize) {
        //syskrnl::allocator::alloc_pages(heap_addr, 1).expect("proc heap alloc");

        // 处理参数
        let args_ptr = ptr_from_addr(args_ptr as u64) as usize;
        let args: &[&str] = unsafe {
            core::slice::from_raw_parts(args_ptr as *const &str, args_len)
        };
        if args_len > 0 {
            debugln!("{:?}",args[0]);
        }
        let mut addr = unsafe { self.allocator.lock().alloc(core::alloc::Layout::from_size_align(1024, 1).expect("Layout fault 8569")) as u64 };
        let vec: Vec<&str> = args.iter().map(|arg| {
            let ptr = addr as *mut u8;
            addr += arg.len() as u64;
            unsafe {
                let s = core::slice::from_raw_parts_mut(ptr, arg.len());
                s.copy_from_slice(arg.as_bytes());
                debugln!("{:?}",s);
                core::str::from_utf8_unchecked(s)
            }
        }).collect();
        let align = core::mem::align_of::<&str>() as u64;
        addr += align - (addr % align);
        let args = vec.as_slice();
        let ptr = addr as *mut &str;
        let args: &[&str] = unsafe {
            let s = core::slice::from_raw_parts_mut(ptr, args.len());
            s.copy_from_slice(args);
            s
        };
        let args_ptr = args.as_ptr() as u64;

        debugln!("LAUNCH");
        set_id(self.id); // 要换咯！
        // 发射！
        unsafe {
            asm!(
            "cli",      // 关中断
            "push {:r}",  // Stack segment (SS)
            "push {:r}",  // Stack pointer (RSP)
            "push 0x200", // RFLAGS with interrupts enabled
            "push {:r}",  // Code segment (CS)
            "push {:r}",  // Instruction pointer (RIP)
            "iretq",
            in(reg) syskrnl::gdt::GDT.1.user_data_selector.0,
            in(reg) self.stack_addr,
            in(reg) syskrnl::gdt::GDT.1.user_code_selector.0,
            in(reg) self.code_addr + self.entry_point,
            in("rdi") args_ptr,
            in("rsi") args_len,
            );
        }
    }
}
