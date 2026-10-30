//! Process management syscalls
use crate::task::{change_program_brk, current_user_token, exit_current_and_run_next, mmap_current_task, suspend_current_and_run_next};
use crate::mm::translated_byte_buffer;
use crate::timer::get_time_us;
use core::{mem::size_of, slice};

use crate::task::get_current_syscall_count;

use crate::mm::{PageTable, VirtAddr};

use crate::mm::MapPermission;

use crate::task::munmap_current_task;

const TRACE_REQUEST_READ: usize = 0;
const TRACE_REQUEST_WRITE: usize = 1;
const TRACE_REQUEST_SYSCALL: usize = 2;

#[repr(C)]
#[derive(Debug)]
pub struct TimeVal {
    pub sec: usize,
    pub usec: usize,
}

/// task exits and submit an exit code
pub fn sys_exit(_exit_code: i32) -> ! {
    trace!("kernel: sys_exit");
    exit_current_and_run_next();
    panic!("Unreachable in sys_exit!");
}

/// current task gives up resources for other tasks
pub fn sys_yield() -> isize {
    trace!("kernel: sys_yield");
    suspend_current_and_run_next();
    0
}

/// YOUR JOB: get time with second and microsecond
/// HINT: You might reimplement it with virtual memory management.
/// HINT: What if [`TimeVal`] is splitted by two pages ?
pub fn sys_get_time(ts: *mut TimeVal, _tz: usize) -> isize {
    trace!("kernel: sys_get_time");
    //取得当前微秒数
    let us = get_time_us();
    //在内核中构造时间结果
    let time_val = TimeVal {
        sec: us/1_000_000,
        usec: us % 1_000_000,
    };
    //把内核中的 TimeVal 看成连续的字节 16个u8  16个字节
    let source = unsafe {
        slice::from_raw_parts(
            &time_val as *const TimeVal as *const u8,
            size_of::<TimeVal>(),
        )
    };
    //使用当前运行任务的用户页表，将从用户虚拟地址 ts 开始的整个 TimeVal 地址范围，
    // 按页翻译成一个或多个内核可以访问和写入的物理内存字节切片。
    let buffers = translated_byte_buffer(current_user_token(), ts as *const u8, size_of::<TimeVal>());

    let mut offset = 0;

    for destination in buffers {
        let end = offset + destination.len();
        destination.copy_from_slice(&source[offset..end]);

        offset = end;
    }

    0
}

/// TODO: Finish sys_trace to pass testcases
/// HINT: You might reimplement it with virtual memory management.
pub fn sys_trace(trace_request: usize, id: usize, data: usize) -> isize {
    trace!("kernel: sys_trace");
    match trace_request {
        TRACE_REQUEST_SYSCALL => {
            get_current_syscall_count(id) as isize
        }

        TRACE_REQUEST_READ => {
            let page_table = PageTable::from_token(current_user_token());
            let va = VirtAddr::from(id);
            match page_table.translate(va.floor()) {
                Some(pte)
                    if pte.is_valid()
                        && pte.user_accessible()
                        && pte.readable() =>
                        {
                            //先执行 PPN << 12，得到物理页起始地址，把这个物理页包装成 4096 字节数组，(PPN << 12)
                            let bytes = pte.ppn().get_bytes_array();
                            //在这个物理页中取第 page_offset 个字节==*(bytes.as_ptr().add(offset))
                            bytes[va.page_offset()] as isize
                        }
                _ => -1
            }

        }

        TRACE_REQUEST_WRITE => {
            let page_table = PageTable::from_token(current_user_token());
            let va = VirtAddr::from(id);

            match page_table.translate(va.floor()) {
                Some(pte) 
                    if pte.is_valid() 
                    && pte.user_accessible()
                    && pte.writable() =>
                    {
                        let bytes = pte.ppn().get_bytes_array();
                        bytes[va.page_offset()] = data as u8;
                        0
                    }
                _ => -1
            }
            
        }
        _ => -1
    }
}

// YOUR JOB: Implement mmap.
pub fn sys_mmap(start: usize, len: usize, port: usize) -> isize {
    trace!("kernel: sys_mmap NOT IMPLEMENTED YET!");

    let start_va = VirtAddr::from(start);

    if !start_va.aligned() {
        return -1;
    }

    if port & !0x7 != 0 {
        return -1;
    }

    if port &0x7 == 0 {
        return -1;
    }

    if len == 0 {
        return 0;
    }

    let end = match start.checked_add(len) {
        Some(end) => end,
        None => return -1
    };

    let mut permission = MapPermission::U;

    if port & 0x1 != 0 {
        permission |= MapPermission::R;
    }

    if port & 0x2 != 0 {
        permission |= MapPermission::W;
    }

    if port & 0x4 != 0 {
        permission |= MapPermission::X;
    }

    if mmap_current_task(start, end, permission) {
        0
    }
    else {
        -1
    }
}

// YOUR JOB: Implement munmap.
pub fn sys_munmap(start: usize, len: usize) -> isize {
    trace!("kernel: sys_munmap NOT IMPLEMENTED YET!");

    let start_va = VirtAddr::from(start);

    if !start_va.aligned() {
        return -1;
    }

    if len == 0 {
        return 0;
    }

    let end = match start.checked_add(len) {
        Some(end) => end,
        None => return -1
    };

    if munmap_current_task(start, end) {
        0
    }
    else
    {
        -1
    }
}
/// change data segment size
pub fn sys_sbrk(size: i32) -> isize {
    trace!("kernel: sys_sbrk");
    if let Some(old_brk) = change_program_brk(size) {
        old_brk as isize
    } else {
        -1
    }
}
