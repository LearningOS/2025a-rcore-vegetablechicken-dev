//! Types related to task management

use super::TaskContext;
use crate::syscall::*;

/// The task control block (TCB) of a task.
#[derive(Copy, Clone)]
pub struct TaskControlBlock {
    /// The task status in it's lifecycle
    pub task_status: TaskStatus,
    /// The task context
    pub task_cx: TaskContext,
    /// The times of syscall
    /// The index of the array is syscall id
    pub task_syscall_times: SyscallTimes,
}

/// The status of a task
#[derive(Copy, Clone, PartialEq)]
pub enum TaskStatus {
    /// uninitialized
    UnInit,
    /// ready to run
    Ready,
    /// running
    Running,
    /// exited
    Exited,
}
/// Record the times of syscall
#[derive(Copy, Clone)]
pub struct SyscallTimes {
    sys_write_times: usize,
    sys_exit_times: usize,
    sys_yield_times: usize,
    sys_get_time_times: usize,
    sys_trace_times: usize,
}

impl SyscallTimes {
    /// Init the syscall times 0
    pub fn new() -> SyscallTimes {
        SyscallTimes {
            sys_write_times: 0,
            sys_exit_times: 0,
            sys_yield_times: 0,
            sys_get_time_times: 0,
            sys_trace_times: 0,
        }
    }
    /// Get syscall times using syscall id
    pub fn get_times(&self, syscall_id: usize) -> usize {
        match syscall_id {
            SYSCALL_WRITE=> self.sys_write_times,
            SYSCALL_EXIT => self.sys_exit_times,
            SYSCALL_YIELD => self.sys_yield_times,
            SYSCALL_GET_TIME => self.sys_get_time_times,
            SYSCALL_TRACE => self.sys_trace_times,
            _ => 0,
        }
    }
    /// Syscall times plus 1
    pub fn record_times(&mut self, syscall_id: usize) {
        match syscall_id {
            SYSCALL_WRITE => self.sys_write_times += 1,
            SYSCALL_EXIT => self.sys_exit_times += 1,
            SYSCALL_YIELD => self.sys_yield_times += 1,
            SYSCALL_GET_TIME => self.sys_get_time_times += 1,
            SYSCALL_TRACE => self.sys_trace_times += 1,
            _ => (),
        }
    }
}
