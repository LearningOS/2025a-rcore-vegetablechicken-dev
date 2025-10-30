//! Process management syscalls
use crate::task::{change_program_brk, exit_current_and_run_next, suspend_current_and_run_next
                , current_user_token, get_syscall_count, mmap, munmap};
use crate::mm::{page_table, address::{VirtAddr}};
use crate::timer::get_time_us;

use core::mem::size_of;
use core::slice;
use core::ptr;
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
    let time_val_size = size_of::<TimeVal>();
    let buffers = page_table::translated_byte_buffer(current_user_token(),
                                                     ts as *const u8,
                                                     time_val_size);
    let us = get_time_us();
    let time_val = TimeVal {
        sec: us / 1_000_000,
        usec: us % 1_000_000,
    };
    let mut cur = 0;
    let time_slice = unsafe {
        slice::from_raw_parts(&time_val as *const TimeVal as *const u8, time_val_size)
    };
    for buffer in buffers {
        let write_len = buffer.len().min(time_slice.len() - cur);
        unsafe {
            ptr::copy(&time_slice[cur..] as *const _ as *const u8,
                      buffer as *mut _ as *mut u8,
                      write_len);
        }
        cur += write_len;
    }
    0
}

/// Finish sys_trace to pass testcases
/// You might reimplement it with virtual memory management.
pub fn sys_trace(trace_request: usize, id: usize, data: usize) -> isize {
    trace!("kernel: sys_trace");
    let mut ret = -1;
    if trace_request == 0 {
        let ptr = page_table::translated_const_ptr(current_user_token(), id);
        ret = match ptr {
            Some(p) => unsafe { *(p as *const u8) as isize },
            None => -1
        };
    } else if trace_request == 1 {
        let ptr = page_table::translated_mut_ptr(current_user_token(), id);
        ret = match ptr {
            Some(p) => { unsafe { *(p as *mut u8) = data as u8 }; 0 },
            None => -1
        }
    } else if trace_request == 2 {
        ret = get_syscall_count(id);
    }
    ret
}

// YOUR JOB: Implement mmap.
pub fn sys_mmap(start: usize, len: usize, prot: usize) -> isize {
    trace!("kernel: sys_mmap NOT IMPLEMENTED YET!");
    if prot & 0x7 == 0 || prot & !0x7 != 0 {
        return -1;
    }
    if !VirtAddr::from(start).aligned() {
        return -1;
    }
    mmap(start, len, prot)
}

// YOUR JOB: Implement munmap.
pub fn sys_munmap(start: usize, len: usize) -> isize {
    trace!("kernel: sys_munmap NOT IMPLEMENTED YET!");
    munmap(start, len)
    -1
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
