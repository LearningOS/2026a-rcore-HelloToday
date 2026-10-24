//! Process management syscalls
use crate::{
    task::{exit_current_and_run_next, suspend_current_and_run_next},
    timer::get_time_us,
};

use crate::task::get_current_syscall_count;

///查询系统调用次数的trace请求编号
const TRACE_REQUEST_SYSCALL: usize = 2;
///读取用户内存的trace请求编号
const TRACE_REQUEST_WRITE: usize = 1;
///写入用户内存的trace请求编号
const TRACE_REQUEST_READ: usize = 0;

#[repr(C)]
#[derive(Debug)]
pub struct TimeVal {
    pub sec: usize,
    pub usec: usize,
}

/// task exits and submit an exit code
pub fn sys_exit(exit_code: i32) -> ! {
    trace!("[kernel] Application exited with code {}", exit_code);
    exit_current_and_run_next();
    panic!("Unreachable in sys_exit!");
}

/// current task gives up resources for other tasks
pub fn sys_yield() -> isize {
    trace!("kernel: sys_yield");
    suspend_current_and_run_next();
    0
}

/// get time with second and microsecond
pub fn sys_get_time(ts: *mut TimeVal, _tz: usize) -> isize {
    trace!("kernel: sys_get_time");
    let us = get_time_us();
    unsafe {
        *ts = TimeVal {
            sec: us / 1_000_000,
            usec: us % 1_000_000,
        };
    }
    0
}

// TODO: implement the syscall
pub fn sys_trace(trace_request: usize, id: usize, data: usize) -> isize {
    trace!("kernel: sys_trace");
    match trace_request {
        TRACE_REQUEST_SYSCALL => get_current_syscall_count(id) as isize,
        TRACE_REQUEST_READ => unsafe { *(id as *const u8) as isize },
        TRACE_REQUEST_WRITE => {
            unsafe { *(id as *mut u8) = data as u8 };
            0
        }
        _ => -1,
    }
}
